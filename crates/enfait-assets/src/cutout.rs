//! Cutout: a segmentation model (ONNX, run by `tract`, in pure Rust) gives
//! a mask of the subject, set as the alpha channel.
//!
//! Two models, downloaded on first use (rembg repository) and checked by
//! fingerprint:
//! - [`Model::Precise`]: ISNet (DIS, Apache-2.0), 1024 px, a few seconds
//!   per image, for library files;
//! - [`Model::Fast`]: U²-Net-p (Apache-2.0), 320 px, under a second, for
//!   the frames of a filmed take.

use crate::AssetError;
use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Calls to `tract`, isolated (its prelude exports many names).
mod engine {
    use tract_onnx::prelude::*;

    pub type Plan = std::sync::Arc<TypedRunnableModel>;

    pub fn load(path: &std::path::Path, side: usize) -> TractResult<Plan> {
        tract_onnx::onnx()
            .model_for_path(path)?
            .with_input_fact(0, f32::fact([1, 3, side, side]).into())?
            .into_optimized()?
            .into_runnable()
    }

    /// First output of the model for an input `[1, 3, side, side]`.
    pub fn run(plan: &Plan, side: usize, input: &[f32]) -> TractResult<Vec<f32>> {
        let tensor = Tensor::from_shape(&[1, 3, side, side], input)?;
        let outputs = plan.run(tvec!(tensor.into()))?;
        let first = outputs
            .first()
            .ok_or_else(|| TractError::msg("no output"))?;
        Ok(first
            .to_plain_array_view::<f32>()?
            .iter()
            .copied()
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Precise,
    Fast,
}

impl Model {
    const fn file(self) -> &'static str {
        match self {
            Self::Precise => "isnet-general-use.onnx",
            Self::Fast => "u2netp.onnx",
        }
    }

    const fn url(self) -> &'static str {
        match self {
            Self::Precise => {
                "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx"
            }
            Self::Fast => {
                "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx"
            }
        }
    }

    /// BLAKE3 fingerprint of the expected file.
    const fn hash(self) -> &'static str {
        match self {
            Self::Precise => "a4803d041160d956c81853125e6391608a3e9b7772e37bcbcc86aa3d87d6b214",
            Self::Fast => "8b4bde6560ee1edb10b48184ecad7895a61aa8ebfd7c651089a7990172eb7b74",
        }
    }

    /// Side of the image given to the model.
    const fn size(self) -> usize {
        match self {
            Self::Precise => 1024,
            Self::Fast => 320,
        }
    }

    /// Colour normalisation (mean, standard deviation).
    const fn normalize(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Precise => ([0.5; 3], [1.0; 3]),
            Self::Fast => ([0.485, 0.456, 0.406], [0.229, 0.224, 0.225]),
        }
    }
}

/// Path of the model in `dir`, downloaded and checked if missing.
pub async fn ensure_model(dir: &Path, model: Model) -> Result<PathBuf, AssetError> {
    let path = dir.join(model.file());
    if path.is_file() {
        return Ok(path);
    }
    tracing::info!("downloading cutout model {}", model.file());
    let client = crate::search::client()?;
    let partial = dir.join(format!("{}.download", model.file()));
    crate::search::download(&client, model.url(), &partial).await?;
    let data = tokio::fs::read(&partial).await?;
    let hash = blake3::hash(&data).to_hex();
    if hash.as_str() != model.hash() {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(AssetError::Source(format!(
            "model {}: unexpected fingerprint",
            model.file()
        )));
    }
    tokio::fs::rename(&partial, &path).await?;
    Ok(path)
}

/// A loaded model, ready to cut out (usable from several threads).
pub struct Cutter {
    model: Model,
    plan: engine::Plan,
}

impl Cutter {
    pub fn load(path: &Path, model: Model) -> Result<Self, AssetError> {
        let side = model.size();
        let plan = engine::load(path, side).map_err(|e| AssetError::Model(format!("{e:#}")))?;
        Ok(Self { model, plan })
    }

