//! Local models managed by enfait through Ollama: a short catalogue of
//! models known to work, how much video memory each one needs, and the
//! calls that download, load and unload them.
//!
//! enfait only unloads the models it loaded itself (their names are kept
//! in the settings, `local_loaded`): a model the user started by hand stays.

use serde_json::{Value, json};
use std::time::Duration;

/// A model of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogModel {
    /// Ollama tag (`qwen3-vl:8b`).
    pub id: &'static str,
    /// Download size (weights and vision projector), in bytes.
    pub bytes: u64,
    /// Cache of one token of context, in bytes (all layers, keys and
    /// values, 16-bit).
    pub kv_per_token: u64,
    /// Can reason before answering (Ollama's `think`).
    pub thinking: bool,
}

const KIB: u64 = 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// From the smallest to the largest. All of them read images (the
/// critiques of the animations look at rendered frames).
pub const CATALOG: [CatalogModel; 7] = [
    CatalogModel {
        id: "qwen3-vl:2b",
        bytes: 1_889_519_136,
        kv_per_token: 112 * KIB,
        thinking: true,
    },
    CatalogModel {
        id: "gemma3:4b",
        bytes: 3_350_272_259,
        kv_per_token: 28 * KIB,
        thinking: false,
    },
    CatalogModel {
        id: "qwen3-vl:4b",
        bytes: 3_336_618_981,
        kv_per_token: 144 * KIB,
        thinking: true,
    },
    CatalogModel {
        id: "qwen3-vl:8b",
        bytes: 6_190_971_493,
        kv_per_token: 144 * KIB,
        thinking: true,
    },
    CatalogModel {
        id: "gemma3:12b",
        bytes: 8_164_039_587,
        kv_per_token: 70 * KIB,
        thinking: false,
    },
    CatalogModel {
        id: "gemma3:27b",
        bytes: 17_413_409_507,
        kv_per_token: 90 * KIB,
        thinking: false,
    },
    CatalogModel {
        id: "qwen3-vl:30b",
        bytes: 19_595_409_504,
        kv_per_token: 96 * KIB,
        thinking: true,
    },
];

/// Context counted to size a model: a storyboard part or an animation
/// asks for 20,000 to 32,000 tokens (see `local::context_for`).
pub const TYPICAL_CONTEXT: u64 = 24_576;
/// Working buffers of the server, besides the weights and the cache.
const OVERHEAD: u64 = GIB / 2;

impl CatalogModel {
    pub fn find(id: &str) -> Option<Self> {
        CATALOG.iter().copied().find(|m| m.id == id)
    }

    /// Video memory needed to run entirely on the graphics card.
    pub const fn needed(&self) -> u64 {
        self.bytes
            .saturating_add(self.kv_per_token.saturating_mul(TYPICAL_CONTEXT))
            .saturating_add(OVERHEAD)
    }

    pub fn fit(&self, gpu: Option<&Gpu>) -> Fit {
        match gpu {
            None => Fit::Unknown,
            Some(gpu) if self.needed() <= gpu.memory => Fit::Full,
            Some(_) => Fit::Partial,
        }
    }
}

/// How a model runs on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Entirely on the graphics card.
    Full,
    /// Partly on the processor: several times slower.
    Partial,
    /// No graphics card found.
    Unknown,
}

/// The model to suggest: the largest that fits, else the smallest.
pub fn recommended(gpu: Option<&Gpu>) -> CatalogModel {
    CATALOG
        .iter()
        .rev()
        .copied()
        .find(|m| m.fit(gpu) == Fit::Full)
        .unwrap_or(CATALOG[0])
}

/// The graphics card models can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    /// Usable memory, in bytes.
    pub memory: u64,
    /// Memory shared with the system (Apple chips): part of the RAM.
    pub unified: bool,
}

/// The largest graphics card of the machine: NVIDIA (`nvidia-smi`), AMD on
/// Linux (sysfs), or the unified memory of a Mac. Blocking (runs a
/// command): call it off the interface thread.
pub fn detect_gpu() -> Option<Gpu> {
    if cfg!(target_os = "macos") {
        return mac_memory();
    }
    nvidia().or_else(amd)
}

