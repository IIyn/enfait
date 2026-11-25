//! Library files in the text fields: `@` followed by a few letters offers
//! the matching files (as in a chat with Claude Code), and files dropped
//! on a block or on the style are added to the library and named there.

use super::{Shell, mono};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_core::{AssetKind, BlockId, Edit};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, Entity, PathPromptOptions, Pixels, SharedString, Stateful, Window,
    anchored, deferred, div, px,
};
use std::path::PathBuf;

/// Suggestions shown at most.
const MAX_SUGGESTIONS: usize = 6;

/// Text field where `@` was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    /// Description of the selected block.
    Description,
    /// Rules of the global style.
    Style,
    /// Description and wishes for the YouTube thumbnail.
    Thumbnail,
}

/// A text field for the models where library files are named with `@`:
/// the storyboard description, the thumbnail wishes.
pub(super) struct PromptField<'a> {
    pub field: Field,
    pub area: &'a Entity<TextareaState>,
    /// Its current text (for the files named).
    pub text: &'a str,
    pub height: Pixels,
    /// Width of the list of files floating under the field.
    pub suggestions_width: Pixels,
    pub hint: SharedString,
}

/// The "+" button adding files to the library and naming them in a field.
pub(super) fn add_files_button(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(3.))
        .border_1()
        .border_color(color(tokens::LINE_STRONG))
        .text_size(px(15.))
        .text_color(color(tokens::FG))
        .cursor_pointer()
        .hover(|d| d.bg(color(tokens::HOVER)))
        .child("+")
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tr!("board.add_files")).build(window, cx)
        })
}

/// A mention being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mention {
    pub field: Field,
    /// Byte offset of the `@`.
    start: usize,
    /// Byte offset of the cursor.
    end: usize,
    /// Letters typed after the `@`.
    query: String,
    /// File chosen: inserted at the next render (it needs the window).
    pub pending: Option<String>,
}

/// The mention ending at `cursor`, if one is being typed: `@` at the
/// start of a word, followed by letters without spaces.
fn typed_mention(text: &str, cursor: usize) -> Option<(usize, String)> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let query = before.get(at.saturating_add(1)..)?;
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    let previous = before.get(..at).and_then(|b| b.chars().next_back());
    if previous.is_some_and(|c| !c.is_whitespace() && !"([{\"'«“".contains(c)) {
        return None;
    }
    Some((at, query.to_owned()))
}

/// Frame of the list of files to mention.
fn suggestion_box() -> Div {
    div()
        .flex()
        .flex_col()
        .py(px(4.))
        .rounded(px(3.))
        .border_1()
        .border_color(color(tokens::LINE_STRONG))
        .bg(color(tokens::RAISED))
        .text_size(px(12.))
}

fn kind_name(kind: AssetKind) -> SharedString {
    super::side::asset_kind_name(kind)
}

impl Shell {
    /// Follows the text of a field: opens, narrows or closes the
    /// suggestions.
    pub(super) fn update_mention(
        &mut self,
        field: Field,
        text: &str,
        cursor: usize,
        cx: &mut Context<Self>,
    ) {
        // With an empty library, the description still opens the list: it
        // says how to add files.
        let found = typed_mention(text, cursor)
            .filter(|_| field != Field::Style || !self.session.look.assets.is_empty());
        let next = found.map(|(start, query)| Mention {
            field,
            start,
            end: cursor,
            query,
            pending: None,
        });
        let concerned = next.is_some() || self.mention.as_ref().is_some_and(|m| m.field == field);
        if concerned && self.mention != next {
            self.mention = next;
            cx.notify();
        }
    }

