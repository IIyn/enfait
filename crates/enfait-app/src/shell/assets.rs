//! Asset tools in the interface: royalty-free image and video search (left
//! column), origin, licence and cutout of library files, credits, and the
//! shot of B-roll blocks.

use super::{Shell, label, mono};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_assets::search::{Hit, Media, Source};
use enfait_core::license::Terms;
use enfait_core::{Asset, AssetKind, Block, BlockId, BlockKind, Broll, Edit, Flicks};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, ClipboardItem, Context, Div, ElementId, ObjectFit, SharedString, Stateful, Window,
    div, img, px,
};
use std::collections::HashSet;
use std::path::PathBuf;

/// Search running or done in the left column.
pub(super) struct SearchState {
    pub(super) media: Media,
    pub(super) busy: bool,
    /// Results and their downloaded thumbnail.
    pub(super) hits: Vec<(Hit, Option<PathBuf>)>,
    pub(super) errors: Vec<String>,
    /// URLs of the files being added, and already added.
    pub(super) adding: HashSet<String>,
    pub(super) added: HashSet<String>,
    /// Number of the last search: an older answer is ignored.
    generation: u64,
    /// B-roll block waiting for a shot: the next added file becomes it.
    pub(super) broll_target: Option<BlockId>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            media: Media::Image,
            busy: false,
            hits: Vec::new(),
            errors: Vec::new(),
            adding: HashSet::new(),
            added: HashSet::new(),
            generation: 0,
            broll_target: None,
        }
    }
}

fn cache_root() -> PathBuf {
    enfait_config::paths::cache_dir()
}

/// Small runtime for a network call from a gpui background task.
pub(super) fn block_on<F: std::future::Future>(future: F) -> Result<F::Output, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|rt| rt.block_on(future))
        .map_err(|e| e.to_string())
}

