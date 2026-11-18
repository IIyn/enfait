//! Recherche d'images et de vidéos libres de droits.
//!
//! Sans clé : Wikimedia Commons (images, SVG, vidéos, sons) et Openverse
//! (images et sons sous Creative Commons). Avec une clé gratuite : Pexels
//! (`PEXELS_API_KEY`) et Pixabay (`PIXABAY_API_KEY`), riches en vidéos
//! d'illustration, et Freesound (`FREESOUND_API_KEY`) pour les effets
//! sonores. Chaque résultat porte son origine et sa licence.

use crate::AssetError;
use enfait_core::{AssetKind, License, Origin};
use futures::future::join_all;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Fichiers plus lourds ignorés (vidéos de Commons en très haute définition).
const MAX_BYTES: u64 = 200 * 1024 * 1024;
/// Résultats demandés à chaque banque.
const PER_SOURCE: usize = 20;

/// Ce qu'on cherche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Media {
    Image,
    Video,
    /// Musiques et effets sonores.
    Audio,
}

/// Une banque d'images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Commons,
    Openverse,
    Pexels,
    Pixabay,
    Freesound,
}

impl Source {
    pub const ALL: [Self; 5] = [
        Self::Commons,
        Self::Openverse,
        Self::Pexels,
        Self::Pixabay,
        Self::Freesound,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Commons => "Wikimedia Commons",
            Self::Openverse => "Openverse",
            Self::Pexels => "Pexels",
            Self::Pixabay => "Pixabay",
            Self::Freesound => "Freesound",
        }
    }

    /// Variable d'environnement de la clé, pour les banques qui en demandent une.
    /// API key the bank needs, if any.
    pub const fn secret(self) -> Option<enfait_config::Secret> {
        match self {
            Self::Pexels => Some(enfait_config::Secret::Pexels),
            Self::Pixabay => Some(enfait_config::Secret::Pixabay),
            Self::Freesound => Some(enfait_config::Secret::Freesound),
            Self::Commons | Self::Openverse => None,
        }
    }

    /// Environment variable that can hold the key.
    pub const fn key_var(self) -> Option<&'static str> {
        match self.secret() {
            Some(secret) => Some(secret.env_var()),
            None => None,
        }
    }

    /// La banque propose ce type de média.
    pub const fn offers(self, media: Media) -> bool {
        match self {
            Self::Commons => true,
            Self::Openverse => !matches!(media, Media::Video),
            Self::Pexels | Self::Pixabay => !matches!(media, Media::Audio),
            Self::Freesound => matches!(media, Media::Audio),
        }
    }

    fn key(self) -> Option<String> {
        enfait_config::secrets::get(self.secret()?)
    }

    /// La banque est utilisable (clé présente si elle en demande une).
    pub fn available(self) -> bool {
        self.secret().is_none() || self.key().is_some()
    }
}

/// Un résultat de recherche.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub source: Source,
    pub kind: AssetKind,
    /// Vignette (adresse ; vide pour un son sans image).
    pub thumb: String,
    /// Fichier à télécharger.
    pub file: String,
    /// Extension du fichier téléchargé, sans le point.
    pub ext: String,
    pub width: u32,
    pub height: u32,
    /// Durée d'une vidéo ou d'un son, en secondes.
    pub duration: Option<f64>,
    pub origin: Origin,
}

/// Résultats d'une recherche, et les banques qui ont échoué.
#[derive(Debug, Default)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub errors: Vec<String>,
}

/// Client HTTP des banques (identifiant d'enfait, délais raisonnables).
pub fn client() -> Result<reqwest::Client, AssetError> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "enfait/",
            env!("CARGO_PKG_VERSION"),
            " (logiciel libre de montage video)"
        ))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

/// Client for large downloads (models): no overall timeout, only on
/// connecting and between two reads.
pub fn download_client() -> Result<reqwest::Client, AssetError> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("enfait/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()?)
}