    /// Files matching the mention being typed in `field`.
    fn mention_matches(&self, field: Field) -> Vec<(String, AssetKind, String)> {
        let Some(mention) = self.mention.as_ref().filter(|m| m.field == field) else {
            return Vec::new();
        };
        let query = mention.query.to_lowercase();
        self.session
            .look
            .assets
            .iter()
            .filter(|a| {
                query.is_empty()
                    || a.file.to_lowercase().contains(&query)
                    || a.name.to_lowercase().contains(&query)
            })
            .take(MAX_SUGGESTIONS)
            .map(|a| (a.file.clone(), a.kind, a.name.clone()))
            .collect()
    }

    /// List of the files to mention, under the field.
    pub(super) fn mention_suggestions(&self, field: Field, cx: &mut Context<Self>) -> Option<Div> {
        let matches = self.mention_matches(field);
        if matches.is_empty() {
            let typing = self.mention.as_ref().is_some_and(|m| m.field == field);
            if !typing || !self.session.look.assets.is_empty() {
                return None;
            }
            return Some(
                suggestion_box()
                    .px(px(8.))
                    .text_color(color(tokens::FG2))
                    .child(if field == Field::Thumbnail {
                        tr!("finish.thumb.mention_empty")
                    } else {
                        tr!("board.mention_empty")
                    }),
            );
        }
        let rows = matches
            .into_iter()
            .enumerate()
            .map(|(i, (file, kind, name))| {
                let chosen = file.clone();
                div()
                    .id(("mention", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .h(px(26.))
                    .cursor_pointer()
                    .hover(|d| d.bg(color(tokens::HOVER)))
                    .child(mono(format!("@{file}")).text_color(color(tokens::SIGNAL)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(color(tokens::FG2))
                            .child(SharedString::from(name)),
                    )
                    .child(div().text_color(color(tokens::MUTE)).child(kind_name(kind)))
                    .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                        if let Some(m) = &mut shell.mention {
                            m.pending = Some(chosen.clone());
                        }
                        cx.notify();
                    }))
            });
        Some(suggestion_box().children(rows))
    }

