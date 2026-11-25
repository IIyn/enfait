//! Still pictures (thumbnails) made of layers.
//!
//! A still is an HTML page like a composition, without animation. Each
//! direct child of `<body>` is a layer, from the bottom up, named by its
//! `data-layer` attribute; the background of `body` is the bottom layer.
//!
//! The layers are captured one by one, the others made invisible, over a
//! black then a white background: the difference between the two gives
//! the transparency of each pixel (difference matting), whatever the
//! effects used (shadows, blurs, gradients).

use crate::error::MotionError;
use crate::render::{OpenStage, Renderer};
use image::{Rgba, RgbaImage};
use std::path::Path;

/// Name of the bottom layer (background of the page).
pub const BACKGROUND_LAYER: &str = "Background";

/// A rendered still.
pub struct Still {
    /// The picture as it shows (opaque).
    pub composite: RgbaImage,
    /// Layers from the bottom up, at the size of the picture, with straight
    /// (non premultiplied) alpha. Empty if not asked for.
    pub layers: Vec<StillLayer>,
    /// JavaScript errors of the page.
    pub errors: Vec<String>,
}

pub struct StillLayer {
    pub name: String,
    pub image: RgbaImage,
}

/// Script declaring the contract for a still page (no animation), to add
/// to its document.
pub const STATIC_CONTRACT: &str =
    "<script>window.enfait = window.enfait || { duration: 1, seek() {} };</script>";

/// Adds [`STATIC_CONTRACT`] at the end of the document.
pub fn make_static(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    match lower.rfind("</body>") {
        Some(at) => {
            let (before, after) = html.split_at(at);
            format!("{before}{STATIC_CONTRACT}\n{after}")
        }
        None => format!("{html}\n{STATIC_CONTRACT}"),
    }
}

/// Layers of the page: direct children of `body` that show something.
const LAYERS_JS: &str = r#"(() => {
  const skip = new Set(["SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "LINK", "META"]);
  window.__enfaitLayers = Array.from(document.body.children).filter((e) => !skip.has(e.tagName));
  return window.__enfaitLayers.map((e, i) =>
    (e.getAttribute("data-layer") || e.id || "").trim() || ("Layer " + (i + 1)));
})()"#;

/// Shows only layer `index` (-1: none) over `background` (`null`: the
/// page's own background).
fn solo_js(index: i64, background: Option<&str>) -> String {
    let background = background.map_or_else(String::new, |b| {
        format!("html, body {{ background: {b} !important; background-image: none !important; }}")
    });
    format!(
        r#"(() => {{
  let style = document.getElementById("__enfait-solo");
  if (!style) {{
    style = document.createElement("style");
    style.id = "__enfait-solo";
    document.head.appendChild(style);
  }}
  style.textContent = `* {{ transition: none !important; }} [data-enfait-off] {{ opacity: 0 !important; }} {background}`;
  window.__enfaitLayers.forEach((e, i) => {{
    if (i === {index}) e.removeAttribute("data-enfait-off");
    else e.setAttribute("data-enfait-off", "");
  }});
  return true;
}})()"#
    )
}

const RESET_JS: &str = r#"(() => {
  document.getElementById("__enfait-solo")?.remove();
  (window.__enfaitLayers || []).forEach((e) => e.removeAttribute("data-enfait-off"));
  return true;
})()"#;

async fn grab(stage: &OpenStage) -> Result<RgbaImage, MotionError> {
    let png = stage.capture_png().await?;
    let image = image::load_from_memory(&png).map_err(|e| MotionError::Browser(e.to_string()))?;
    Ok(image.to_rgba8())
}