fn chip(id: impl Into<ElementId>, text: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(7.))
        .h(px(22.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .cursor_pointer()
        .text_size(px(12.))
        .map(|d| {
            if on {
                d.bg(color(tokens::SIGNAL))
                    .text_color(color(tokens::ON_SIGNAL))
            } else {
                d.border_1()
                    .border_color(color(tokens::LINE_STRONG))
                    .text_color(color(tokens::FG2))
                    .hover(|d| d.bg(color(tokens::HOVER)))
            }
        })
        .child(text.into())
}

fn small(text: impl Into<SharedString>, tone: crate::theme::Token) -> Div {
    div()
        .text_size(px(11.))
        .text_color(color(tone))
        .child(text.into())
}

/// Short summary of what a licence allows.
pub(super) fn terms_summary(terms: &Terms) -> SharedString {
    match (terms.known, terms.attribution) {
        (false, _) => tr!("assets.terms.check"),
        (true, false) => tr!("assets.terms.free"),
        (true, true) => tr!("assets.terms.credit"),
    }
}

/// What to know before publishing, in plain words.
pub(super) fn terms_warnings(terms: &Terms) -> Vec<SharedString> {
    let mut out = Vec::new();
    if !terms.known {
        out.push(tr!("assets.warn.unknown"));
    }
    if terms.non_commercial {
        out.push(tr!("assets.warn.nc"));
    }
    if terms.no_derivatives {
        out.push(tr!("assets.warn.nd"));
    }
    if terms.share_alike {
        out.push(tr!("assets.warn.sa"));
    }
    out
}

/// Licence of a file in a few words, and its colour (warning if it sets a
/// condition to check).
fn licence_tag(asset: &Asset) -> Option<(String, crate::theme::Token)> {
    let origin = asset.origin.as_ref()?;
    let terms = origin.license.terms();
    let tone = if terms.restrictive() {
        tokens::MIXED
    } else {
        tokens::MUTE
    };
    Some((origin.license.name.clone(), tone))
}

/// Files that can be a b-roll shot: videos and still images.
fn broll_candidate(asset: &Asset) -> bool {
    match asset.kind {
        AssetKind::Video => true,
        AssetKind::Image => !asset.file.to_ascii_lowercase().ends_with(".gif"),
        AssetKind::Svg | AssetKind::Font | AssetKind::Audio => false,
    }
}

/// The file can be cut out (raster image, licence allowing it).
fn can_cut_out(asset: &Asset) -> bool {
    let lower = asset.file.to_ascii_lowercase();
    asset.kind == AssetKind::Image
        && [".png", ".jpg", ".jpeg", ".webp"]
            .iter()
            .any(|e| lower.ends_with(e))
        && asset
            .origin
            .as_ref()
            .is_none_or(|o| o.license.terms().allows_changes())
}

impl Shell {
    /// Runs the search of the left column field.
    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        let text = self.search_input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let media = self.search.media;
        self.search.generation = self.search.generation.saturating_add(1);
        let generation = self.search.generation;
        self.search.busy = true;
        self.search.errors.clear();
        cx.notify();
        let thumbs = cache_root().join("search").join("thumbs");
        let work = cx.background_spawn(async move {
            block_on(async move {
                let client = enfait_assets::search::client().map_err(|e| e.to_string())?;
                let results = enfait_assets::search::search(&client, &text, media).await;
                let paths =
                    enfait_assets::search::thumbnails(&client, &results.hits, &thumbs).await;
                Ok::<_, String>((
                    results.hits.into_iter().zip(paths).collect(),
                    results.errors,
                ))
            })
            .and_then(|r| r)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.search.generation != generation {
                    return;
                }
                shell.search.busy = false;
                match result {
                    Ok((hits, errors)) => {
                        shell.search.hits = hits;
                        shell.search.errors = errors;
                    }
                    Err(e) => shell.search.errors = vec![e],
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads a result and adds it to the library, with its origin.
    fn add_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((hit, _)) = self.search.hits.get(index).cloned() else {
            return;
        };
        if !self.search.adding.insert(hit.file.clone()) {
            return;
        }
        cx.notify();
        let hash = blake3::hash(hit.file.as_bytes()).to_hex();
        let tmp = cache_root().join("search").join("downloads").join(format!(
            "{}.{}",
            hash.get(..32).unwrap_or("file"),
            hit.ext
        ));
        let url = hit.file.clone();
        let target = tmp.clone();
        let work = cx.background_spawn(async move {
            block_on(async move {
                let client = enfait_assets::search::client().map_err(|e| e.to_string())?;
                enfait_assets::search::download(&client, &url, &target)
                    .await
                    .map_err(|e| e.to_string())
            })
            .and_then(|r| r)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                shell.search.adding.remove(&hit.file);
                let added = result.and_then(|()| {
                    shell
                        .session
                        .import_found(&tmp, &hit.origin.title, Some(hit.origin.clone()))
                });
                let _ = std::fs::remove_file(&tmp);
                match added {
                    Ok(file) => {
                        shell.search.added.insert(hit.file.clone());
                        shell.library_notice = None;
                        shell.credits_copied = false;
                        shell.offer_broll(&file, hit.kind, cx);
                    }
                    Err(e) => {
                        shell.library_notice = Some(tr!(
                            "assets.add_failed",
                            title = hit.origin.title,
                            error = e
                        ));
                    }
                }
                shell.push_snapshot();
                cx.notify();
            });
        })
        .detach();
    }

    /// If a B-roll block waits for a shot, the added file becomes it.
    fn offer_broll(&mut self, file: &str, kind: AssetKind, cx: &mut Context<Self>) {
        let Some(target) = self.search.broll_target else {
            return;
        };
        if !matches!(kind, AssetKind::Video | AssetKind::Image) {
            return;
        }
        let Some(board) = &mut self.board else { return };
        let waiting = board
            .project
            .block(target)
            .is_some_and(|b| b.kind == BlockKind::Broll && b.broll.is_none());
        if !waiting {
            self.search.broll_target = None;
            return;
        }
        let edit = Edit::SetBroll {
            block: target,
            broll: Some(Broll {
                file: file.to_owned(),
                start: Flicks::ZERO,
            }),
        };
        if board.apply(edit).is_ok() {
            self.search.broll_target = None;
            self.save_board();
            cx.notify();
        }
    }

    /// Cuts out a library image: a new transparent PNG joins the library
    /// (same origin, marked as cut out).
    fn cut_out_asset(&mut self, file: String, cx: &mut Context<Self>) {
        let Some(asset) = self
            .session
            .look
            .assets
            .iter()
            .find(|a| a.file == file)
            .cloned()
        else {
            return;
        };
        if self.cutting.is_some() {
            return;
        }
        self.cutting = Some(file.clone());
        self.library_notice = None;
        cx.notify();
        let source = self.session.library_dir().join(&file);
        let models = enfait_config::paths::cutout_models_dir();
        let out = cache_root()
            .join("search")
            .join(format!("cutout-{}.png", std::process::id()));
        let target = out.clone();
        let work = cx.background_spawn(async move {
            use enfait_assets::cutout::{Cutter, Model, cut_file, ensure_model};
            let model = block_on(async {
                tokio::fs::create_dir_all(&models).await?;
                ensure_model(&models, Model::Precise).await
            })?
            .map_err(|e| e.to_string())?;
            let cutter = Cutter::load(&model, Model::Precise).map_err(|e| e.to_string())?;
            cut_file(&cutter, &source, &target).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                shell.cutting = None;
                let mut origin = asset.origin.clone();
                if let Some(o) = &mut origin {
                    o.changes.push(tr!("assets.cut_out_change").to_string());
                }
                let name = tr!("assets.cut_out_name", name = asset.name).to_string();
                let added = result.and_then(|()| shell.session.import_found(&out, &name, origin));
                let _ = std::fs::remove_file(&out);
                match added {
                    Ok(new_file) => {
                        if let Some(a) = shell
                            .session
                            .look
                            .assets
                            .iter_mut()
                            .find(|a| a.file == new_file)
                        {
                            a.description.clone_from(&asset.description);
                        }
                        shell.session.save_look();
                        shell.selected_asset = None;
                    }
                    Err(e) => {
                        shell.library_notice = Some(tr!("assets.cut_out_failed", error = e));
                    }
                }
                shell.push_snapshot();
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_credits(&mut self, cx: &mut Context<Self>) {
        let text = enfait_core::license::credits(&self.session.look.assets);
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.library_notice = None;
            self.credits_copied = true;
            cx.notify();
        }
    }

    /// Opens the search (left column) to find the shot of a B-roll block.
    fn search_broll_for(&mut self, block: &Block, window: &mut Window, cx: &mut Context<Self>) {
        let query = if block.description.trim().is_empty() {
            self.board
                .as_ref()
                .map(|b| b.text_of(block))
                .unwrap_or_default()
        } else {
            block.description.trim().to_owned()
        };
        self.search.broll_target = Some(block.id);
        self.search.media = Media::Video;
        self.sidebar_open = true;
        self.search_input
            .update(cx, |s, cx| s.set_value(query, window, cx));
        cx.notify();
    }

    /// Royalty-free search, in the Library section.
    pub(super) fn search_section(&self, cx: &mut Context<Self>) -> Div {
        let listeners: &Context<Self> = cx;
        let state = &self.search;
        let medias = [
            (Media::Image, tr!("assets.media.images")),
            (Media::Video, tr!("assets.media.videos")),
            (Media::Audio, tr!("assets.media.sounds")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (media, name))| {
            chip(("search-media", i), name, state.media == media).on_click(listeners.listener(
                move |shell, _: &ClickEvent, _, cx| {
                    shell.search.media = media;
                    shell.run_search(cx);
                    cx.notify();
                },
            ))
        });
        let missing: Vec<&str> = Source::ALL
            .iter()
            .filter(|s| !s.available())
            .map(|s| s.name())
            .collect();
        let status = if state.busy {
            Some((tr!("assets.searching"), tokens::FG2))
        } else if !state.hits.is_empty() {
            Some((
                tr!("assets.results", count = state.hits.len()),
                tokens::MUTE,
            ))
        } else {
            None
        };
        let cards = state.hits.iter().enumerate().map(|(i, (hit, thumb))| {
            let adding = state.adding.contains(&hit.file);
            let added = state.added.contains(&hit.file);
            let terms = hit.origin.license.terms();
            let warn = terms.restrictive();
            let picture = match thumb {
                Some(path) => div().h(px(70.)).overflow_hidden().child(
                    img(path.clone())
                        .w_full()
                        .h(px(70.))
                        .object_fit(ObjectFit::Cover),
                ),
                None => div().h(px(70.)).bg(color(tokens::RAISED)),
            };
            let corner = if adding {
                Some(tr!("assets.adding"))
            } else if added {
                Some(tr!("assets.added"))
            } else if hit.kind == AssetKind::Video {
                Some(tr!("assets.video"))
            } else {
                None
            };
            div()
                .id(("hit", i))
                .w(px(119.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(3.))
                .rounded(px(3.))
                .cursor_pointer()
                .border_1()
                .border_color(color(if added { tokens::SIGNAL } else { tokens::LINE }))
                .hover(|d| d.bg(color(tokens::HOVER)))
                .child(picture.relative().when_some(corner, |d, text| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .left(px(2.))
                            .px(px(4.))
                            .rounded(px(2.))
                            .bg(color(tokens::BG))
                            .text_size(px(10.))
                            .text_color(color(tokens::FG))
                            .child(text),
                    )
                }))
                .child(
                    div()
                        .h(px(16.))
                        .overflow_hidden()
                        .text_size(px(11.))
                        .text_color(color(tokens::FG))
                        .child(SharedString::from(hit.origin.title.clone())),
                )
                .child(
                    div()
                        .h(px(14.))
                        .overflow_hidden()
                        .text_size(px(10.))
                        .text_color(color(if warn { tokens::MIXED } else { tokens::MUTE }))
                        .child(SharedString::from(hit.origin.license.name.clone())),
                )
                .when(!adding && !added, |d| {
                    d.on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                        shell.add_hit(i, cx);
                    }))
                })
        });
        let has_hits = !state.hits.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(4.))
            .child(label(tr!("assets.label.search")))
            .child(Input::new(&self.search_input))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(medias)
                    .when_some(status, |d, (text, tone)| {
                        d.child(small(text, tone).ml(px(4.)))
                    }),
            )
            .when(state.broll_target.is_some(), |d| {
                d.child(small(tr!("assets.broll_target"), tokens::FG2))
            })
            .children(state.errors.iter().map(|e| small(e.clone(), tokens::ERROR)))
            .when(has_hits, |d| {
                d.child(small(tr!("assets.click_to_add"), tokens::MUTE))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(cards))
                    .child(chip("clear-search", tr!("assets.clear"), false).on_click(
                        listeners.listener(|shell, _: &ClickEvent, _, cx| {
                            shell.search.hits.clear();
                            shell.search.errors.clear();
                            cx.notify();
                        }),
                    ))
            })
            .when(!missing.is_empty() && !has_hits, |d| {
                d.child(
                    small(
                        tr!("assets.missing_keys", banks = missing.join(", ")),
                        tokens::MUTE,
                    )
                    .id("missing-keys")
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .on_click(listeners.listener(
                        |shell, _: &ClickEvent, _, cx| {
                            shell.open_settings(cx);
                        },
                    )),
                )
            })
    }

    /// Licence tag of a library row.
    pub(super) fn asset_licence(asset: &Asset) -> Option<Div> {
        let (text, tone) = licence_tag(asset)?;
        Some(small(text, tone))
    }

    /// Origin, licence and actions of the file chosen in the library.
    pub(super) fn asset_details(&self, cx: &mut Context<Self>) -> Option<Div> {
        let file = self.selected_asset.clone()?;
        let asset = self.session.look.assets.iter().find(|a| a.file == file)?;
        let listeners: &Context<Self> = cx;
        let mut details = div().flex().flex_col().gap(px(4.));
        match &asset.origin {
            Some(origin) => {
                let terms = origin.license.terms();
                let mut line = origin.source.clone();
                if !origin.author.is_empty() {
                    line.push_str(" · ");
                    line.push_str(&origin.author);
                }
                details = details
                    .child(small(line, tokens::FG2))
                    .child(small(
                        tr!(
                            "assets.licence_line",
                            licence = origin.license.name,
                            summary = terms_summary(&terms)
                        ),
                        tokens::FG2,
                    ))
                    .children(
                        terms_warnings(&terms)
                            .into_iter()
                            .map(|w| small(tr!("assets.warning", text = w), tokens::MIXED)),
                    );
                if !origin.changes.is_empty() {
                    details = details.child(small(
                        tr!("assets.modified", changes = origin.changes.join(", ")),
                        tokens::MUTE,
                    ));
                }
                let page = origin.page.clone();
                if !page.is_empty() {
                    details =
                        details.child(chip("open-page", tr!("assets.file_page"), false).on_click(
                            listeners.listener(move |_, _: &ClickEvent, _, cx| cx.open_url(&page)),
                        ));
                }
            }
            None => {
                details = details.child(small(tr!("assets.personal"), tokens::MUTE));
            }
        }
        if can_cut_out(asset) {
            let busy = self.cutting.is_some();
            let text = if self.cutting.as_deref() == Some(file.as_str()) {
                tr!("assets.cutting")
            } else {
                tr!("assets.cut_out")
            };
            let target = file.clone();
            details = details.child(chip("cut-out", text, false).when(!busy, |d| {
                d.on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                    shell.cut_out_asset(target.clone(), cx);
                }))
            }));
        }
        Some(details)
    }

    /// Credits to publish with the video.
    pub(super) fn credits_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let found: Vec<&Asset> = self
            .session
            .look
            .assets
            .iter()
            .filter(|a| a.origin.is_some())
            .collect();
        if found.is_empty() {
            return None;
        }
        let to_credit = found
            .iter()
            .filter(|a| {
                a.origin
                    .as_ref()
                    .is_some_and(|o| o.license.terms().attribution)
            })
            .count();
        let warnings = found
            .iter()
            .filter(|a| {
                a.origin
                    .as_ref()
                    .is_some_and(|o| o.license.terms().restrictive())
            })
            .count();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(small(
                    tr!(
                        "assets.found_online",
                        count = found.len(),
                        credit = to_credit
                    ),
                    tokens::FG2,
                ))
                .when(warnings > 0, |d| {
                    d.child(small(
                        tr!("assets.restricted", count = warnings),
                        tokens::MIXED,
                    ))
                })
                .child(
                    chip(
                        "copy-credits",
                        if self.credits_copied {
                            tr!("assets.credits_copied")
                        } else {
                            tr!("assets.copy_credits")
                        },
                        self.credits_copied,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| shell.copy_credits(cx))),
                ),
        )
    }

    /// Inspector of a B-roll block: shot chosen in the library, start point
    /// of a video.
    pub(super) fn broll_inspector(&self, block: &Block, cx: &mut Context<Self>) -> Div {
        let id = block.id;
        let state = self.block_state(block);
        let listeners: &Context<Self> = cx;
        let current = block.broll.clone();
        let candidates: Vec<&Asset> = self
            .session
            .look
            .assets
            .iter()
            .filter(|a| broll_candidate(a))
            .collect();
        let chosen = current
            .as_ref()
            .and_then(|b| self.session.look.assets.iter().find(|a| a.file == b.file));
        let choices = candidates.iter().enumerate().map(|(i, asset)| {
            let on = current.as_ref().is_some_and(|b| b.file == asset.file);
            let file = asset.file.clone();
            chip(
                ("broll", i),
                format!(
                    "{} · {}",
                    asset.name,
                    super::side::asset_kind_name(asset.kind)
                ),
                on,
            )
            .when(!on, |d| {
                d.on_click(
                    listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                        shell.edit_board(
                            Edit::SetBroll {
                                block: id,
                                broll: Some(Broll {
                                    file: file.clone(),
                                    start: Flicks::ZERO,
                                }),
                            },
                            window,
                            cx,
                        );
                    }),
                )
            })
        });
        let start_controls = match (&current, chosen) {
            (Some(broll), Some(asset)) if asset.kind == AssetKind::Video => {
                let start = broll.start.as_secs_f64();
                let file = broll.file.clone();
                let shift = move |delta: f64| {
                    let file = file.clone();
                    listeners.listener(move |shell: &mut Self, _: &ClickEvent, window, cx| {
                        let secs = if delta == 0.0 {
                            0.0
                        } else {
                            (start + delta).max(0.0)
                        };
                        // Milliseconds: bounded, no useful loss.
                        #[allow(clippy::cast_possible_truncation)]
                        let millis = (secs * 1000.0).round() as i64;
                        let Ok(start) = Flicks::from_millis(millis) else {
                            return;
                        };
                        shell.edit_board(
                            Edit::SetBroll {
                                block: id,
                                broll: Some(Broll {
                                    file: file.clone(),
                                    start,
                                }),
                            },
                            window,
                            cx,
                        );
                    })
                };
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            mono(tr!("assets.start_at", seconds = format!("{start:.1}")))
                                .text_size(px(12.))
                                .text_color(color(tokens::FG2))
                                .mr(px(4.)),
                        )
                        .child(chip("broll-back", "−1 s", false).on_click(shift(-1.0)))
                        .child(chip("broll-forward", "+1 s", false).on_click(shift(1.0)))
                        .child(
                            chip("broll-zero", tr!("assets.from_start"), false)
                                .on_click(shift(0.0)),
                        ),
                )
            }
            _ => None,
        };
        let warnings: Vec<SharedString> = chosen
            .and_then(|a| a.origin.as_ref())
            .map(|o| terms_warnings(&o.license.terms()))
            .unwrap_or_default();
        let block_for_search = block.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("assets.label.broll")))
            .child(small(state.label().unwrap_or_default(), state.tone()))
            .when(candidates.is_empty(), |d| {
                d.child(small(tr!("assets.no_candidates"), tokens::FG2))
            })
            .child(div().flex().flex_wrap().gap(px(4.)).children(choices))
            .when_some(start_controls, |d, c| d.child(c))
            .children(
                warnings
                    .into_iter()
                    .map(|w| small(tr!("assets.warning", text = w), tokens::MIXED)),
            )
            .child(
                chip("search-broll", tr!("assets.search_shot"), false).on_click(
                    listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                        shell.search_broll_for(&block_for_search, window, cx);
                        shell.run_search(cx);
                    }),
                ),
            )
            .child(small(tr!("assets.broll_note"), tokens::MUTE))
    }

    /// Mixed block: cut the person out so the animation passes behind them.
    pub(super) fn cutout_toggle(&self, block: &Block, cx: &mut Context<Self>) -> Div {
        let id = block.id;
        let on = block.cutout;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                chip("cutout", tr!("assets.cutout_person"), on).on_click(cx.listener(
                    move |shell, _: &ClickEvent, window, cx| {
                        shell.edit_board(
                            Edit::SetCutout {
                                block: id,
                                cutout: !on,
                            },
                            window,
                            cx,
                        );
                    },
                )),
            )
            .child(small(
                if on {
                    tr!("assets.cutout_on")
                } else {
                    tr!("assets.cutout_off")
                },
                tokens::MUTE,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::{License, Origin};

    fn asset(file: &str, kind: AssetKind, licence: Option<&str>) -> Asset {
        Asset {
            file: file.into(),
            kind,
            name: file.into(),
            description: String::new(),
            origin: licence.map(|l| Origin {
                source: "Wikimedia Commons".into(),
                page: String::new(),
                title: file.into(),
                author: String::new(),
                license: License::new(l, None),
                changes: Vec::new(),
            }),
        }
    }

    #[test]
    fn what_can_be_used() {
        assert!(broll_candidate(&asset("a.mp4", AssetKind::Video, None)));
        assert!(broll_candidate(&asset("a.jpg", AssetKind::Image, None)));
        assert!(!broll_candidate(&asset("a.gif", AssetKind::Image, None)));
        assert!(!broll_candidate(&asset("a.svg", AssetKind::Svg, None)));
        assert!(can_cut_out(&asset(
            "a.JPG",
            AssetKind::Image,
            Some("CC BY 4.0")
        )));
        assert!(!can_cut_out(&asset(
            "a.jpg",
            AssetKind::Image,
            Some("CC BY-ND 4.0")
        )));
        assert!(!can_cut_out(&asset("a.svg", AssetKind::Svg, None)));
        let tag = licence_tag(&asset("a.jpg", AssetKind::Image, Some("CC BY-NC 2.0")));
        assert_eq!(tag.map(|t| t.1), Some(tokens::MIXED));
    }
}
