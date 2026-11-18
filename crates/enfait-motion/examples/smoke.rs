//! Essai manuel : `cargo run -p enfait-motion --example smoke -- <dossier> [page.html]`.
#![allow(clippy::print_stdout)]
use enfait_motion::{CheckJob, FrameJob, Gpu, Renderer, composition, ensure_browser, library};
use std::path::PathBuf;

const HTML: &str = r##"<!doctype html><html><head><style>
html,body{margin:0;background:#101312;color:#E6ECE9;font-family:"Fraunces";overflow:hidden}
h1{position:absolute;left:160px;top:420px;font-size:140px;margin:0;font-weight:600}
.bar{position:absolute;left:160px;top:620px;height:12px;width:900px;background:#C3F53C;transform-origin:left}
svg{position:absolute;left:1200px;top:300px}
</style></head><body><h1 id="t">Salut à tous</h1><div class="bar"></div><div id="n" style="position:absolute;left:160px;top:760px;font:600 90px Inter">0</div><div id="m" style="position:absolute;left:760px;top:760px;font:600 90px Inter">0</div>
<svg width="500" height="500" viewBox="0 0 500 500"><circle id="c" cx="250" cy="250" r="200" fill="none" stroke="#C3F53C" stroke-width="10"/></svg>
<script>
const { createTimeline, splitText, stagger, svg } = anime;
const { chars } = splitText("#t", { chars: { wrap: "clip" } });
const tl = createTimeline({ autoplay: false, defaults: { ease: "outExpo" } });
tl.add(chars, { y: ["110%", "0%"], duration: 600, delay: stagger(40) }, 0)
  .add(".bar", { scaleX: [0, 1], duration: 800, ease: "inOutExpo" }, 300)
  .add(svg.createDrawable("#c"), { draw: ["0 0", "0 1"], duration: 1200 }, 200);
const n = { v: 0 };
tl.add(n, { v: 1250, duration: 1200, ease: "outQuad", modifier: anime.utils.round(0),
  onUpdate: () => { document.querySelector("#n").textContent = n.v.toLocaleString("fr-FR"); } }, 0);
tl.add("#m", { innerHTML: [0, 980], duration: 1200, modifier: anime.utils.round(0) }, 0);
window.enfait = { duration: 2, seek(t) { tl.seek(t * 1000); } };
</script></body></html>"##;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("dossier"))?,
    );
    let cache = PathBuf::from(std::env::var("HOME")?).join(".cache/enfait");
    let chrome_dir =
        std::env::var("ENFAIT_CHROME_DIR").map_or_else(|_| cache.join("chromium"), PathBuf::from);
    let chrome = ensure_browser(&chrome_dir).await?;
    println!("chrome : {}", chrome.display());
    library::install(&root)?;
    let path = root.join("smoke.html");
    let html = match std::env::args().nth(2) {
        Some(file) => std::fs::read_to_string(file)?,
        None => HTML.to_owned(),
    };
    std::fs::write(&path, composition::prepare(&html))?;
    let renderer = Renderer::launch(
        &chrome,
        &cache.join("profile-smoke"),
        match std::env::var("ENFAIT_GPU").as_deref() {
            Ok("nvidia") => Gpu::Vulkan {
                nvidia_offload: true,
            },
            Ok("vulkan") => Gpu::Vulkan {
                nvidia_offload: false,
            },
            _ => Gpu::Software,
        },
    )
    .await?;
    let check = CheckJob {
        html: &path,
        width: 1920,
        height: 1080,
        expected_duration: 2.0,
        sample_times: vec![0.0, 0.4, 0.9, 1.9, 0.4],
        sample_width: 960,
        scenes: None,
        out_dir: &root.join("check"),
    }
    .run(&renderer)
    .await?;
    println!(
        "blocking: {:?}\nwarnings: {:?}",
        check.blocking(),
        check.warnings()
    );
    let start = std::time::Instant::now();
    let out = FrameJob {
        html: &path,
        width: 1920,
        height: 1080,
        times: (0..150).map(|i| f64::from(i % 60) / 30.0).collect(),
        scenes: None,
        extra_key: String::new(),
        out_dir: &root.join("frames"),
    }
    .run(&renderer)
    .await?;
    println!(
        "{} images en {:?}, erreurs {:?}",
        out.frames,
        start.elapsed(),
        out.errors
    );
    let stage = renderer.open(&path, 1920, 1080).await?;
    let (mut seek, mut cap) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    let mut bytes = 0;
    for i in 0..60 {
        let a = std::time::Instant::now();
        stage.seek(f64::from(i) / 30.0, None).await?;
        let b = std::time::Instant::now();
        bytes += stage.capture_jpeg().await?.len();
        seek += b - a;
        cap += b.elapsed();
    }
    println!(
        "seek {:?}/image, capture {:?}/image, {} Ko/image",
        seek / 60,
        cap / 60,
        bytes / 60 / 1000
    );
    stage.close().await;
    renderer.close().await;
    Ok(())
}
