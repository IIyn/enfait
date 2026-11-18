//! System tray icon: enfait keeps running (jobs, exports, magic mode) when
//! the window is closed, and the tray menu brings it back.
//!
//! `tray-icon` covers the three platforms: StatusNotifierItem over D-Bus on
//! Linux (`ksni` backend, no GTK), `NSStatusItem` on macOS, the
//! notification area on Windows. Its events are polled from the gpui
//! main thread. Without a tray (e.g. GNOME without the AppIndicator
//! extension), closing the window quits as before.

use crate::{prefs, tr};
use enfait_config::CloseAction;
use gpui_kit::{App, Global};
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/enfait-64.png");

struct Tray {
    _icon: TrayIcon,
    open: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
}

#[derive(Default)]
struct TrayState(Option<Tray>);

impl Global for TrayState {}

/// Creates the tray icon (main thread, after the event loop started) and
/// starts listening to it.
pub fn start(cx: &mut App) {
    let tray = match build() {
        Ok(tray) => Some(tray),
        Err(e) => {
            tracing::warn!("no system tray ({e:#}); closing the window will quit");
            None
        }
    };
    let active = tray.is_some();
    cx.set_global(TrayState(tray));
    if active {
        poll(cx);
    }
}

fn build() -> anyhow::Result<Tray> {
    let image = image::load_from_memory(ICON_PNG)?.into_rgba8();
    let (width, height) = image.dimensions();
    let icon = tray_icon::Icon::from_rgba(image.into_raw(), width, height)?;

    let open = MenuItem::new(rust_i18n::t!("tray.open"), true, None);
    let settings = MenuItem::new(rust_i18n::t!("tray.settings"), true, None);
    let quit = MenuItem::new(rust_i18n::t!("tray.quit"), true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &open,
        &settings,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("enfait")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok(Tray {
        _icon: icon,
        open,
        settings,
        quit,
    })
}

/// A tray icon exists.
pub fn available(cx: &App) -> bool {
    cx.try_global::<TrayState>().is_some_and(|t| t.0.is_some())
}

/// Closing the main window keeps enfait running.
pub fn keeps_running(cx: &App) -> bool {
    cx.try_global::<TrayState>().is_some_and(|t| t.0.is_some())
        && prefs::settings(cx).close_action == CloseAction::Tray
}

/// Translates the menu again after a language change.
pub fn relabel(cx: &mut App) {
    if let Some(TrayState(Some(tray))) = cx.try_global::<TrayState>() {
        tray.open.set_text(tr!("tray.open"));
        tray.settings.set_text(tr!("tray.settings"));
        tray.quit.set_text(tr!("tray.quit"));
    }
}

enum Event {
    Click,
    Menu(tray_icon::menu::MenuId),
}

fn poll(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let mut events = Vec::new();
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    events.push(Event::Click);
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                events.push(Event::Menu(event.id));
            }
            if events.is_empty() {
                continue;
            }
            let _ = cx.update(|cx| {
                for event in events {
                    handle(event, cx);
                }
            });
        }
    })
    .detach();
}

fn handle(event: Event, cx: &mut App) {
    let Some(TrayState(Some(tray))) = cx.try_global::<TrayState>() else {
        return;
    };
    match event {
        Event::Click => crate::show_main_window(cx),
        Event::Menu(id) if &id == tray.open.id() => crate::show_main_window(cx),
        Event::Menu(id) if &id == tray.settings.id() => {
            crate::show_main_window(cx);
            if let Some(shell) = crate::main_shell(cx) {
                shell.update(cx, |shell, cx| shell.open_settings(cx));
            }
        }
        Event::Menu(id) if &id == tray.quit.id() => cx.quit(),
        Event::Menu(_) => {}
    }
}