/// Cherche `text` dans toutes les banques disponibles, en parallèle ; les
/// résultats sont entrelacés pour que chaque banque soit visible.
pub async fn search(client: &reqwest::Client, text: &str, media: Media) -> Results {
    let text = text.trim();
    let mut results = Results::default();
    if text.is_empty() {
        return results;
    }
    let sources: Vec<Source> = Source::ALL
        .into_iter()
        .filter(|s| s.available() && s.offers(media))
        .collect();
    let replies = join_all(sources.iter().map(|s| search_one(client, *s, text, media))).await;
    let mut lists = Vec::new();
    for (source, reply) in sources.iter().zip(replies) {
        match reply {
            Ok(hits) => lists.push(hits.into_iter()),
            Err(e) => results.errors.push(format!("{} : {e}", source.name())),
        }
    }
    loop {
        let before = results.hits.len();
        for list in &mut lists {
            results.hits.extend(list.next());
        }
        if results.hits.len() == before {
            break;
        }
    }
    results
}

async fn search_one(
    client: &reqwest::Client,
    source: Source,
    text: &str,
    media: Media,
) -> Result<Vec<Hit>, AssetError> {
    let per = PER_SOURCE.to_string();
    let key = source.key().unwrap_or_default();
    let (base, params): (&str, Vec<(&str, String)>) = match (source, media) {
        (Source::Commons, _) => (
            "https://commons.wikimedia.org/w/api.php",
            vec![
                ("action", "query".into()),
                ("format", "json".into()),
                ("generator", "search".into()),
                ("gsrnamespace", "6".into()),
                ("gsrlimit", per.clone()),
                (
                    "gsrsearch",
                    format!(
                        "{text} filetype:{}",
                        match media {
                            Media::Image => "bitmap|drawing",
                            Media::Video => "video",
                            Media::Audio => "audio",
                        }
                    ),
                ),
                ("prop", "imageinfo".into()),
                ("iiprop", "url|extmetadata|mime|size".into()),
                ("iiurlwidth", "400".into()),
                ("iiextmetadatafilter", COMMONS_FIELDS.into()),
            ],
        ),
        (Source::Openverse, _) => (
            if media == Media::Audio {
                "https://api.openverse.org/v1/audio/"
            } else {
                "https://api.openverse.org/v1/images/"
            },
            vec![("q", text.into()), ("page_size", per.clone())],
        ),
        (Source::Freesound, _) => (
            "https://freesound.org/apiv2/search/text/",
            vec![
                ("query", text.into()),
                ("page_size", per.clone()),
                ("token", key.clone()),
                (
                    "fields",
                    "id,name,username,license,previews,duration,url,images".into(),
                ),
            ],
        ),
        (Source::Pexels | Source::Pixabay, Media::Audio) => {
            return Ok(Vec::new());
        }
        (Source::Pexels, Media::Image) => (
            "https://api.pexels.com/v1/search",
            vec![("query", text.into()), ("per_page", per.clone())],
        ),
        (Source::Pexels, Media::Video) => (
            "https://api.pexels.com/videos/search",
            vec![("query", text.into()), ("per_page", per.clone())],
        ),
        (Source::Pixabay, Media::Image) => (
            "https://pixabay.com/api/",
            vec![
                ("key", key.clone()),
                ("q", text.into()),
                ("per_page", per.clone()),
                ("safesearch", "true".into()),
            ],
        ),
        (Source::Pixabay, Media::Video) => (
            "https://pixabay.com/api/videos/",
            vec![
                ("key", key.clone()),
                ("q", text.into()),
                ("per_page", per.clone()),
                ("safesearch", "true".into()),
            ],
        ),
    };
    let url = url::Url::parse_with_params(base, &params)
        .map_err(|e| AssetError::Source(e.to_string()))?;
    let mut request = client.get(url);
    if source == Source::Pexels {
        request = request.header("Authorization", key);
    }
    let json: Value = request.send().await?.error_for_status()?.json().await?;
    Ok(match (source, media) {
        (Source::Commons, _) => parse_commons(&json),
        (Source::Openverse, _) => parse_openverse(&json),
        (Source::Freesound, _) => parse_freesound(&json),
        (Source::Pexels | Source::Pixabay, Media::Audio) => Vec::new(),
        (Source::Pexels, Media::Image) => parse_pexels_photos(&json),
        (Source::Pexels, Media::Video) => parse_pexels_videos(&json),
        (Source::Pixabay, Media::Image) => parse_pixabay_images(&json),
        (Source::Pixabay, Media::Video) => parse_pixabay_videos(&json),
    })
}

