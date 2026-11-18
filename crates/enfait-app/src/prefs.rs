//! User settings inside the app: loaded at startup, applied (language,
//! theme) and saved on every change. Views observe the [`Prefs`] global to
//! follow changes.

use crate::theme;
use enfait_config::Settings;
use gpui_kit::{App, Global, WindowAppearance};

pub struct Prefs {
    pub settings: Settings,
    /// Appearance of the desktop, for the "System" theme.
    appearance: WindowAppearance,
}

impl Global for Prefs {}

pub fn init(cx: &mut App) -> anyhow::Result<()> {
    let settings = Settings::load();
    let appearance = cx.window_appearance();
    // Reading the keychain can take a moment (D-Bus, unlock prompt): warm
    // the cache away from the interface thread.
    std::thread::spawn(|| {
        for secret in enfait_config::Secret::ALL {
            enfait_config::secrets::is_set(secret);
        }
    });
    rust_i18n::set_locale(settings.language.code());
    theme::apply(settings.theme, settings.accent, appearance, cx)?;
    cx.set_global(Prefs {
        settings,
        appearance,
    });
    Ok(())
}

pub fn settings(cx: &App) -> &Settings {
    &cx.global::<Prefs>().settings
}

pub fn appearance(cx: &App) -> WindowAppearance {
    cx.global::<Prefs>().appearance
}

/// Changes the settings, saves them and applies what changed.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let before = settings(cx).clone();
    let mut after = before.clone();
    change(&mut after);
    if after == before {
        return;
    }
    if let Err(e) = after.save() {
        tracing::error!("cannot save the settings: {e}");
    }
    if (&after.local_url, &after.local_model) != (&before.local_url, &before.local_model) {
        enfait_config::bump_generation();
    }
    if after.language != before.language {
        rust_i18n::set_locale(after.language.code());
    }
    let appearance = appearance(cx);
    if (after.theme, after.accent) != (before.theme, before.accent)
        && let Err(e) = theme::apply(after.theme, after.accent, appearance, cx)
    {
        tracing::error!("cannot apply the theme: {e:#}");
    }
    let language_changed = after.language != before.language;
    cx.global_mut::<Prefs>().settings = after;
    if language_changed {
        crate::tray::relabel(cx);
    }
    cx.refresh_windows();
}

/// The desktop switched between light and dark.
pub fn follow_appearance(appearance: WindowAppearance, cx: &mut App) {
    let Some(prefs) = cx.try_global::<Prefs>() else {
        return;
    };
    if prefs.appearance == appearance {
        return;
    }
    let (theme_id, accent) = (prefs.settings.theme, prefs.settings.accent);
    cx.global_mut::<Prefs>().appearance = appearance;
    if theme_id == enfait_config::ThemeId::System
        && let Err(e) = theme::apply(theme_id, accent, appearance, cx)
    {
        tracing::error!("cannot apply the theme: {e:#}");
    }
}
