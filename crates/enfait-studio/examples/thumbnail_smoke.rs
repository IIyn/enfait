//! Manual test of the thumbnail render, without a model:
//! `cargo run -p enfait-studio --example thumbnail_smoke -- <folder>`.
//! Writes `v1.png`, `mini.jpg` and `mini.psd` (layers) in `<folder>`.
#![allow(clippy::print_stdout)]
use enfait_motion::{Gpu, Renderer, composition, ensure_browser, library, still};
use enfait_studio::thumbnail;
use std::path::PathBuf;

const HTML: &str = r##"<!doctype html><html><head><style>
html,body{margin:0;width:1280px;height:720px;overflow:hidden;background:#101312}
.l{position:absolute}
</style></head><body>
<div class="l" data-layer="Fond" style="inset:0;background:radial-gradient(circle at 30% 40%,#2c3b35,#0c0f0e)"></div>
<div class="l" data-layer="Halo" style="left:640px;top:80px;width:560px;height:560px;border-radius:50%;background:#F5B53D;filter:blur(40px);opacity:.55"></div>
<div class="l" data-layer="Titre" style="left:70px;top:190px;font:600 150px/1 'Archivo';color:#fff;text-shadow:0 8px 30px rgba(0,0,0,.6)">30 JOURS<br><span style="color:#F5B53D">SANS</span></div>
<svg class="l" data-layer="Flèche" style="left:820px;top:420px" width="300" height="200"><path d="M10 150 C 120 20, 200 20, 280 90" stroke="#fff" stroke-width="14" fill="none" stroke-linecap="round"/></svg>
</body></html>"##;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "thumb-smoke".into()),
    );
    std::fs::create_dir_all(&dir)?;
    library::install(&dir)?;
    let cache = directories::ProjectDirs::from("", "", "enfait")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    let chrome = ensure_browser(&cache.join("chromium")).await?;
    let renderer = Renderer::launch(&chrome, &cache.join("profile-smoke"), Gpu::Software).await?;
    let html = thumbnail::html_path(&dir, 1);
    std::fs::write(&html, still::make_static(&composition::prepare(HTML)))?;
    let started = std::time::Instant::now();
    let rendered = enfait_motion::render_still(
        &renderer,
        &html,
        thumbnail::WIDTH,
        thumbnail::HEIGHT,
        thumbnail::SCALE,
        false,
    )
    .await?;
    rendered.composite.save(thumbnail::png_path(&dir, 1))?;
    println!(
        "picture {:?} in {:?}, errors: {:?}",
        rendered.composite.dimensions(),
        started.elapsed(),
        rendered.errors
    );
    let size = thumbnail::export_image(&dir, 1, &dir.join("mini.jpg"))?;
    println!("mini.jpg: {size} bytes");
    let started = std::time::Instant::now();
    let layers = thumbnail::export_psd(&renderer, &dir, 1, &dir.join("mini.psd")).await?;
    println!("mini.psd: {layers} layers in {:?}", started.elapsed());
    renderer.close().await;
    Ok(())
}
