//! Frame-by-frame rendering in a headless Chromium.

use crate::error::MotionError;
use crate::library;
use base64::Engine as _;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::headless_experimental::{
    BeginFrameParams, ScreenshotParams as HeadlessScreenshot, ScreenshotParamsFormat,
};
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::task::JoinHandle;

const RUNTIME_JS: &str = include_str!("runtime.js");

/// Margin of the safe area, as a fraction of width and height.
pub const SAFE_MARGIN: f64 = 0.05;

/// chromiumoxide adds the `--` in front of each option itself: it is
/// left out here to keep the options readable.
fn flags(list: &[&str]) -> Vec<String> {
    list.iter()
        .map(|f| f.trim_start_matches("--").to_owned())
        .collect()
}

/// Render acceleration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gpu {
    /// Software rendering (SwiftShader), available everywhere.
    #[default]
    Software,
    /// GPU rendering through ANGLE/Vulkan.
    Vulkan {
        /// On a hybrid laptop, force the NVIDIA card.
        nvidia_offload: bool,
    },
}

/// A running Chromium, shared by several renders.
pub struct Renderer {
    browser: Browser,
    handler: JoinHandle<()>,
}

impl Renderer {
    pub async fn launch(
        executable: &Path,
        profile_dir: &Path,
        gpu: Gpu,
    ) -> Result<Self, MotionError> {
        let mut builder = BrowserConfig::builder()
            .chrome_executable(executable)
            .user_data_dir(profile_dir)
            .viewport(None)
            .launch_timeout(Duration::from_secs(30))
            .request_timeout(Duration::from_secs(60))
            .args(flags(&[
                "--hide-scrollbars",
                "--mute-audio",
                "--force-color-profile=srgb",
                "--font-render-hinting=none",
                // No network request: only local resources load.
                "--host-resolver-rules=MAP * ~NOTFOUND",
                "--allow-file-access-from-files",
                // Frames triggered one by one by `HeadlessExperimental.beginFrame`.
                "--enable-begin-frame-control",
                "--run-all-compositor-stages-before-draw",
                "--disable-new-content-rendering-timeout",
                "--disable-threaded-animation",
                "--disable-threaded-scrolling",
                "--disable-checker-imaging",
            ]));
        match gpu {
            Gpu::Software => {}
            Gpu::Vulkan { nvidia_offload } => {
                builder = builder.args(flags(&[
                    "--use-angle=vulkan",
                    "--enable-features=Vulkan",
                    "--ignore-gpu-blocklist",
                    "--enable-gpu-rasterization",
                    "--enable-zero-copy",
                ]));
                if nvidia_offload {
                    // Hybrid laptop: force the NVIDIA card rather than the iGPU.
                    builder = builder
                        .env("__NV_PRIME_RENDER_OFFLOAD", "1")
                        .env("__VK_LAYER_NV_optimus", "NVIDIA_only");
                }
            }
        }
        // Inside a Flatpak, Chromium cannot create its own sandbox (no user
        // namespaces): Flatpak's sandbox stands in, and the page has no
        // network anyway (`--host-resolver-rules` above).
        if Path::new("/.flatpak-info").exists() {
            builder = builder.no_sandbox();
        }
        // Experimentation setting: extra Chromium options.
        if let Ok(extra) = std::env::var("ENFAIT_CHROME_ARGS") {
            builder = builder.args(flags(&extra.split_whitespace().collect::<Vec<_>>()));
        }
        let config = builder.build().map_err(MotionError::Browser)?;
        let (browser, mut handler) = Browser::launch(config).await?;
        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    tracing::debug!("chromium : {e}");
                }
            }
        });
        Ok(Self { browser, handler })
    }

    /// Opens `html` in a new `width`×`height` tab and waits until the
    /// composition is ready.
    pub async fn open(
        &self,
        html: &Path,
        width: u32,
        height: u32,
    ) -> Result<OpenStage, MotionError> {
        let html = std::path::absolute(html)?;
        let url = file_url(&html)?;
        let mut target = CreateTargetParams::new("about:blank");
        target.enable_begin_frame_control = Some(true);
        let page = self.browser.new_page(target).await?;
        page.execute(SetDeviceMetricsOverrideParams::new(
            i64::from(width),
            i64::from(height),
            1.0,
            false,
        ))
        .await?;
        page.evaluate_on_new_document(RUNTIME_JS).await?;
        page.goto(url).await?;
        let stage = Stage { page };
        let ready: Ready = stage.eval("window.__enfait.ready()").await?;
        if !ready.ok {
            let reason = ready.reason.unwrap_or_default();
            let errors = ready.errors.join(" ; ");
            stage.close().await;
            return Err(MotionError::Contract(
                format!("{reason} {errors}").trim().to_owned(),
            ));
        }
        Ok(stage.with_duration(ready.duration, ready.errors))
    }

    pub async fn close(mut self) {
        if let Err(e) = self.browser.close().await {
            tracing::debug!("fermeture de chromium : {e}");
        }
        let _ = self.browser.wait().await;
        self.handler.abort();
    }
}

