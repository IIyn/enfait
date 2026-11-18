//! enfait: local video editing and AI-assisted motion design.

// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cut;
mod fonts;
mod jobs;
mod player;
mod prefs;
mod prompter;
mod session;
mod shell;
mod storyboard;
mod theme;
mod tray;
mod worker;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Global, KeyBinding, Subscription,
    TitlebarOptions, WindowBounds, WindowOptions, actions, px, size,
};
use tracing_subscriber::EnvFilter;

rust_i18n::i18n!("locales", fallback = "en");

/// Translated text as a `SharedString`, ready for gpui: `tr!("key")`,
/// `tr!("key", name = value)`.
#[macro_export]
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).into_owned())
    };
}

actions!(enfait, [Quit, ShowWindow]);

/// A key binding with the platform's main modifier: `cmd-…` on macOS,
/// `ctrl-…` elsewhere.
#[macro_export]
macro_rules! primary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

/// A shortcut with the platform's main modifier: `⌘O` on macOS, `Ctrl+O`
/// elsewhere.
pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// Media to open at startup: `enfait <file>`.
fn initial_media() -> Option<std::path::PathBuf> {
    std::env::args_os().nth(1).map(Into::into)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // enfait decides itself when to quit (see `on_window_closed`): the
    // last window closing keeps it in the tray when one exists.
    let app = gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_quit_mode(gpui_kit::QuitMode::Explicit);
    // macOS: clicking the Dock icon while running in the background.
    app.on_reopen(show_main_window);
    app.run(|cx: &mut App| {
        if let Err(err) = start(cx) {
            tracing::error!("cannot start: {err:#}");
            cx.quit();
        }
    });
}

/// The main window and its content. The `Shell` outlives the window when
/// enfait keeps running in the system tray: reopening shows it again with
/// its state (jobs, player, magic mode) intact.
#[derive(Default)]
struct MainWindow {
    shell: Option<Entity<shell::Shell>>,
    window: Option<AnyWindowHandle>,
    _appearance: Option<Subscription>,
}

impl Global for MainWindow {}

fn start(cx: &mut App) -> anyhow::Result<()> {
    gpui_kit::init(cx);
    fonts::register(cx)?;
    prefs::init(cx)?;

    cx.bind_keys([KeyBinding::new(crate::primary!("q"), Quit, None)]);
    cx.bind_keys(shell::key_bindings());
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    cx.on_action(|_: &ShowWindow, cx: &mut App| show_main_window(cx));
    cx.set_global(MainWindow::default());
    cx.on_window_closed(|cx: &mut App, id| {
        let main = cx.global::<MainWindow>().window;
        if main.is_some_and(|w| w.window_id() == id) {
            cx.global_mut::<MainWindow>().window = None;
            cx.global_mut::<MainWindow>()._appearance = None;
            if !tray::keeps_running(cx) {
                cx.quit();
            }
        } else if cx.windows().is_empty() && !tray::keeps_running(cx) {
            cx.quit();
        }
    })
    .detach();

    tray::start(cx);
    show_main_window(cx);
    Ok(())
}

/// Shows the main window: brings it forward if it is open, otherwise
/// opens it (with the existing `Shell` when enfait was in the tray).
pub fn show_main_window(cx: &mut App) {
    if let Some(handle) = cx.global::<MainWindow>().window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let existing = cx.global::<MainWindow>().shell.clone();
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    // `ENFAIT_FULLSCREEN=1` opens the window fullscreen (screenshots under a
    // tiling window manager).
    let fullscreen = std::env::var_os("ENFAIT_FULLSCREEN").is_some();
    let options = WindowOptions {
        window_bounds: Some(if fullscreen {
            WindowBounds::Fullscreen(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        titlebar: Some(TitlebarOptions {
            title: Some("enfait".into()),
            ..Default::default()
        }),
        app_id: Some("enfait".to_owned()),
        ..Default::default()
    };
    let mut appearance = None;
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        if fullscreen && !window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        // The "System" theme follows the desktop appearance.
        prefs::follow_appearance(window.appearance(), cx);
        appearance = Some(window.observe_window_appearance(|window, cx| {
            prefs::follow_appearance(window.appearance(), cx);
        }));
        match existing {
            Some(shell) => {
                shell.update(cx, |shell, cx| shell.attach_window(window, cx));
                shell
            }
            None => cx.new(|cx| shell::Shell::new(window, cx, initial_media())),
        }
    });
    match opened {
        Ok((handle, shell)) => {
            let main = cx.global_mut::<MainWindow>();
            main.window = Some(handle);
            main.shell = Some(shell);
            main._appearance = appearance;
            cx.activate(true);
        }
        Err(e) => {
            tracing::error!("cannot open the window: {e:#}");
            if !tray::keeps_running(cx) {
                cx.quit();
            }
        }
    }
}

/// The main window's `Shell`, if it exists (open or in the tray).
pub fn main_shell(cx: &App) -> Option<Entity<shell::Shell>> {
    cx.global::<MainWindow>().shell.clone()
}
