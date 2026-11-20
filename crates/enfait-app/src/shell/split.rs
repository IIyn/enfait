//! Panels resized with the mouse: a handle on the edge of the left column,
//! of the right panel, under the player and under the "on screen" field of
//! the storyboard. Sizes are kept in the settings; a double click on a
//! handle restores the default size.

use super::Shell;
use crate::prefs;
use crate::theme::{color, tokens};
use enfait_config::PanelSizes;
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, CursorStyle, Div, DragMoveEvent, EmptyView, Pixels, Stateful, div, px,
};

/// What a handle resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Splitter {
    Sidebar,
    Inspector,
    Player,
    /// "On screen" field of the storyboard inspector.
    Description,
}

/// Value carried while a handle is dragged.
#[derive(Debug, Clone, Copy)]
pub(super) struct Resize(Splitter);

/// Width the centre keeps whatever the side panels are, in pixels.
const CENTER_MIN: f32 = 420.;

/// Thickness of the grabbing area, centred on the edge.
const GRIP: f32 = 6.;

/// Handle laid over the edge of a panel, which must be `relative()`:
/// left edge for the right panel, right edge for the left column, bottom
/// edge for the player.
pub(super) fn handle(splitter: Splitter, cx: &Context<Shell>) -> Stateful<Div> {
    let grip = div()
        .id(("split", splitter as usize))
        .absolute()
        .hover(|d| d.bg(color(tokens::LINE_STRONG)))
        .on_drag(Resize(splitter), |_, _, _, cx| cx.new(|_| EmptyView))
        .on_click(cx.listener(move |shell, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                shell.reset_panel(splitter, cx);
            }
        }));
    let half = px(GRIP / 2.);
    match splitter {
        Splitter::Sidebar => grip
            .top_0()
            .bottom_0()
            .right(-half)
            .w(px(GRIP))
            .cursor(CursorStyle::ResizeColumn),
        Splitter::Inspector => grip
            .top_0()
            .bottom_0()
            .left(-half)
            .w(px(GRIP))
            .cursor(CursorStyle::ResizeColumn),
        Splitter::Player => grip
            .left_0()
            .right_0()
            .bottom(-half)
            .h(px(GRIP))
            .cursor(CursorStyle::ResizeRow),
        Splitter::Description => grip
            .left_0()
            .right_0()
            .bottom(px(0.))
            .h(px(GRIP * 2.))
            .cursor(CursorStyle::ResizeRow),
    }
}

/// Visible grip under the "on screen" field: drag it to change the height
/// of the field. To lay inside a `relative()` box around the field.
pub(super) fn description_grip(cx: &Context<Shell>) -> Div {
    div()
        .relative()
        .h(px(GRIP * 2.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(32.))
                .h(px(3.))
                .rounded(px(2.))
                .bg(color(tokens::LINE_STRONG)),
        )
        .child(handle(Splitter::Description, cx))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round(v: f32) -> u32 {
    v.round().max(0.0) as u32
}

impl Shell {
    /// Sizes in use: those being dragged, else the saved ones.
    pub(super) fn panels(&self, cx: &gpui_kit::App) -> PanelSizes {
        self.panels
            .unwrap_or_else(|| prefs::settings(cx).panels)
            .clamped()
    }

    /// Follows a handle being dragged; `event.bounds`: the area holding the
    /// columns (under the header, above the timeline).
    pub(super) fn drag_panel(&mut self, event: &DragMoveEvent<Resize>, cx: &mut Context<Self>) {
        let Resize(splitter) = *event.drag(cx);
        let bounds = event.bounds;
        let at = event.event.position;
        let width = f32::from(bounds.size.width);
        let mut panels = self.panels(cx);
        let other = |v: u32| {
            #[allow(clippy::cast_precision_loss)]
            let v = v as f32;
            v
        };
        let room = |taken: u32| (width - other(taken) - CENTER_MIN).max(0.);
        match splitter {
            Splitter::Sidebar => {
                let x = f32::from(at.x - bounds.left()).min(room(panels.inspector));
                panels.sidebar = round(x);
            }
            Splitter::Inspector => {
                let x = f32::from(bounds.right() - at.x).min(room(panels.sidebar));
                panels.inspector = round(x);
            }
            Splitter::Player => {
                let height = f32::from(bounds.size.height).max(1.);
                let y = f32::from(at.y - bounds.top());
                panels.player = round(y / height * 1000.);
            }
            // Followed by the field itself, see `drag_description`.
            Splitter::Description => return,
        }
        let panels = panels.clamped();
        if self.panels != Some(panels) {
            self.panels = Some(panels);
            cx.notify();
        }
    }

    /// Follows the grip of the "on screen" field; `event.bounds`: the field.
    pub(super) fn drag_description(
        &mut self,
        event: &DragMoveEvent<Resize>,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != Splitter::Description {
            return;
        }
        let mut panels = self.panels(cx);
        let y = event.event.position.y - event.bounds.top() - px(GRIP);
        panels.description = round(f32::from(y));
        let panels = panels.clamped();
        if self.panels != Some(panels) {
            self.panels = Some(panels);
            cx.notify();
        }
    }

    /// End of a drag: the sizes are saved.
    pub(super) fn drop_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(panels) = self.panels.take() {
            prefs::update(cx, |s| s.panels = panels);
        }
        cx.notify();
    }

    fn reset_panel(&mut self, splitter: Splitter, cx: &mut Context<Self>) {
        let default = PanelSizes::default();
        let mut panels = self.panels(cx);
        match splitter {
            Splitter::Sidebar => panels.sidebar = default.sidebar,
            Splitter::Inspector => panels.inspector = default.inspector,
            Splitter::Player => panels.player = default.player,
            Splitter::Description => panels.description = default.description,
        }
        self.panels = None;
        prefs::update(cx, |s| s.panels = panels);
        cx.notify();
    }

    /// The player at the top of a screen. `narrow`: the inspector sits
    /// under the list, the player keeps a fixed small height.
    pub(super) fn player_pane(&self, narrow: bool, cx: &Context<Self>) -> Div {
        let pane = div()
            .flex_none()
            .relative()
            .border_b_1()
            .border_color(color(tokens::LINE))
            .child(self.player.clone());
        if narrow {
            pane.h(gpui_kit::relative(0.3))
        } else {
            #[allow(clippy::cast_precision_loss)]
            let fraction = self.panels(cx).player as f32 / 1000.;
            pane.h(gpui_kit::relative(fraction))
                .child(handle(Splitter::Player, cx))
        }
    }

    /// Width of a side panel in pixels.
    pub(super) fn panel_width(&self, splitter: Splitter, cx: &gpui_kit::App) -> Pixels {
        let panels = self.panels(cx);
        #[allow(clippy::cast_precision_loss)]
        let w = match splitter {
            Splitter::Sidebar => panels.sidebar,
            Splitter::Inspector | Splitter::Player => panels.inspector,
            Splitter::Description => panels.description,
        } as f32;
        px(w)
    }
}