const COMMONS_FIELDS: &str = "ObjectName|Artist|LicenseShortName|LicenseUrl";

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn num(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(Value::as_u64)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// Texte d'un fragment HTML (Commons donne l'auteur en HTML).
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extension d'une adresse, en minuscules, sans le point.
fn extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Titre lisible tiré du nom d'un fichier ou d'une adresse.
fn title_from(name: &str) -> String {
    let name = name.trim_end_matches('/');
    let last = name.rsplit('/').next().unwrap_or(name);
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    let stem = stem.strip_prefix("File:").unwrap_or(stem);
    // Pexels : « un-train-qui-passe-1234567 ».
    let stem = stem
        .rsplit_once('-')
        .filter(|(_, id)| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        .map_or(stem, |(s, _)| s);
    stem.replace(['-', '_'], " ").trim().to_owned()
}

/// Auteur raccourci : certaines pages donnent un paragraphe entier.
fn short(text: &str) -> String {
    const MAX: usize = 120;
    if text.chars().count() <= MAX {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX).collect();
    out.push('…');
    out
}

/// Vignette de Commons à 1920 px de large, pour une photo plus large que
/// 2400 px (le fichier d'origine pèse souvent plus de 10 Mo).
fn resized(thumb: &str, ext: &str, width: u32) -> Option<String> {
    if width <= 2400 || !matches!(ext, "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    let url = thumb.split('?').next()?;
    let (dir, name) = url.rsplit_once('/')?;
    let (size, rest) = name.split_once("px-")?;
    if size.is_empty() || !size.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{dir}/1920px-{rest}"))
}

pub fn parse_commons(json: &Value) -> Vec<Hit> {
    let Some(pages) = json
        .get("query")
        .and_then(|q| q.get("pages"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut pages: Vec<&Value> = pages.values().collect();
    pages.sort_by_key(|p| p.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX));
    pages
        .into_iter()
        .filter_map(|page| {
            let info = page.get("imageinfo")?.as_array()?.first()?;
            let file = text(info, "url");
            let ext = extension(&file);
            let kind = AssetKind::from_extension(&ext)?;
            let size = info.get("size").and_then(Value::as_u64).unwrap_or(0);
            if size > MAX_BYTES {
                return None;
            }
            let meta = info.get("extmetadata");
            let field = |k: &str| {
                meta.and_then(|m| m.get(k))
                    .map(|f| strip_html(&text(f, "value")))
                    .unwrap_or_default()
            };
            let licence = field("LicenseShortName");
            let page_title = text(page, "title");
            let title = Some(field("ObjectName"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| title_from(&page_title));
            let thumb = match kind {
                AssetKind::Audio => String::new(),
                _ => Some(text(info, "thumburl"))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| file.clone()),
            };
            let duration = info.get("duration").and_then(Value::as_f64);
            let (mut width, mut height) = (num(info, "width"), num(info, "height"));
            // Grande photo : la version de 1920 px de large suffit.
            let (file, ext) = match resized(&thumb, &ext, width) {
                Some(url) => {
                    height = u32::try_from(u64::from(height) * 1920 / u64::from(width.max(1)))
                        .unwrap_or(height);
                    width = 1920;
                    let ext = extension(&url);
                    (url, ext)
                }
                None => (file, ext),
            };
            Some(Hit {
                source: Source::Commons,
                kind,
                thumb,
                file,
                ext,
                width,
                height,
                duration,
                origin: Origin {
                    source: Source::Commons.name().into(),
                    page: text(info, "descriptionurl"),
                    title,
                    author: short(&field("Artist")),
                    license: License::new(
                        if licence.is_empty() {
                            "licence inconnue".to_owned()
                        } else {
                            licence
                        },
                        Some(field("LicenseUrl")).filter(|u| !u.is_empty()),
                    ),
                    changes: Vec::new(),
                },
            })
        })
        .collect()
}

pub fn parse_openverse(json: &Value) -> Vec<Hit> {
    let Some(results) = json.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let file = text(r, "url");
            let ext = Some(text(r, "filetype"))
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| extension(&file))
                .to_ascii_lowercase();
            let kind = AssetKind::from_extension(&ext)?;
            let code = text(r, "license").to_ascii_lowercase();
            let version = text(r, "license_version");
            let name = match code.as_str() {
                "cc0" => "CC0 1.0".to_owned(),
                "pdm" => "Public Domain Mark 1.0".to_owned(),
                _ => format!("CC {} {version}", code.to_ascii_uppercase())
                    .trim()
                    .to_owned(),
            };
            let thumb = match (text(r, "thumbnail"), kind) {
                (t, _) if !t.is_empty() => t,
                (_, AssetKind::Audio) => String::new(),
                _ => file.clone(),
            };
            // Openverse donne la durée d'un son en millisecondes.
            let duration = r
                .get("duration")
                .and_then(Value::as_f64)
                .map(|ms| ms / 1000.0);
            Some(Hit {
                source: Source::Openverse,
                kind,
                thumb,
                file,
                ext,
                width: num(r, "width"),
                height: num(r, "height"),
                duration,
                origin: Origin {
                    source: format!("Openverse ({})", text(r, "source")),
                    page: text(r, "foreign_landing_url"),
                    title: text(r, "title"),
                    author: text(r, "creator"),
                    license: License::new(
                        name,
                        Some(text(r, "license_url")).filter(|u| !u.is_empty()),
                    ),
                    changes: Vec::new(),
                },
            })
        })
        .collect()
}

/// Nom court d'une licence Creative Commons d'après son adresse
/// (`…/licenses/by-nc/4.0/` → « CC BY-NC 4.0 »).
pub fn license_from_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("publicdomain/zero") {
        return "CC0 1.0".to_owned();
    }
    if lower.contains("publicdomain/mark") {
        return "Public Domain Mark 1.0".to_owned();
    }
    let parts: Vec<&str> = lower.split('/').filter(|p| !p.is_empty()).collect();
    let code = parts
        .iter()
        .position(|p| *p == "licenses")
        .and_then(|i| parts.get(i.saturating_add(1)..i.saturating_add(3)));
    match code {
        Some([code, version]) => format!("CC {} {version}", code.to_ascii_uppercase()),
        _ => format!("licence : {url}"),
    }
}

