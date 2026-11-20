//! Left column: animation style, look (global post-FX, previewed in the
//! player) and the user's library.

use super::{Shell, label, mono};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_core::{AssetKind, FX_PRESETS, PostFx};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, ElementId, Entity, PathPromptOptions, SharedString, Stateful,
    Subscription, Window, div, px,
};

/// A post-FX setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FxField {
    Grain,
    Vignette,
    Contrast,
    Saturation,
    Temperature,
    Aberration,
}

pub(super) const FX_FIELDS: [FxField; 6] = [
    FxField::Contrast,
    FxField::Saturation,
    FxField::Temperature,
    FxField::Grain,
    FxField::Vignette,
    FxField::Aberration,
];

impl FxField {
    fn label(self) -> SharedString {
        match self {
            Self::Grain => tr!("side.fx.grain"),
            Self::Vignette => tr!("side.fx.vignette"),
            Self::Contrast => tr!("side.fx.contrast"),
            Self::Saturation => tr!("side.fx.saturation"),
            Self::Temperature => tr!("side.fx.temperature"),
            Self::Aberration => tr!("side.fx.aberration"),
        }
    }

    /// Minimum value (−1 for settings centred on 0).
    const fn min(self) -> f32 {
        match self {
            Self::Contrast | Self::Saturation | Self::Temperature => -1.0,
            _ => 0.0,
        }
    }

    pub(super) fn get(self, fx: &PostFx) -> f32 {
        match self {
            Self::Grain => fx.grain,
            Self::Vignette => fx.vignette,
            Self::Contrast => fx.contrast,
            Self::Saturation => fx.saturation,
            Self::Temperature => fx.temperature,
            Self::Aberration => fx.aberration,
        }
    }

    fn set(self, fx: &mut PostFx, value: f32) {
        let slot = match self {
            Self::Grain => &mut fx.grain,
            Self::Vignette => &mut fx.vignette,
            Self::Contrast => &mut fx.contrast,
            Self::Saturation => &mut fx.saturation,
            Self::Temperature => &mut fx.temperature,
            Self::Aberration => &mut fx.aberration,
        };
        *slot = value;
        *fx = fx.clamped();
    }
}

/// Post-FX sliders and their subscriptions (release = apply).
pub(super) fn fx_sliders(
    fx: &PostFx,
    cx: &mut Context<Shell>,
) -> (Vec<(FxField, Entity<SliderState>)>, Vec<Subscription>) {
    let mut sliders = Vec::new();
    let mut subscriptions = Vec::new();
    for field in FX_FIELDS {
        let state = cx.new(|_| {
            SliderState::new()
                .min(field.min())
                .max(1.0)
                .step(0.05)
                .default_value(field.get(fx))
        });
        subscriptions.push(cx.subscribe(
            &state,
            move |shell: &mut Shell, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(SliderValue::Single(v) | SliderValue::Range(_, v)) =
                    event
                {
                    field.set(&mut shell.session.look.fx, *v);
                    shell.apply_fx(cx);
                }
            },
        ));
        sliders.push((field, state));
    }
    (sliders, subscriptions)
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

/// Name of a ready-made look, by its position in `FX_PRESETS`.
fn fx_preset_name(index: usize, fallback: &'static str) -> SharedString {
    match index {
        0 => tr!("side.fx_preset.0"),
        1 => tr!("side.fx_preset.1"),
        2 => tr!("side.fx_preset.2"),
        3 => tr!("side.fx_preset.3"),
        4 => tr!("side.fx_preset.4"),
        5 => tr!("side.fx_preset.5"),
        _ => fallback.into(),
    }
}

/// Name and summary of a style preset.
pub(super) fn style_text(preset: &enfait_ai::styles::Preset) -> (SharedString, SharedString) {
    match preset.slug {
        "engraving" => (
            tr!("side.style.engraving.name"),
            tr!("side.style.engraving.summary"),
        ),
        "paper" => (
            tr!("side.style.paper.name"),
            tr!("side.style.paper.summary"),
        ),
        "swiss" => (
            tr!("side.style.swiss.name"),
            tr!("side.style.swiss.summary"),
        ),
        "chalk" => (
            tr!("side.style.chalk.name"),
            tr!("side.style.chalk.summary"),
        ),
        _ => (preset.name.into(), preset.summary.into()),
    }
}

/// Kind of a library file, in the interface.
pub(super) fn asset_kind_name(kind: AssetKind) -> SharedString {
    match kind {
        AssetKind::Image => tr!("side.kind.image"),
        AssetKind::Svg => tr!("side.kind.svg"),
        AssetKind::Video => tr!("side.kind.video"),
        AssetKind::Font => tr!("side.kind.font"),
        AssetKind::Audio => tr!("side.kind.audio"),
    }
}

