//! Long projects (a two-hour take gives a thousand blocks and more): lists
//! that only lay out the rows in view, timelines painted in one pass, and
//! clip checks that do not touch the disk on every frame.

use super::Shell;
use crate::theme::{Token, color};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, ListAlignment, ListState, MouseButton, MouseDownEvent,
    Pixels, Window, canvas, div, fill, list, point, px, size,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Rows laid out beyond the viewport, so that scrolling does not show holes.
const OVERDRAW: f32 = 400.0;
/// How long a listing of the clips folder is trusted.
const CLIPS_TTL: Duration = Duration::from_millis(500);

/// Scroll states of the long lists, one per screen.
pub(super) struct Lists {
    pub blocks: ListState,
    pub sentences: ListState,
    pub retouch: ListState,
    pub montage: ListState,
    /// Block under the playhead, for the rows drawn this frame.
    pub playing: Option<enfait_core::BlockId>,
    /// Sentence under the playhead on the cut screen.
    pub cut_current: Option<usize>,
}

impl Default for Lists {
    fn default() -> Self {
        let new = || ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        Self {
            blocks: new(),
            sentences: new(),
            retouch: new(),
            montage: new(),
            playing: None,
            cut_current: None,
        }
    }
}

/// A list of `count` rows of which only those in view are built, by
/// `row(shell, index, window, cx)`. The scroll position survives as long
/// as the number of rows does not change.
pub(super) fn lazy_list(
    state: &ListState,
    count: usize,
    row: impl Fn(&mut Shell, usize, &mut Window, &mut Context<Shell>) -> AnyElement + 'static,
    cx: &Context<Shell>,
) -> gpui_kit::List {
    if state.item_count() != count {
        state.reset(count);
    }
    list(state.clone(), cx.processor(row))
}

/// Files present in `studio/clips`, listed again at most every
/// [`CLIPS_TTL`]: the state of each block is asked for each row drawn.
#[derive(Default)]
pub(super) struct ClipIndex {
    listed: Option<(Instant, PathBuf)>,
    names: HashSet<OsString>,
}

impl ClipIndex {
    /// Whether `path` is a file, from the cached listing of its folder.
    pub fn exists(&mut self, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return path.is_file();
        };
        let fresh = self
            .listed
            .as_ref()
            .is_some_and(|(at, listed)| listed == dir && at.elapsed() < CLIPS_TTL);
        if !fresh {
            self.names = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                        .map(|e| e.file_name())
                        .collect()
                })
                .unwrap_or_default();
            self.listed = Some((Instant::now(), dir.to_path_buf()));
        }
        self.names.contains(name)
    }

    /// The engine wrote or removed a clip: list again on the next check.
    pub fn invalidate(&mut self) {
        self.listed = None;
    }
}

/// One mark of a painted timeline: `start` and `width` as shares of the
/// whole (0 to 1).
#[derive(Clone, Copy)]
pub(super) struct Mark {
    pub start: f32,
    pub width: f32,
    pub tone: Token,
}

/// Timeline of `marks` painted in a single element (thousands of blocks
/// would otherwise be as many laid-out elements). A click gives the share
/// of the width it landed on.
pub(super) fn painted_track(
    marks: Vec<Mark>,
    opacity: f32,
    gap: f32,
    on_click: impl Fn(f32, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::default();
    let painted = bounds.clone();
    div()
        .size_full()
        .child(
            canvas(
                move |b, _, _| {
                    *painted.borrow_mut() = Some(b);
                },
                move |b, (), window, _| {
                    let w = f32::from(b.size.width);
                    // Marks thinner than a pixel merge with their neighbour of the
                    // same tone: as many quads as visible marks.
                    let mut last: Option<(f32, f32, Token)> = None;
                    let mut quads = Vec::new();
                    for m in &marks {
                        let x0 = m.start * w;
                        let x1 = (m.start + m.width) * w;
                        match &mut last {
                            Some((_, end, tone)) if *tone == m.tone && x0 - *end < 1.0 => {
                                *end = x1;
                            }
                            _ => {
                                if let Some(q) = last.take() {
                                    quads.push(q);
                                }
                                last = Some((x0, x1, m.tone));
                            }
                        }
                    }
                    quads.extend(last);
                    for (x0, x1, tone) in quads {
                        let width = (x1 - x0 - gap).max(1.0);
                        let quad = Bounds::new(
                            point(b.origin.x + px(x0), b.origin.y),
                            size(px(width), b.size.height),
                        );
                        window.paint_quad(fill(quad, color(tone).opacity(opacity)));
                    }
                },
            )
            .size_full(),
        )
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
            let Some(b) = *bounds.borrow() else { return };
            let w = f32::from(b.size.width);
            if w <= 0.0 {
                return;
            }
            let share = (f32::from(e.position.x - b.origin.x) / w).clamp(0.0, 1.0);
            on_click(share, window, cx);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_index_sees_new_files_once_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("a.mp4");
        let mut index = ClipIndex::default();
        assert!(!index.exists(&clip));
        std::fs::write(&clip, b"x").unwrap();
        // Cached listing: not seen yet.
        assert!(!index.exists(&clip));
        index.invalidate();
        assert!(index.exists(&clip));
        assert!(!index.exists(&dir.path().join("b.mp4")));
    }
}