pub fn parse_freesound(json: &Value) -> Vec<Hit> {
    let Some(results) = json.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let file = text(r.get("previews")?, "preview-hq-mp3");
            if file.is_empty() {
                return None;
            }
            let licence = text(r, "license");
            let thumb = r
                .get("images")
                .map(|i| text(i, "waveform_m"))
                .unwrap_or_default();
            Some(Hit {
                source: Source::Freesound,
                kind: AssetKind::Audio,
                thumb,
                file,
                ext: "mp3".into(),
                width: 0,
                height: 0,
                duration: r.get("duration").and_then(Value::as_f64),
                origin: Origin {
                    source: Source::Freesound.name().into(),
                    page: text(r, "url"),
                    title: text(r, "name"),
                    author: text(r, "username"),
                    license: License::new(
                        license_from_url(&licence),
                        Some(licence).filter(|l| !l.is_empty()),
                    ),
                    changes: Vec::new(),
                },
            })
        })
        .collect()
}

fn pexels_license() -> License {
    License::new(
        "Licence Pexels",
        Some("https://www.pexels.com/license/".into()),
    )
}

pub fn parse_pexels_photos(json: &Value) -> Vec<Hit> {
    let Some(photos) = json.get("photos").and_then(Value::as_array) else {
        return Vec::new();
    };
    photos
        .iter()
        .filter_map(|p| {
            let src = p.get("src")?;
            let file = Some(text(src, "large2x"))
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| text(src, "original"));
            let ext = extension(&file);
            let kind = AssetKind::from_extension(&ext)?;
            let page = text(p, "url");
            let alt = text(p, "alt");
            Some(Hit {
                source: Source::Pexels,
                kind,
                thumb: text(src, "medium"),
                file,
                ext,
                width: num(p, "width"),
                height: num(p, "height"),
                duration: None,
                origin: Origin {
                    source: Source::Pexels.name().into(),
                    title: if alt.is_empty() {
                        title_from(&page)
                    } else {
                        alt
                    },
                    page,
                    author: text(p, "photographer"),
                    license: pexels_license(),
                    changes: Vec::new(),
                },
            })
        })
        .collect()
}