    /// Mask of the subject (255: subject, 0: background), at the size of `image`.
    pub fn mask(&self, image: &RgbImage) -> Result<GrayImage, AssetError> {
        let side = self.model.size();
        let side_u32 = u32::try_from(side).unwrap_or(u32::MAX);
        let small = image::imageops::resize(
            image,
            side_u32,
            side_u32,
            image::imageops::FilterType::Triangle,
        );
        let (mean, std) = self.model.normalize();
        // Like rembg: colours divided by the highest value of the image.
        let max = f32::from(small.pixels().flat_map(|p| p.0).max().unwrap_or(255).max(1));
        let mut input = vec![0.0_f32; 3 * side * side];
        for (x, y, Rgb(px)) in small.enumerate_pixels() {
            let (x, y) = (x as usize, y as usize);
            for (c, value) in px.iter().enumerate() {
                let (m, s) = (
                    mean.get(c).copied().unwrap_or(0.0),
                    std.get(c).copied().unwrap_or(1.0),
                );
                let index = c * side * side + y * side + x;
                if let Some(slot) = input.get_mut(index) {
                    *slot = (f32::from(*value) / max - m) / s;
                }
            }
        }
        let values = engine::run(&self.plan, side, &input)
            .map_err(|e| AssetError::Model(format!("{e:#}")))?;
        let plane = values
            .get(..side * side)
            .ok_or_else(|| AssetError::Model("sortie trop petite".into()))?;
        let (lo, hi) = plane
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let span = (hi - lo).max(1e-6);
        let mut mask = GrayImage::new(side_u32, side_u32);
        for (i, v) in plane.iter().enumerate() {
            let (x, y) = (i % side, i / side);
            // Value brought back into [0, 255].
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let level = (((v - lo) / span) * 255.0).round().clamp(0.0, 255.0) as u8;
            mask.put_pixel(
                u32::try_from(x).unwrap_or(0),
                u32::try_from(y).unwrap_or(0),
                Luma([level]),
            );
        }
        Ok(image::imageops::resize(
            &mask,
            image.width(),
            image.height(),
            image::imageops::FilterType::Triangle,
        ))
    }

    /// The cut-out image: its colours, with the mask as alpha channel.
    pub fn cut(&self, image: &RgbImage) -> Result<RgbaImage, AssetError> {
        let mask = self.mask(image)?;
        Ok(ImageBuffer::from_fn(
            image.width(),
            image.height(),
            |x, y| {
                let Rgb([r, g, b]) = *image.get_pixel(x, y);
                let Luma([a]) = *mask.get_pixel(x, y);
                Rgba([r, g, b, a])
            },
        ))
    }
}

/// Cuts out the file `input` (image) and writes a transparent PNG to `out`.
pub fn cut_file(cutter: &Cutter, input: &Path, out: &Path) -> Result<(), AssetError> {
    let image = image::open(input)
        .map_err(|e| AssetError::Image(e.to_string()))?
        .to_rgb8();
    let cut = cutter.cut(&image)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    cut.save_with_format(out, image::ImageFormat::Png)
        .map_err(|e| AssetError::Image(e.to_string()))
}

/// Cuts out a sequence of images (`frames[i]` → `outs[i]`, PNG), on
/// `threads` threads; `progress` gets the number of images done.
pub fn cut_frames(
    cutter: &Cutter,
    jobs: &[(PathBuf, PathBuf)],
    threads: usize,
    progress: &(dyn Fn(usize) + Sync),
) -> Result<(), AssetError> {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let errors = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::AcqRel);
                    let Some((input, out)) = jobs.get(i) else {
                        break;
                    };
                    let result = image::open(input)
                        .map_err(|e| AssetError::Image(e.to_string()))
                        .and_then(|img| cutter.cut(&img.to_rgb8()))
                        .and_then(|cut| save_png_fast(&cut, out));
                    if let Err(e) = result {
                        if let Ok(mut errors) = errors.lock() {
                            errors.push(e);
                        }
                        break;
                    }
                    progress(done.fetch_add(1, Ordering::AcqRel).saturating_add(1));
                }
            });
        }
    });
    match errors.into_inner().ok().and_then(|mut e| e.pop()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Fast-compression PNG (working images, not an archive).
fn save_png_fast(image: &RgbaImage, out: &Path) -> Result<(), AssetError> {
    use image::ImageEncoder as _;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::fs::File::create(out)?;
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Fast,
        FilterType::Sub,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| AssetError::Image(e.to_string()))
}