fn section(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(16.))
        .border_b_1()
        .border_color(color(tokens::LINE))
        .child(label(title))
}

impl Shell {
    /// Saves the look and updates the preview in the player.
    pub(super) fn apply_fx(&mut self, cx: &mut Context<Self>) {
        self.session.save_look();
        let fx = self.session.look.fx;
        self.player.update(cx, |p, cx| p.set_fx(fx, cx));
        cx.notify();
    }

    fn choose_fx_preset(&mut self, fx: PostFx, window: &mut Window, cx: &mut Context<Self>) {
        self.session.look.fx = fx;
        for (field, state) in &self.fx_sliders {
            let v = field.get(&fx);
            state.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        self.apply_fx(cx);
    }

    fn choose_style(&mut self, preset: Option<&'static str>, cx: &mut Context<Self>) {
        self.session.look.style.preset = preset.map(str::to_owned);
        self.session.save_look();
        cx.notify();
    }

    pub(super) fn commit_style_rules(&mut self, cx: &mut Context<Self>) {
        let rules = self.style_rules.read(cx).value().to_string();
        if rules != self.session.look.style.rules {
            self.session.look.style.rules = rules;
            self.session.save_look();
        }
    }

    pub(super) fn commit_asset_note(&mut self, cx: &mut Context<Self>) {
        let note = self.asset_note.read(cx).value().trim().to_owned();
        let Some(file) = &self.selected_asset else {
            return;
        };
        let changed = self
            .session
            .look
            .assets
            .iter_mut()
            .find(|a| &a.file == file)
            .filter(|a| a.description != note)
            .map(|a| a.description = note)
            .is_some();
        if changed {
            self.session.save_look();
            cx.notify();
        }
    }

    fn select_asset(&mut self, file: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_asset_note(cx);
        let note = self
            .session
            .look
            .assets
            .iter()
            .find(|a| a.file == file)
            .map(|a| a.description.clone())
            .unwrap_or_default();
        self.selected_asset = Some(file);
        self.asset_note
            .update(cx, |s, cx| s.set_value(note, window, cx));
        cx.notify();
    }

    fn import_assets(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(tr!("side.add_to_library")),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |shell, cx| {
                    let errors: Vec<String> = paths
                        .iter()
                        .filter_map(|p| {
                            shell.session.import_asset(p).err().map(|e| {
                                let name = p
                                    .file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                                tr!("side.import_error", name = name, error = e).to_string()
                            })
                        })
                        .collect();
                    shell.library_notice = (!errors.is_empty()).then(|| errors.join("\n").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The left column.
    pub(super) fn sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let search = self.search_section(cx);
        let style_suggestions = self.mention_suggestions(super::mention::Field::Style, cx);
        let details = self.asset_details(cx);
        let credits = self.credits_section(cx);
        let look = &self.session.look;
        let current_style = look.style.preset.clone();
        let summary = current_style
            .as_deref()
            .and_then(enfait_ai::styles::find)
            .map_or_else(|| tr!("side.free_summary"), |p| style_text(p).1);
        let fx = look.fx;
        let listeners: &Context<Self> = cx;

        // A session saved with an old slug still shows its style as chosen.
        let current_slug = current_style
            .as_deref()
            .and_then(enfait_ai::styles::find)
            .map(|p| p.slug);
        let styles = std::iter::once((None, tr!("side.free")))
            .chain(
                enfait_ai::styles::PRESETS
                    .iter()
                    .map(|p| (Some(p.slug), style_text(p).0)),
            )
            .enumerate()
            .map(|(i, (slug, name))| {
                chip(("style", i), name, current_slug == slug).on_click(listeners.listener(
                    move |shell, _: &ClickEvent, _, cx| {
                        shell.choose_style(slug, cx);
                    },
                ))
            });
        let looks = FX_PRESETS.iter().enumerate().map(|(i, preset)| {
            let preset_fx = preset.fx;
            chip(
                ("look", i),
                fx_preset_name(i, preset.name),
                (preset_fx.clamped()) == fx.clamped(),
            )
            .on_click(
                listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                    shell.choose_fx_preset(preset_fx, window, cx);
                }),
            )
        });
        let sliders = self.fx_sliders.iter().map(|(field, state)| {
            let value = field.get(&fx);
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(field.label())
                        .child(
                            mono(format!("{:+.0}", value * 100.0))
                                .text_size(px(11.))
                                .text_color(color(tokens::MUTE)),
                        ),
                )
                .child(Slider::new(state))
        });
        let selected = self.selected_asset.clone();
        let assets = look.assets.iter().map(|asset| {
            let file = asset.file.clone();
            let remove = asset.file.clone();
            let on = selected.as_deref() == Some(asset.file.as_str());
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id(SharedString::from(format!("asset-{}", asset.file)))
                        .flex_1()
                        .min_w(px(0.))
                        .px(px(6.))
                        .py(px(3.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .when(on, |d| d.bg(color(tokens::RAISED)))
                        .hover(|d| d.bg(color(tokens::HOVER)))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_color(color(tokens::FG))
                                        .child(SharedString::from(asset.name.clone())),
                                )
                                .child(
                                    div()
                                        .text_color(color(if asset.kind == AssetKind::Font {
                                            tokens::MIXED
                                        } else {
                                            tokens::MUTE
                                        }))
                                        .child(asset_kind_name(asset.kind)),
                                ),
                        )
                        .when_some(Self::asset_licence(asset), |d, tag| d.child(tag))
                        .when(!asset.description.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(color(tokens::MUTE))
                                    .child(SharedString::from(asset.description.clone())),
                            )
                        })
                        .on_click(
                            listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                                shell.select_asset(file.clone(), window, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("remove-{}", asset.file)))
                        .px(px(4.))
                        .cursor_pointer()
                        .text_color(color(tokens::MUTE))
                        .hover(|d| d.text_color(color(tokens::ERROR)))
                        .child("×")
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            if shell.selected_asset.as_deref() == Some(remove.as_str()) {
                                shell.selected_asset = None;
                            }
                            shell.session.remove_asset(&remove);
                            shell.credits_copied = false;
                            shell.push_snapshot();
                            cx.notify();
                        })),
                )
        });
        let empty_library = look.assets.is_empty();
        let rules: Vec<Div> = look
            .memory
            .iter()
            .enumerate()
            .map(|(i, rule)| {
                div()
                    .flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(color(tokens::FG))
                            .child(SharedString::from(rule.clone())),
                    )
                    .child(
                        div()
                            .id(("forget-rule", i))
                            .px(px(4.))
                            .cursor_pointer()
                            .text_color(color(tokens::MUTE))
                            .hover(|d| d.text_color(color(tokens::ERROR)))
                            .child("×")
                            .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                                shell.forget_rule(i, cx);
                            })),
                    )
            })
            .collect();
        let notice = self.library_notice.clone();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(color(tokens::PANEL))
            .border_r_1()
            .border_color(color(tokens::LINE))
            .child(
                div()
                    .id("sidebar")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(
                        section(tr!("side.label.style"))
                            .id("style-section")
                            .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| {
                                style.bg(color(tokens::HOVER))
                            })
                            .on_drop(cx.listener(
                                |shell, paths: &gpui_kit::ExternalPaths, _, cx| {
                                    cx.stop_propagation();
                                    shell.drop_on_style(paths.paths(), cx);
                                },
                            ))
                            .child(div().flex().flex_wrap().gap(px(4.)).children(styles))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(color(tokens::FG2))
                                    .child(summary),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(color(tokens::MUTE))
                                    .child(tr!("side.your_rules")),
                            )
                            .child(
                                div()
                                    .h(px(90.))
                                    .child(Textarea::new(&self.style_rules).h_full()),
                            )
                            .when_some(style_suggestions, |d, s| d.child(s))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(color(tokens::MUTE))
                                    .child(tr!("side.style_drop_hint")),
                            ),
                    )
                    .child(
                        section(tr!("side.label.look"))
                            .child(div().flex().flex_wrap().gap(px(4.)).children(looks))
                            .children(sliders),
                    )
                    .child(
                        section(tr!("side.label.library"))
                            .child(
                                chip("import-assets", tr!("side.add_files"), false).on_click(
                                    cx.listener(|shell, _: &ClickEvent, _, cx| {
                                        shell.import_assets(cx);
                                    }),
                                ),
                            )
                            .child(search)
                            .when_some(notice, |d, n| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(color(tokens::ERROR))
                                        .child(n),
                                )
                            })
                            .when(empty_library, |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(color(tokens::FG2))
                                        .child(tr!("side.library_empty")),
                                )
                            })
                            .children(assets)
                            .when(selected.is_some(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(color(tokens::MUTE))
                                        .child(tr!("side.asset_purpose")),
                                )
                                .child(Input::new(&self.asset_note))
                            })
                            .when_some(details, |d, details| d.child(details)),
                    )
                    .when_some(credits, |d, credits| {
                        d.child(section(tr!("side.label.credits")).child(credits))
                    })
                    .child(
                        section(tr!("side.label.rules"))
                            .when(rules.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(color(tokens::FG2))
                                        .child(tr!("side.rules_empty")),
                                )
                            })
                            .children(rules),
                    ),
            )
    }
}