/// Fichier vidéo à garder parmi plusieurs définitions : le plus petit d'au
/// moins 1920 de large, sinon le plus grand.
fn pick_video(files: &[(u32, u32, String)]) -> Option<&(u32, u32, String)> {
    files
        .iter()
        .filter(|f| f.0 >= 1920)
        .min_by_key(|f| f.0)
        .or_else(|| files.iter().max_by_key(|f| f.0))
}

pub fn parse_pexels_videos(json: &Value) -> Vec<Hit> {
    let Some(videos) = json.get("videos").and_then(Value::as_array) else {
        return Vec::new();
    };
    videos
        .iter()
        .filter_map(|v| {
            let files: Vec<(u32, u32, String)> = v
                .get("video_files")?
                .as_array()?
                .iter()
                .filter(|f| text(f, "file_type") == "video/mp4")
                .map(|f| (num(f, "width"), num(f, "height"), text(f, "link")))
                .filter(|f| !f.2.is_empty())
                .collect();
            let (width, height, file) = pick_video(&files)?.clone();
            let page = text(v, "url");
            let author = v.get("user").map(|u| text(u, "name")).unwrap_or_default();
            Some(Hit {
                source: Source::Pexels,
                kind: AssetKind::Video,
                thumb: text(v, "image"),
                file,
                ext: "mp4".into(),
                width,
                height,
                duration: v.get("duration").and_then(Value::as_f64),
                origin: Origin {
                    source: Source::Pexels.name().into(),
                    title: title_from(&page),
                    page,
                    author,
                    license: pexels_license(),
                    changes: Vec::new(),
                },
            })
        })
        .collect()
}

fn pixabay_license() -> License {
    License::new(
        "Licence Pixabay",
        Some("https://pixabay.com/service/license-summary/".into()),
    )
}

fn pixabay_origin(h: &Value) -> Origin {
    Origin {
        source: Source::Pixabay.name().into(),
        page: text(h, "pageURL"),
        title: text(h, "tags"),
        author: text(h, "user"),
        license: pixabay_license(),
        changes: Vec::new(),
    }
}

pub fn parse_pixabay_images(json: &Value) -> Vec<Hit> {
    let Some(hits) = json.get("hits").and_then(Value::as_array) else {
        return Vec::new();
    };
    hits.iter()
        .filter_map(|h| {
            let file = text(h, "largeImageURL");
            let ext = extension(&file);
            let kind = AssetKind::from_extension(&ext)?;
            Some(Hit {
                source: Source::Pixabay,
                kind,
                thumb: text(h, "webformatURL"),
                file,
                ext,
                width: num(h, "imageWidth"),
                height: num(h, "imageHeight"),
                duration: None,
                origin: pixabay_origin(h),
            })
        })
        .collect()
}