    /// Replaces the typed `@…` with the chosen file.
    pub(super) fn apply_mention(
        &mut self,
        file: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mention) = self.mention.take() else {
            return;
        };
        let text = format!("@{file} ");
        let range = mention.start..mention.end;
        match mention.field {
            Field::Description => self.description.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
                s.focus(window, cx);
            }),
            Field::Style => self.style_rules.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
                s.focus(window, cx);
            }),
            Field::Thumbnail => self.yt_notes.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
                s.focus(window, cx);
            }),
        }
        cx.notify();
    }

    /// A prompt field: the text area (`decorate` applies to its frame), the
    /// files typed after `@` floating under it, an optional `grip` under
    /// it, the files named, and a hint.
    pub(super) fn prompt_field(
        &self,
        prompt: PromptField<'_>,
        decorate: impl FnOnce(Div) -> Div,
        grip: Option<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        let suggestions = self.mention_suggestions(prompt.field, cx);
        let mentioned = self.mentioned_files(prompt.text);
        let width = prompt.suggestions_width;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        decorate(div().relative().h(prompt.height))
                            .child(Textarea::new(prompt.area).h_full())
                            .when_some(suggestions, |d, s| {
                                d.child(
                                    div().absolute().left_0().top_full().mt(px(2.)).child(
                                        deferred(
                                            anchored()
                                                .snap_to_window_with_margin(px(8.))
                                                .child(s.w(width)),
                                        )
                                        .with_priority(1),
                                    ),
                                )
                            }),
                    )
                    .when_some(grip, |d, g| d.child(g)),
            )
            .when(!mentioned.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .text_size(px(11.))
                        .text_color(color(tokens::FG2))
                        .child(tr!("board.mentioned"))
                        .children(mentioned.into_iter().map(|name| {
                            mono(format!("@{name}"))
                                .px(px(4.))
                                .rounded(px(2.))
                                .bg(color(tokens::RAISED))
                                .text_color(color(tokens::SIGNAL))
                        })),
                )
            })
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(color(tokens::MUTE))
                    .child(prompt.hint),
            )
    }

    /// Files added to the library from the thumbnail field (dropped or
    /// picked with +): named at the end of the wishes.
    pub(super) fn add_to_thumbnail(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_youtube(cx);
        let files = self.import_paths(paths, cx);
        if files.is_empty() {
            return;
        }
        let mut notes = self.session.data.youtube.notes.trim_end().to_owned();
        for file in files {
            if !notes.is_empty() {
                notes.push(' ');
            }
            notes.push('@');
            notes.push_str(&file);
        }
        notes.push(' ');
        self.yt_notes.update(cx, |s, cx| {
            s.set_value(notes, window, cx);
            s.focus(window, cx);
        });
        self.commit_youtube(cx);
        cx.notify();
    }

    /// "+" of the thumbnail field.
    pub(super) fn pick_for_thumbnail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mention = None;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(tr!("side.add_to_library")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.add_to_thumbnail(&paths, window, cx);
                });
            }
        })
        .detach();
    }

    /// Library files named in a text (`@file`).
    pub(super) fn mentioned_files(&self, text: &str) -> Vec<String> {
        self.session
            .look
            .mentioned(text)
            .into_iter()
            .map(|a| a.file.clone())
            .collect()
    }

    /// Adds files to the library; returns the names they got there.
    /// Errors are shown in the library section.
    pub(super) fn import_paths(
        &mut self,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut files = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            match self.session.import_asset(path) {
                Ok(file) => files.push(file),
                Err(e) => {
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    errors.push(tr!("side.import_error", name = name, error = e).to_string());
                }
            }
        }
        self.library_notice = (!errors.is_empty()).then(|| errors.join("\n").into());
        if !files.is_empty() {
            self.push_snapshot();
            self.toast(tr!("board.imported", count = files.len()));
        }
        cx.notify();
        files
    }

    /// Files dropped on a block: added to the library and named in its
    /// description.
    pub(super) fn drop_on_block(
        &mut self,
        id: BlockId,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.step != super::STEP_STORYBOARD {
            self.dropped(paths, cx);
            return;
        }
        // What is being typed is kept before the files are named.
        self.commit_description(cx);
        let files = self.import_paths(paths, cx);
        if files.is_empty() {
            return;
        }
        self.select_block(Some(id), false, window, cx);
        let Some(block) = self.board.as_ref().and_then(|b| b.project.block(id)) else {
            return;
        };
        let mut text = block.description.trim_end().to_owned();
        for file in files {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push('@');
            text.push_str(&file);
        }
        self.edit_board(Edit::SetDescription { block: id, text }, window, cx);
    }

    /// "+" of the description: picks files, adds them to the library and
    /// names them in the block, as if they were dropped on it.
    pub(super) fn pick_for_block(
        &mut self,
        id: BlockId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_description(cx);
        self.mention = None;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(tr!("side.add_to_library")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.drop_on_block(id, &paths, window, cx);
                });
            }
        })
        .detach();
    }

    /// Files dropped on the style: added to the library and named in the
    /// style rules.
    pub(super) fn drop_on_style(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let files = self.import_paths(paths, cx);
        if files.is_empty() {
            return;
        }
        self.commit_style_rules(cx);
        let rules = &mut self.session.look.style.rules;
        let trimmed = rules.trim_end().len();
        rules.truncate(trimmed);
        for file in files {
            if !rules.is_empty() {
                rules.push(' ');
            }
            rules.push('@');
            rules.push_str(&file);
        }
        self.session.save_look();
        self.push_snapshot();
        self.reload_look_inputs = true;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::typed_mention;

    #[test]
    fn mention_being_typed() {
        assert_eq!(typed_mention("le @lo", 6), Some((3, "lo".into())));
        assert_eq!(typed_mention("@", 1), Some((0, String::new())));
        assert_eq!(typed_mention("mail@x", 6), None);
        assert_eq!(typed_mention("@logo puis", 10), None);
        assert_eq!(typed_mention("(@é", 4), Some((1, "é".into())));
    }
}