fn nvidia() -> Option<Gpu> {
    let program = enfait_config::process::which("nvidia-smi")?;
    let output = enfait_config::process::command(program)
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_nvidia(&String::from_utf8_lossy(&output.stdout))
}

/// Reads `name, MiB` lines; keeps the largest card.
fn parse_nvidia(stdout: &str) -> Option<Gpu> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, mib) = line.rsplit_once(',')?;
            let mib: u64 = mib.trim().parse().ok()?;
            Some(Gpu {
                name: name.trim().to_owned(),
                memory: mib.saturating_mul(1024 * 1024),
                unified: false,
            })
        })
        .max_by_key(|g| g.memory)
}

fn amd() -> Option<Gpu> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join("device/mem_info_vram_total"));
            text.ok()?.trim().parse::<u64>().ok()
        })
        .max()
        .filter(|bytes| *bytes >= GIB)
        .map(|memory| Gpu {
            name: "AMD".into(),
            memory,
            unified: false,
        })
}

fn mac_memory() -> Option<Gpu> {
    let output = enfait_config::process::command("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let total: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(Gpu {
        name: "Apple".into(),
        // macOS lets the graphics part use about two thirds of the RAM.
        memory: total / 3 * 2,
        unified: true,
    })
}

/// Root of Ollama's own API (`http://localhost:11434`) for an address of
/// its OpenAI endpoint (`…/v1`).
pub fn root(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base).to_owned()
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
}

fn http_error(e: &reqwest::Error) -> String {
    if e.is_connect() || e.is_timeout() {
        "unreachable".to_owned()
    } else {
        e.to_string()
    }
}

async fn post(root: &str, path: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    let response = client()?
        .post(format!("{root}{path}"))
        .timeout(timeout)
        .json(body)
        .send()
        .await
        .map_err(|e| http_error(&e))?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .map_or_else(|| format!("HTTP {}", status.as_u16()), str::to_owned));
    }
    Ok(value)
}

async fn get(root: &str, path: &str) -> Result<Value, String> {
    let response = client()?
        .get(format!("{root}{path}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| http_error(&e))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// Names in a `models` list (`/api/tags`, `/api/ps`).
fn names(value: &Value) -> Vec<String> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("name").or_else(|| m.get("model"))?.as_str())
        .map(str::to_owned)
        .collect()
}

/// Models downloaded in Ollama. `Err("unreachable")`: not running.
pub async fn installed(root: &str) -> Result<Vec<String>, String> {
    get(root, "/api/tags").await.map(|v| names(&v))
}

/// Models loaded in memory right now.
pub async fn running(root: &str) -> Result<Vec<String>, String> {
    get(root, "/api/ps").await.map(|v| names(&v))
}

/// Downloads `model`; `progress(done, total)` in bytes as it goes.
pub async fn pull(
    root: &str,
    model: &str,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let mut response = client()?
        .post(format!("{root}/api/pull"))
        .json(&json!({ "model": model, "stream": true }))
        .send()
        .await
        .map_err(|e| http_error(&e))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let mut lines = Lines::default();
    loop {
        // A stalled download: nothing for two minutes.
        let chunk = tokio::time::timeout(Duration::from_secs(120), response.chunk())
            .await
            .map_err(|_| "download stalled".to_owned())?
            .map_err(|e| http_error(&e))?;
        let Some(chunk) = chunk else { break };
        for line in lines.push(&chunk) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                return Err(error.to_owned());
            }
            let n = |k: &str| value.get(k).and_then(Value::as_u64);
            if let (Some(done), Some(total)) = (n("completed"), n("total")) {
                progress(done, total);
            }
            if value.get("status").and_then(Value::as_str) == Some("success") {
                return Ok(());
            }
        }
    }
    Err("download interrupted".into())
}

/// Loads `model` in memory (an empty request), so that a failure shows
/// now rather than in the middle of a storyboard.
pub async fn load(root: &str, model: &str) -> Result<(), String> {
    post(
        root,
        "/api/generate",
        &json!({ "model": model, "keep_alive": "10m" }),
        Duration::from_secs(600),
    )
    .await
    .map(drop)
}