pub fn parse_pixabay_videos(json: &Value) -> Vec<Hit> {
    let Some(hits) = json.get("hits").and_then(Value::as_array) else {
        return Vec::new();
    };
    hits.iter()
        .filter_map(|h| {
            let videos = h.get("videos")?;
            let sizes: Vec<(u32, u32, String)> = ["large", "medium", "small", "tiny"]
                .iter()
                .filter_map(|k| videos.get(*k))
                .map(|v| (num(v, "width"), num(v, "height"), text(v, "url")))
                .filter(|v| !v.2.is_empty())
                .collect();
            let (width, height, file) = pick_video(&sizes)?.clone();
            let thumb = ["medium", "small", "large", "tiny"]
                .iter()
                .filter_map(|k| videos.get(*k))
                .map(|v| text(v, "thumbnail"))
                .find(|t| !t.is_empty())
                .unwrap_or_default();
            Some(Hit {
                source: Source::Pixabay,
                kind: AssetKind::Video,
                thumb,
                ext: Some(extension(&file))
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| "mp4".into()),
                file,
                width,
                height,
                duration: h.get("duration").and_then(Value::as_f64),
                origin: pixabay_origin(h),
            })
        })
        .collect()
}

/// GET qui patiente quand le serveur demande de ralentir (429, 503) :
/// jusqu'à trois nouvelles tentatives, en suivant `Retry-After` (borné).
async fn get_patiently(
    client: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, AssetError> {
    let mut wait = Duration::from_secs(2);
    for _ in 0..3 {
        let response = client.get(url).send().await?;
        let status = response.status();
        if status != reqwest::StatusCode::TOO_MANY_REQUESTS
            && status != reqwest::StatusCode::SERVICE_UNAVAILABLE
        {
            return Ok(response.error_for_status()?);
        }
        let asked = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        tokio::time::sleep(asked.unwrap_or(wait).min(Duration::from_secs(10))).await;
        wait = wait.saturating_mul(2);
    }
    Ok(client.get(url).send().await?.error_for_status()?)
}

/// Télécharge `url` dans `path` (fichier temporaire puis renommage).
pub async fn download(client: &reqwest::Client, url: &str, path: &Path) -> Result<(), AssetError> {
    use futures::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let response = get_patiently(client, url).await?;
    if response.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Err(AssetError::Source("fichier trop lourd".into()));
    }
    let partial = path.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut stream = response.bytes_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(AssetError::Source("fichier trop lourd".into()));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}

/// Downloads a large file (models) to `path` without size limit, reporting
/// `(done, total)` in bytes as it goes. Written to `<path>.part`, renamed
/// at the end.
pub async fn download_large(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    progress: &(dyn Fn(u64, Option<u64>) + Sync),
) -> Result<(), AssetError> {
    use futures::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length();
    let partial = path.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut stream = response.bytes_stream();
    let mut done = 0_u64;
    progress(0, total);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        done = done.saturating_add(chunk.len() as u64);
        progress(done, total);
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}