#[derive(Deserialize)]
struct Ready {
    ok: bool,
    duration: f64,
    errors: Vec<String>,
    reason: Option<String>,
}

/// An open tab.
struct Stage {
    page: Page,
}

/// A loaded composition, with its declared duration.
pub struct OpenStage {
    stage: Stage,
    /// Last captured frame: Chromium produces no new one when nothing changed
    /// on screen.
    last: std::sync::Mutex<Option<Vec<u8>>>,
    pub duration: f64,
    /// Errors raised while loading.
    pub load_errors: Vec<String>,
}

impl Stage {
    fn with_duration(self, duration: f64, load_errors: Vec<String>) -> OpenStage {
        OpenStage {
            stage: self,
            last: std::sync::Mutex::new(None),
            duration,
            load_errors,
        }
    }

    async fn eval<T: for<'de> Deserialize<'de>>(&self, expression: &str) -> Result<T, MotionError> {
        let params = EvaluateParams::builder()
            .expression(expression)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .map_err(MotionError::Browser)?;
        let result = self.page.evaluate_expression(params).await?;
        Ok(result.into_value()?)
    }

    async fn close(self) {
        if let Err(e) = self.page.close().await {
            tracing::debug!("fermeture d'onglet : {e}");
        }
    }
}

/// Frames of the neighbouring scenes for a transition frame.
pub struct Scenes<'a> {
    pub from: &'a Path,
    pub to: &'a Path,
}

/// Problems found by [`OpenStage::inspect`] at a given time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub outside: Vec<OutsideText>,
    pub unknown_fonts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutsideText {
    pub text: String,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl OpenStage {
    /// Moves the composition to `t` seconds.
    pub async fn seek(&self, t: f64, scenes: Option<Scenes<'_>>) -> Result<(), MotionError> {
        let scenes = match scenes {
            Some(s) => {
                let from = file_url(s.from)?;
                let to = file_url(s.to)?;
                serde_json::json!({ "from": from, "to": to }).to_string()
            }
            None => "null".to_owned(),
        };
        let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
        let _: bool = self
            .stage
            .eval(&format!("window.__enfait.seek({t}, {scenes})"))
            .await?;
        Ok(())
    }

    /// Captures the current frame as JPEG quality 95: the PNG of a textured
    /// frame (grain, noise) costs up to 400 ms of compression.
    pub async fn capture_jpeg(&self) -> Result<Vec<u8>, MotionError> {
        // "Frame control" mode: we trigger the frame ourselves and the capture
        // comes out directly, without waiting for Chromium's pace.
        let screenshot = HeadlessScreenshot::builder()
            .format(ScreenshotParamsFormat::Jpeg)
            .quality(95)
            .build();
        for _ in 0..3 {
            let frame = self
                .stage
                .page
                .execute(
                    BeginFrameParams::builder()
                        .screenshot(screenshot.clone())
                        .build(),
                )
                .await?;
            if let Some(data) = &frame.result.screenshot_data {
                let data: &str = data.as_ref();
                let png = base64::engine::general_purpose::STANDARD.decode(data)?;
                if let Ok(mut last) = self.last.lock() {
                    *last = Some(png.clone());
                }
                return Ok(png);
            }
            // No damage on screen: the frame is identical to the previous one.
            if !frame.result.has_damage
                && let Some(previous) = self.last.lock().ok().and_then(|l| l.clone())
            {
                return Ok(previous);
            }
        }
        Err(MotionError::Browser(
            "no frame produced by beginFrame".into(),
        ))
    }

    /// JavaScript errors gathered since opening.
    pub async fn errors(&self) -> Result<Vec<String>, MotionError> {
        self.stage.eval("window.__enfait.errors").await
    }

    /// Text outside the safe area and unknown fonts at the current time.
    pub async fn inspect(&self) -> Result<Inspection, MotionError> {
        let families = serde_json::to_string(&library::font_families())?;
        self.stage
            .eval(&format!(
                "window.__enfait.inspect({SAFE_MARGIN}, {families})"
            ))
            .await
    }

    pub async fn close(self) {
        self.stage.close().await;
    }
}

fn file_url(path: &Path) -> Result<String, MotionError> {
    let path = std::path::absolute(path)?;
    url::Url::from_file_path(&path)
        .map(String::from)
        .map_err(|()| MotionError::Path(path.clone()))
}