/// Frees the memory taken by `model`.
pub async fn unload(root: &str, model: &str) -> Result<(), String> {
    post(
        root,
        "/api/generate",
        &json!({ "model": model, "keep_alive": 0 }),
        Duration::from_secs(60),
    )
    .await
    .map(drop)
}

/// Makes `model` the only one of `ours` (models enfait loaded) in memory:
/// unloads the others, downloads it if needed, loads it.
pub async fn switch_to(
    root: &str,
    model: &str,
    ours: &[String],
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    for other in running(root).await? {
        if other != model
            && ours.iter().any(|o| same_model(o, &other))
            && let Err(e) = unload(root, &other).await
        {
            tracing::warn!("cannot unload {other}: {e}");
        }
    }
    if !installed(root).await?.iter().any(|m| same_model(m, model)) {
        pull(root, model, progress).await?;
    }
    load(root, model).await
}

/// `qwen3` and `qwen3:latest` are the same model.
pub fn same_model(a: &str, b: &str) -> bool {
    let full = |m: &str| {
        if m.contains(':') {
            m.to_owned()
        } else {
            format!("{m}:latest")
        }
    };
    full(a) == full(b)
}

/// Splits a stream of bytes into lines (NDJSON).
#[derive(Default)]
pub(crate) struct Lines {
    pending: Vec<u8>,
}

impl Lines {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_owned();
            if !line.is_empty() {
                out.push(line);
            }
        }
        out
    }

    /// What is left after the last newline.
    pub(crate) fn rest(&mut self) -> Option<String> {
        let line = String::from_utf8_lossy(&self.pending).trim().to_owned();
        self.pending.clear();
        (!line.is_empty()).then_some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(gib: u64) -> Gpu {
        Gpu {
            name: "x".into(),
            memory: gib * GIB,
            unified: false,
        }
    }

    #[test]
    fn sizes_models_for_the_card() {
        // 4 GB: nothing fits whole, the smallest is offered.
        assert!(CATALOG.iter().all(|m| m.fit(Some(&gpu(4))) == Fit::Partial));
        assert_eq!(recommended(Some(&gpu(4))).id, "qwen3-vl:2b");
        assert_eq!(recommended(Some(&gpu(8))).id, "qwen3-vl:4b");
        assert_eq!(recommended(Some(&gpu(12))).id, "gemma3:12b");
        assert_eq!(recommended(Some(&gpu(24))).id, "qwen3-vl:30b");
        assert_eq!(recommended(None).id, "qwen3-vl:2b");
        assert_eq!(CATALOG[0].fit(None), Fit::Unknown);
    }

    #[test]
    fn reads_cards_and_lists() {
        let gpu = parse_nvidia("NVIDIA GeForce GTX 1650, 4096\nTesla T4, 15360\n").unwrap();
        assert_eq!(
            (gpu.name.as_str(), gpu.memory),
            ("Tesla T4", 15360 * 1024 * 1024)
        );
        assert!(parse_nvidia("").is_none());
        let ps = json!({ "models": [{ "name": "qwen3-vl:8b" }, { "model": "gemma3:4b" }] });
        assert_eq!(names(&ps), ["qwen3-vl:8b", "gemma3:4b"]);
        assert_eq!(root("http://localhost:11434/v1/"), "http://localhost:11434");
        assert!(same_model("qwen3", "qwen3:latest"));
        assert!(!same_model("qwen3:8b", "qwen3:4b"));
    }

    #[test]
    fn splits_lines() {
        let mut lines = Lines::default();
        assert_eq!(lines.push(b"{\"a\":1}\n{\"b\""), ["{\"a\":1}"]);
        assert_eq!(lines.push(b":2}\n\n{\"c\":3}"), ["{\"b\":2}"]);
        assert_eq!(lines.rest().as_deref(), Some("{\"c\":3}"));
        assert_eq!(lines.rest(), None);
    }
}