/// Vignettes des résultats, téléchargées dans `dir` (une par adresse, gardée
/// en cache) ; `None` pour celles qui ont échoué.
pub async fn thumbnails(
    client: &reqwest::Client,
    hits: &[Hit],
    dir: &Path,
) -> Vec<Option<PathBuf>> {
    join_all(hits.iter().map(|hit| async move {
        if hit.thumb.is_empty() {
            return None;
        }
        let ext = match extension(&hit.thumb).as_str() {
            e @ ("png" | "jpg" | "jpeg" | "webp" | "gif") => e.to_owned(),
            _ => "jpg".to_owned(),
        };
        let name = format!(
            "{}.{ext}",
            blake3::hash(hit.thumb.as_bytes()).to_hex().get(..32)?
        );
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
        match download(client, &hit.thumb, &path).await {
            Ok(()) => Some(path),
            Err(e) => {
                tracing::debug!("vignette {} : {e}", hit.thumb);
                None
            }
        }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn commons() {
        let hits = parse_commons(&fixture("commons.json"));
        assert_eq!(hits.len(), 2, "le TIFF est écarté");
        let video = &hits[0];
        assert_eq!(video.kind, AssetKind::Video);
        assert_eq!(video.ext, "webm");
        assert_eq!(video.duration, Some(20.053));
        assert_eq!(video.origin.title, "GO Train at level crossing");
        assert_eq!(video.origin.author, "Whpq");
        assert_eq!(video.origin.license.name, "CC BY-SA 3.0");
        assert!(video.origin.license.terms().share_alike);
        assert_eq!(extension(&video.thumb), "jpg", "vignette fixe d'une vidéo");
        let svg = &hits[1];
        assert_eq!(svg.kind, AssetKind::Svg);
        assert_eq!(svg.origin.title, "Montreal public transit icons - Train");
        assert!(!svg.origin.license.terms().attribution);
    }

    #[test]
    fn openverse() {
        let hits = parse_openverse(&fixture("openverse.json"));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].origin.license.name, "CC BY-NC 2.0");
        assert!(hits[0].origin.license.terms().non_commercial);
        assert_eq!(hits[0].origin.source, "Openverse (flickr)");
        assert_eq!(hits[1].origin.license.name, "CC0 1.0");
    }

    #[test]
    fn pexels() {
        let photos = parse_pexels_photos(&fixture("pexels-photos.json"));
        assert_eq!(photos.len(), 1);
        assert_eq!(photos[0].ext, "jpeg");
        assert_eq!(photos[0].origin.author, "Jan Kopřiva");
        let videos = parse_pexels_videos(&fixture("pexels-videos.json"));
        assert_eq!(videos.len(), 1);
        let v = &videos[0];
        assert_eq!((v.width, v.height), (1920, 1080), "le plus petit HD");
        assert_eq!(v.origin.title, "train passing through a station");
        assert_eq!(v.duration, Some(14.0));
    }

    #[test]
    fn pixabay() {
        let images = parse_pixabay_images(&fixture("pixabay-images.json"));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].origin.title, "train, railway, locomotive");
        let videos = parse_pixabay_videos(&fixture("pixabay-videos.json"));
        assert_eq!(videos.len(), 1);
        assert_eq!((videos[0].width, videos[0].height), (1920, 1080));
        assert!(videos[0].thumb.ends_with(".jpg"));
    }

    #[test]
    fn sounds() {
        let hits = parse_freesound(&fixture("freesound.json"));
        assert_eq!(hits.len(), 1, "sans aperçu : écarté");
        let h = &hits[0];
        assert_eq!(h.kind, AssetKind::Audio);
        assert_eq!(h.origin.license.name, "CC BY 4.0");
        assert_eq!(h.duration, Some(1.2));
        assert!(h.thumb.ends_with(".png"));
        let ov = parse_openverse(&fixture("openverse-audio.json"));
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].kind, AssetKind::Audio);
        assert_eq!(ov[0].duration, Some(95.5));
        assert!(ov[0].thumb.is_empty(), "pas de son en guise de vignette");
        assert!(Source::Freesound.offers(Media::Audio));
        assert!(!Source::Pexels.offers(Media::Audio));
    }

    #[test]
    fn helpers() {
        assert_eq!(
            strip_html("<a href=\"//x\">Jean &amp; Lou</a>\n (talk)"),
            "Jean & Lou (talk)"
        );
        assert_eq!(extension("https://x.org/a/B.WebM?utm=1"), "webm");
        assert_eq!(
            title_from("https://www.pexels.com/video/a-train-passing-1234567/"),
            "a train passing"
        );
        assert_eq!(title_from("File:Train_icon.svg"), "Train icon");
        assert_eq!(
            license_from_url("http://creativecommons.org/licenses/by-nc/3.0/"),
            "CC BY-NC 3.0"
        );
        assert_eq!(
            license_from_url("https://creativecommons.org/publicdomain/zero/1.0/"),
            "CC0 1.0"
        );
        let thumb =
            "https://upload.wikimedia.org/wikipedia/commons/thumb/c/cb/A.jpg/500px-A.jpg?utm=x";
        assert_eq!(
            resized(thumb, "jpg", 6000).as_deref(),
            Some("https://upload.wikimedia.org/wikipedia/commons/thumb/c/cb/A.jpg/1920px-A.jpg")
        );
        assert_eq!(resized(thumb, "jpg", 2000), None);
        assert_eq!(resized(thumb, "svg", 6000), None);
        assert_eq!(short(&"a".repeat(200)).chars().count(), 121);
    }
}