/// Renders the still `html`, `width`×`height` CSS pixels at `scale` device
/// pixels per CSS pixel; with `layers`, also each layer apart.
pub async fn render_still(
    renderer: &Renderer,
    html: &Path,
    width: u32,
    height: u32,
    scale: f64,
    layers: bool,
) -> Result<Still, MotionError> {
    // The device pixel ratio is ignored by the captures in frame control
    // mode: the page is zoomed instead, in a viewport of the final size.
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let device = |v: u32| {
        // A few thousand pixels at most.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let d = (f64::from(v) * scale).round() as u32;
        d.max(1)
    };
    let stage = renderer.open(html, device(width), device(height)).await?;
    let result = async {
        if (scale - 1.0).abs() > 1e-6 {
            let _: bool = stage
                .eval(&format!(
                    "(() => {{ document.documentElement.style.zoom = '{scale}'; return true; }})()"
                ))
                .await?;
        }
        stage.seek(0.0, None).await?;
        let composite = grab(&stage).await?;
        let mut out = Vec::new();
        if layers {
            let names: Vec<String> = stage.eval(LAYERS_JS).await?;
            // The page's own background, alone.
            let _: bool = stage.eval(&solo_js(-1, None)).await?;
            let mut background = grab(&stage).await?;
            for p in background.pixels_mut() {
                p.0[3] = 255;
            }
            out.push(StillLayer {
                name: BACKGROUND_LAYER.to_owned(),
                image: background,
            });
            for (i, name) in names.into_iter().enumerate() {
                let index = i64::try_from(i).unwrap_or(i64::MAX);
                let _: bool = stage.eval(&solo_js(index, Some("#000"))).await?;
                let black = grab(&stage).await?;
                let _: bool = stage.eval(&solo_js(index, Some("#fff"))).await?;
                let white = grab(&stage).await?;
                let image = matte(&black, &white);
                if image.pixels().any(|p| p.0[3] > 0) {
                    out.push(StillLayer { name, image });
                }
            }
            let _: bool = stage.eval(RESET_JS).await?;
        }
        let mut errors = stage.load_errors.clone();
        errors.extend(stage.errors().await?);
        errors.dedup();
        Ok(Still {
            composite,
            layers: out,
            errors,
        })
    }
    .await;
    stage.close().await;
    result
}

/// Layer with transparency from its two captures, over black and over
/// white: `white - black = (1 - alpha) × 255` and `black = colour × alpha`.
pub fn matte(black: &RgbaImage, white: &RgbaImage) -> RgbaImage {
    let (w, h) = black.dimensions();
    let mut out = RgbaImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let [br, bg, bb, _] = black.get_pixel(x, y).0;
        let [wr, wg, wb, _] = white
            .get_pixel_checked(x, y)
            .map_or([br, bg, bb, 255], |p| p.0);
        let spread = u32::from(wr.saturating_sub(br))
            + u32::from(wg.saturating_sub(bg))
            + u32::from(wb.saturating_sub(bb));
        // Mean over the three channels: JPEG-free captures, but the
        // rasteriser rounds each channel on its own.
        let alpha = 255_u32.saturating_sub(spread / 3);
        if alpha == 0 {
            *p = Rgba([0, 0, 0, 0]);
            continue;
        }
        let colour = |c: u8| {
            let v = u32::from(c) * 255 / alpha;
            u8::try_from(v.min(255)).unwrap_or(255)
        };
        *p = Rgba([
            colour(br),
            colour(bg),
            colour(bb),
            u8::try_from(alpha).unwrap_or(255),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matte_recovers_alpha() {
        let mut black = RgbaImage::new(3, 1);
        let mut white = RgbaImage::new(3, 1);
        // Opaque red, nothing, red at half transparency.
        black.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        white.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        black.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        white.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        black.put_pixel(2, 0, Rgba([128, 0, 0, 255]));
        white.put_pixel(2, 0, Rgba([255, 127, 127, 255]));
        let out = matte(&black, &white);
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0[3], 0);
        let half = out.get_pixel(2, 0).0;
        assert_eq!(half[3], 128);
        assert!(half[0] >= 254 && half[1] == 0);
    }

    #[test]
    fn static_contract_goes_before_body_end() {
        let html = make_static("<html><body><p>a</p></body></html>");
        assert!(html.contains("seek() {}"));
        assert!(html.find("window.enfait").unwrap() < html.find("</body>").unwrap());
        assert!(make_static("<p>a</p>").ends_with("</script>"));
    }
}
