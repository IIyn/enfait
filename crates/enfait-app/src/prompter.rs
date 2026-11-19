//! Prompter: the script in large type, scrolling while filming.
//!
//! Fills the window, light text on black whatever the theme, reading line
//! at the upper third. Space: scroll or stop; ↑ ↓: speed; + −: text size;
//! wheel: forward or back; Home: back to the top; Esc: close.

use crate::fonts;
use crate::theme::{color, tokens};
use crate::tr;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, EventEmitter, FocusHandle, FontWeight, KeyBinding, ScrollEvent, SharedString, Window,
    actions, div, px, relative, rgb,
};
use std::time::Instant;

actions!(
    prompter,
    [Toggle, Faster, Slower, Bigger, Smaller, Top, Close]
);

const KEY_CONTEXT: &str = "Prompter";
/// Text colours, fixed: the prompter is always light on black.
const TEXT: u32 = 0xE6ECE9;
const DIM: u32 = 0x85908C;
/// Speed step, in pixels per second.
const SPEED_STEP: f32 = 10.0;
const MAX_SPEED: f32 = 400.0;

pub fn key_bindings() -> Vec<KeyBinding> {
    let ctx = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("space", Toggle, ctx),
        KeyBinding::new("up", Faster, ctx),
        KeyBinding::new("down", Slower, ctx),
        KeyBinding::new("=", Bigger, ctx),
        KeyBinding::new("+", Bigger, ctx),
        KeyBinding::new("-", Smaller, ctx),
        KeyBinding::new("home", Top, ctx),
        KeyBinding::new("escape", Close, ctx),
    ]
}

/// The prompter asks to be closed.
pub struct Closed;

pub struct Prompter {
    paragraphs: Vec<SharedString>,
    /// Scroll offset, in pixels.
    offset: f32,
    /// Speed, in pixels per second.
    speed: f32,
    size: f32,
    running: bool,
    last_frame: Option<Instant>,
    focus: FocusHandle,
    /// Voice being recorded while reading.
    recording: Option<enfait_media::record::Meter>,
}

impl EventEmitter<Closed> for Prompter {}

impl Prompter {
    pub fn new(paragraphs: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            paragraphs: paragraphs.into_iter().map(Into::into).collect(),
            offset: 0.0,
            speed: 60.0,
            size: 44.0,
            running: false,
            last_frame: None,
            focus,
            recording: None,
        }
    }

    /// The prompter while the voice is recorded: the scrolling starts at
    /// once, a meter shows the level and Escape ends the recording.
    pub fn recording(
        paragraphs: Vec<String>,
        meter: enfait_media::record::Meter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut prompter = Self::new(paragraphs, window, cx);
        prompter.recording = Some(meter);
        prompter.running = true;
        prompter
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.running = !self.running;
        self.last_frame = None;
        cx.notify();
    }

    fn set_speed(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.speed = (self.speed + delta).clamp(0.0, MAX_SPEED);
        cx.notify();
    }

    fn set_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.size = (self.size + delta).clamp(24.0, 96.0);
        cx.notify();
    }

    /// Advances the scroll by the time elapsed since the last frame.
    fn advance(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            // At most 0.1 s per frame, so as not to jump after a pause of the
            // display (hidden window).
            let dt = now.duration_since(last).as_secs_f32().min(0.1);
            self.offset += self.speed * dt;
        }
        self.last_frame = Some(now);
    }
}

impl Render for Prompter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.running {
            self.advance();
            window.request_animation_frame();
        }
        let meter = self.recording.as_ref().map(|m| (m.seconds(), m.level()));
        if meter.is_some() {
            window.request_animation_frame();
        }
        let line = self.size * 1.45;
        let status = tr!(
            "prompter.status",
            state = if self.running {
                tr!("prompter.running")
            } else {
                tr!("prompter.stopped")
            },
            speed = format!("{:.0}", self.speed)
        );
        let empty = self.paragraphs.is_empty();
        div()
            .id("prompter")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|p, _: &Toggle, _, cx| p.toggle(cx)))
            .on_action(cx.listener(|p, _: &Faster, _, cx| p.set_speed(SPEED_STEP, cx)))
            .on_action(cx.listener(|p, _: &Slower, _, cx| p.set_speed(-SPEED_STEP, cx)))
            .on_action(cx.listener(|p, _: &Bigger, _, cx| p.set_size(4.0, cx)))
            .on_action(cx.listener(|p, _: &Smaller, _, cx| p.set_size(-4.0, cx)))
            .on_action(cx.listener(|p, _: &Top, _, cx| {
                p.offset = 0.0;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &Close, _, cx| cx.emit(Closed)))
            .on_scroll(cx.listener(|p, e: &ScrollEvent, window, cx| {
                let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                p.offset = (p.offset - dy).max(0.0);
                cx.notify();
            }))
            .absolute()
            .inset_0()
            .bg(gpui_kit::black())
            .overflow_hidden()
            .child(
                // The text starts on the reading line and moves up.
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(0.33))
                    .mt(px(-self.offset))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(relative(0.8))
                            .max_w(px(1100.))
                            .flex()
                            .flex_col()
                            .gap(px(line))
                            .text_size(px(self.size))
                            .line_height(px(line))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .when(empty, |d| {
                                d.text_color(rgb(DIM)).child(tr!("prompter.empty"))
                            })
                            .children(self.paragraphs.iter().cloned().map(|p| div().child(p))),
                    ),
            )
            .child(
                // Reading line.
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(0.33))
                    .mt(px(-6.))
                    .h(px(2.))
                    .flex()
                    .justify_between()
                    .child(div().w(px(36.)).h_full().bg(color(tokens::SIGNAL)))
                    .child(div().w(px(36.)).h_full().bg(color(tokens::SIGNAL))),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .left(px(16.))
                    .font_family(fonts::MONO)
                    .text_size(px(12.))
                    .text_color(rgb(DIM))
                    .child(status),
            )
            .when_some(meter, |d, (seconds, level)| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let secs = seconds as u64;
                d.child(
                    div()
                        .absolute()
                        .top(px(16.))
                        .left(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .font_family(fonts::MONO)
                        .text_size(px(13.))
                        .text_color(rgb(TEXT))
                        .child(div().size(px(10.)).rounded(px(5.)).bg(color(tokens::ERROR)))
                        .child(format!("REC {:02}:{:02}", secs / 60, secs % 60))
                        .child(
                            div().w(px(120.)).h(px(4.)).bg(rgb(0x2B3230)).child(
                                div()
                                    .h_full()
                                    .w(relative(level.sqrt().clamp(0.0, 1.0)))
                                    .bg(color(if level > 0.9 {
                                        tokens::ERROR
                                    } else {
                                        tokens::SIGNAL
                                    })),
                            ),
                        )
                        .child(
                            div()
                                .text_color(rgb(DIM))
                                .child(tr!("prompter.stop_recording")),
                        ),
                )
            })
    }
}
