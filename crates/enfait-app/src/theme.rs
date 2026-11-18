//! Colour themes.
//!
//! The views name colours by role ([`tokens`]); [`color`] resolves a role
//! against the current [`Palette`], chosen from the settings (theme and
//! accent) and, for the "System" theme, the appearance of the desktop.
//! [`apply`] also projects the palette onto the gpui-component theme
//! (buttons, inputs, lists…).

use crate::fonts;
use enfait_config::{Accent, ThemeId};
use gpui_kit::component::{Theme, ThemeMode, ThemeSet};
use gpui_kit::{App, Hsla, WindowAppearance, rgb};
use serde_json::json;
use std::rc::Rc;
use std::sync::RwLock;

/// A colour role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Bg,
    Panel,
    Raised,
    Hover,
    Line,
    LineStrong,
    Fg,
    Fg2,
    Mute,
    Signal,
    SignalHover,
    SignalPressed,
    OnSignal,
    Rush,
    Animation,
    Mixed,
    Effect,
    Broll,
    Sfx,
    Music,
    Success,
    Danger,
}

/// Colour roles under their historical names.
pub mod tokens {
    use super::Token;

    // Neutrals.
    pub const BG: Token = Token::Bg;
    pub const PANEL: Token = Token::Panel;
    pub const RAISED: Token = Token::Raised;
    pub const HOVER: Token = Token::Hover;
    pub const LINE: Token = Token::Line;
    pub const LINE_STRONG: Token = Token::LineStrong;

    // Text.
    pub const FG: Token = Token::Fg;
    pub const FG2: Token = Token::Fg2;
    pub const MUTE: Token = Token::Mute;

    // Signal (accent): main action, playhead, recording.
    pub const SIGNAL: Token = Token::Signal;
    pub const SIGNAL_HOVER: Token = Token::SignalHover;
    #[allow(dead_code)]
    pub const SIGNAL_PRESSED: Token = Token::SignalPressed;
    pub const ON_SIGNAL: Token = Token::OnSignal;

    // Tracks and block kinds.
    pub const RUSH: Token = Token::Rush;
    pub const ANIMATION: Token = Token::Animation;
    pub const MIXED: Token = Token::Mixed;
    pub const EFFECT: Token = Token::Effect;
    pub const BROLL: Token = Token::Broll;
    pub const SFX: Token = Token::Sfx;
    pub const MUSIC: Token = Token::Music;

    // States.
    pub const OK: Token = Token::Success;
    pub const ERROR: Token = Token::Danger;
}

/// Base colours of a theme, in `0xRRGGBB`.
#[derive(Debug, Clone, Copy)]
struct Base {
    dark: bool,
    bg: u32,
    panel: u32,
    raised: u32,
    hover: u32,
    line: u32,
    line_strong: u32,
    fg: u32,
    fg2: u32,
    mute: u32,
    rush: u32,
    animation: u32,
    mixed: u32,
    effect: u32,
    broll: u32,
    sfx: u32,
    music: u32,
    ok: u32,
    error: u32,
    /// Chartreuse, mint, sky, amber, rose, lavender.
    accents: [u32; 6],
}

/// Resolved colours.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    base: Base,
    signal: u32,
    signal_hover: u32,
    signal_pressed: u32,
    on_signal: u32,
}

impl Palette {
    pub const fn is_dark(&self) -> bool {
        self.base.dark
    }

    fn hex(&self, token: Token) -> u32 {
        let b = &self.base;
        match token {
            Token::Bg => b.bg,
            Token::Panel => b.panel,
            Token::Raised => b.raised,
            Token::Hover => b.hover,
            Token::Line => b.line,
            Token::LineStrong => b.line_strong,
            Token::Fg => b.fg,
            Token::Fg2 => b.fg2,
            Token::Mute => b.mute,
            Token::Signal => self.signal,
            Token::SignalHover => self.signal_hover,
            Token::SignalPressed => self.signal_pressed,
            Token::OnSignal => self.on_signal,
            Token::Rush => b.rush,
            Token::Animation => b.animation,
            Token::Mixed => b.mixed,
            Token::Effect => b.effect,
            Token::Broll => b.broll,
            Token::Sfx => b.sfx,
            Token::Music => b.music,
            Token::Success => b.ok,
            Token::Danger => b.error,
        }
    }

    fn new(base: Base, accent: Accent) -> Self {
        let index = Accent::ALL.iter().position(|a| *a == accent).unwrap_or(0);
        let signal = base.accents.get(index).copied().unwrap_or(base.fg);
        let (hover, pressed) = if base.dark {
            (mix(signal, 0xFFFFFF, 0.18), mix(signal, 0x000000, 0.12))
        } else {
            (mix(signal, 0x000000, 0.10), mix(signal, 0x000000, 0.20))
        };
        let on_signal = if luminance(signal) > 0.45 {
            0x0E1110
        } else {
            0xFFFFFF
        };
        Self {
            base,
            signal,
            signal_hover: hover,
            signal_pressed: pressed,
            on_signal,
        }
    }
}

/// `a` moved towards `b` by `t` (0 to 1), per channel.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let x = ((a >> shift) & 0xFF) as f32;
        let y = ((b >> shift) & 0xFF) as f32;
        let v = (x + (y - x) * t).round().clamp(0.0, 255.0) as u32;
        v << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Relative luminance (sRGB, WCAG).
fn luminance(c: u32) -> f32 {
    let lin = |shift: u32| {
        let v = ((c >> shift) & 0xFF) as f32 / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(16) + 0.7152 * lin(8) + 0.0722 * lin(0)
}

/// enfait dark: green-tinted cool greys.
const DARK: Base = Base {
    dark: true,
    bg: 0x101312,
    panel: 0x161A19,
    raised: 0x1C211F,
    hover: 0x242A28,
    line: 0x2B3230,
    line_strong: 0x3D4542,
    fg: 0xE6ECE9,
    fg2: 0xAEB8B4,
    mute: 0x85908C,
    rush: 0x7FA7D9,
    animation: 0xD77BC0,
    mixed: 0xE0A458,
    effect: 0x9B8CF0,
    broll: 0x8FBF6A,
    sfx: 0x5CC8C0,
    music: 0xE58F6E,
    ok: 0x5FD39A,
    error: 0xE5566E,
    accents: [0xC3F53C, 0x5FE3B0, 0x6CC4F5, 0xF2C04A, 0xF27BA6, 0xB39DF5],
};

/// enfait light: the same greys, inverted.
const LIGHT: Base = Base {
    dark: false,
    bg: 0xF4F6F5,
    panel: 0xEBEEED,
    raised: 0xFFFFFF,
    hover: 0xE1E6E4,
    line: 0xD5DBD9,
    line_strong: 0xB8C1BE,
    fg: 0x151917,
    fg2: 0x3F4845,
    mute: 0x66716D,
    rush: 0x3A6FB0,
    animation: 0xB0408F,
    mixed: 0xB8741A,
    effect: 0x6A55D6,
    broll: 0x4F8A2C,
    sfx: 0x1E8C84,
    music: 0xC2573A,
    ok: 0x1E9A5E,
    error: 0xC8283F,
    accents: [0x5F8A00, 0x128F67, 0x1F7FC4, 0xA86F00, 0xC23D72, 0x6C4FD1],
};

/// Catppuccin (MIT): <https://catppuccin.com/palette>.
const LATTE: Base = Base {
    dark: false,
    bg: 0xEFF1F5,
    panel: 0xE6E9EF,
    raised: 0xFFFFFF,
    hover: 0xDCE0E8,
    line: 0xCCD0DA,
    line_strong: 0xACB0BE,
    fg: 0x4C4F69,
    fg2: 0x5C5F77,
    mute: 0x7C7F93,
    rush: 0x1E66F5,
    animation: 0xEA76CB,
    mixed: 0xFE640B,
    effect: 0x8839EF,
    broll: 0x40A02B,
    sfx: 0x179299,
    music: 0xE64553,
    ok: 0x40A02B,
    error: 0xD20F39,
    accents: [0x40A02B, 0x179299, 0x04A5E5, 0xDF8E1D, 0xEA76CB, 0x7287FD],
};

const FRAPPE: Base = Base {
    dark: true,
    bg: 0x303446,
    panel: 0x292C3C,
    raised: 0x414559,
    hover: 0x51576D,
    line: 0x414559,
    line_strong: 0x626880,
    fg: 0xC6D0F5,
    fg2: 0xB5BFE2,
    mute: 0x949CBB,
    rush: 0x8CAAEE,
    animation: 0xF4B8E4,
    mixed: 0xEF9F76,
    effect: 0xCA9EE6,
    broll: 0xA6D189,
    sfx: 0x81C8BE,
    music: 0xEA999C,
    ok: 0xA6D189,
    error: 0xE78284,
    accents: [0xA6D189, 0x81C8BE, 0x99D1DB, 0xE5C890, 0xF4B8E4, 0xBABBF1],
};

const MACCHIATO: Base = Base {
    dark: true,
    bg: 0x24273A,
    panel: 0x1E2030,
    raised: 0x363A4F,
    hover: 0x494D64,
    line: 0x363A4F,
    line_strong: 0x5B6078,
    fg: 0xCAD3F5,
    fg2: 0xB8C0E0,
    mute: 0x939AB7,
    rush: 0x8AADF4,
    animation: 0xF5BDE6,
    mixed: 0xF5A97F,
    effect: 0xC6A0F6,
    broll: 0xA6DA95,
    sfx: 0x8BD5CA,
    music: 0xEE99A0,
    ok: 0xA6DA95,
    error: 0xED8796,
    accents: [0xA6DA95, 0x8BD5CA, 0x91D7E3, 0xEED49F, 0xF5BDE6, 0xB7BDF8],
};

const MOCHA: Base = Base {
    dark: true,
    bg: 0x1E1E2E,
    panel: 0x181825,
    raised: 0x313244,
    hover: 0x45475A,
    line: 0x313244,
    line_strong: 0x585B70,
    fg: 0xCDD6F4,
    fg2: 0xBAC2DE,
    mute: 0x9399B2,
    rush: 0x89B4FA,
    animation: 0xF5C2E7,
    mixed: 0xFAB387,
    effect: 0xCBA6F7,
    broll: 0xA6E3A1,
    sfx: 0x94E2D5,
    music: 0xEBA0AC,
    ok: 0xA6E3A1,
    error: 0xF38BA8,
    accents: [0xA6E3A1, 0x94E2D5, 0x89DCEB, 0xF9E2AF, 0xF5C2E7, 0xB4BEFE],
};

/// Gruvbox (MIT): <https://github.com/morhetz/gruvbox>.
const GRUVBOX: Base = Base {
    dark: true,
    bg: 0x282828,
    panel: 0x1D2021,
    raised: 0x32302F,
    hover: 0x45403D,
    line: 0x3C3836,
    line_strong: 0x665C54,
    fg: 0xEBDBB2,
    fg2: 0xD5C4A1,
    mute: 0xA89984,
    rush: 0x83A598,
    animation: 0xD3869B,
    mixed: 0xFE8019,
    effect: 0xB16286,
    broll: 0xB8BB26,
    sfx: 0x8EC07C,
    music: 0xFB4934,
    ok: 0xB8BB26,
    error: 0xFB4934,
    accents: [0xB8BB26, 0x8EC07C, 0x83A598, 0xFABD2F, 0xD3869B, 0xB16286],
};

/// Nord (MIT): <https://www.nordtheme.com>.
const NORD: Base = Base {
    dark: true,
    bg: 0x2E3440,
    panel: 0x292E39,
    raised: 0x3B4252,
    hover: 0x434C5E,
    line: 0x3B4252,
    line_strong: 0x4C566A,
    fg: 0xECEFF4,
    fg2: 0xD8DEE9,
    mute: 0x9AA5B8,
    rush: 0x81A1C1,
    animation: 0xB48EAD,
    mixed: 0xD08770,
    effect: 0x5E81AC,
    broll: 0xA3BE8C,
    sfx: 0x8FBCBB,
    music: 0xBF616A,
    ok: 0xA3BE8C,
    error: 0xBF616A,
    accents: [0xA3BE8C, 0x8FBCBB, 0x88C0D0, 0xEBCB8B, 0xBF616A, 0xB48EAD],
};

fn base(theme: ThemeId, appearance: WindowAppearance) -> Base {
    match theme {
        ThemeId::System => match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => LIGHT,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK,
        },
        ThemeId::Dark => DARK,
        ThemeId::Light => LIGHT,
        ThemeId::CatppuccinLatte => LATTE,
        ThemeId::CatppuccinFrappe => FRAPPE,
        ThemeId::CatppuccinMacchiato => MACCHIATO,
        ThemeId::CatppuccinMocha => MOCHA,
        ThemeId::GruvboxDark => GRUVBOX,
        ThemeId::Nord => NORD,
    }
}

/// Swatch of a theme for the settings (background, panel, text, accent).
pub fn swatch(theme: ThemeId, accent: Accent, appearance: WindowAppearance) -> [Hsla; 4] {
    let palette = Palette::new(base(theme, appearance), accent);
    [
        rgb(palette.base.bg).into(),
        rgb(palette.base.raised).into(),
        rgb(palette.base.fg).into(),
        rgb(palette.signal).into(),
    ]
}

/// Accent colour of `accent` in `theme`.
pub fn accent_color(theme: ThemeId, accent: Accent, appearance: WindowAppearance) -> Hsla {
    rgb(Palette::new(base(theme, appearance), accent).signal).into()
}

static CURRENT: RwLock<Palette> = RwLock::new(Palette {
    base: DARK,
    signal: 0xC3F53C,
    signal_hover: 0xD0F765,
    signal_pressed: 0xACD835,
    on_signal: 0x0E1110,
});

fn current() -> Palette {
    CURRENT.read().map_or_else(
        |poisoned| *poisoned.into_inner(),
        |palette| *palette,
    )
}

/// Row height of lists, in px.
pub const ROW_HEIGHT: f32 = 28.;
/// Interface text size, in px.
pub const TEXT_SIZE: f32 = 13.;

/// Colour of a role in the current theme.
pub fn color(token: Token) -> Hsla {
    rgb(current().hex(token)).into()
}

fn hex(value: u32) -> String {
    format!("#{value:06X}")
}

/// Hex with opacity (`alpha` from 0 to 255).
fn hexa(value: u32, alpha: u8) -> String {
    format!("#{value:06X}{alpha:02X}")
}

/// Selects the palette for `theme` and `accent` and installs it, in
/// enfait's views and in gpui-component. `appearance` matters only for
/// the System theme.
pub fn apply(
    theme: ThemeId,
    accent: Accent,
    appearance: WindowAppearance,
    cx: &mut App,
) -> anyhow::Result<()> {
    let palette = Palette::new(base(theme, appearance), accent);
    if let Ok(mut slot) = CURRENT.write() {
        *slot = palette;
    }
    let p = |token| palette.hex(token);
    use Token::*;
    let (black, white) = (0x000000, 0xFFFFFF);
    let link_hover = if palette.is_dark() { white } else { black };

    let colors: serde_json::Map<String, serde_json::Value> = [
        ("background", hex(p(Bg))),
        ("foreground", hex(p(Fg))),
        ("border", hex(p(Line))),
        ("window.border", hex(p(Line))),
        ("caret", hex(p(Signal))),
        ("ring", hex(p(LineStrong))),
        ("overlay", hexa(black, 0x66)),
        ("primary.background", hex(p(Signal))),
        ("primary.hover.background", hex(p(SignalHover))),
        ("primary.active.background", hex(p(SignalPressed))),
        ("primary.foreground", hex(p(OnSignal))),
        ("secondary.background", hex(p(Raised))),
        ("secondary.hover.background", hex(p(Hover))),
        ("secondary.active.background", hex(p(Line))),
        ("secondary.foreground", hex(p(Fg))),
        ("accent.background", hex(p(Hover))),
        ("accent.foreground", hex(p(Fg))),
        ("muted.background", hex(p(Raised))),
        ("muted.foreground", hex(p(Mute))),
        ("danger.background", hex(p(Danger))),
        ("danger.foreground", hex(p(Danger))),
        ("success.background", hex(p(Success))),
        ("success.foreground", hex(p(Success))),
        ("warning.background", hex(p(Mixed))),
        ("warning.foreground", hex(p(Mixed))),
        ("info.background", hex(p(Rush))),
        ("info.foreground", hex(p(Rush))),
        ("input.border", hex(p(LineStrong))),
        ("selection.background", hexa(p(Signal), 0x40)),
        ("drag_border", hex(p(Signal))),
        ("drop_target.background", hexa(p(Signal), 0x1A)),
        ("link.foreground", hex(p(Fg))),
        ("link.hover.foreground", hex(link_hover)),
        ("link.active.foreground", hex(p(Fg2))),
        ("popover.background", hex(p(Raised))),
        ("popover.foreground", hex(p(Fg))),
        ("list.background", hex(p(Bg))),
        ("list.even.background", hex(p(Bg))),
        ("list.head.background", hex(p(Panel))),
        ("list.hover.background", hex(p(Hover))),
        ("list.active.background", hex(p(Hover))),
        ("list.active.border", hex(p(Signal))),
        ("list.highlight.foreground", hex(p(Signal))),
        ("table.background", hex(p(Bg))),
        ("table.head.foreground", hex(p(Mute))),
        ("table.row.border", hex(p(Line))),
        ("tab.background", hexa(black, 0x00)),
        ("tab.active.background", hex(p(Bg))),
        ("tab.active.foreground", hex(p(Fg))),
        ("tab.foreground", hex(p(Fg2))),
        ("tab_bar.background", hex(p(Panel))),
        ("tab_bar.segmented.background", hex(p(Panel))),
        ("sidebar.background", hex(p(Panel))),
        ("sidebar.foreground", hex(p(Fg))),
        ("sidebar.border", hex(p(Line))),
        ("sidebar.accent.background", hex(p(Hover))),
        ("sidebar.accent.foreground", hex(p(Fg))),
        ("sidebar.primary.background", hex(p(Signal))),
        ("sidebar.primary.foreground", hex(p(OnSignal))),
        ("title_bar.background", hex(p(Panel))),
        ("title_bar.border", hex(p(Line))),
        ("status_bar.background", hex(p(Panel))),
        ("status_bar.border", hex(p(Line))),
        ("scrollbar.background", hexa(p(Bg), 0x00)),
        ("scrollbar.thumb.background", hexa(p(LineStrong), 0xE6)),
        ("scrollbar.thumb.hover.background", hex(p(LineStrong))),
        ("progress_bar.background", hex(p(Signal))),
        ("slider.bar.background", hex(p(Signal))),
        ("slider.thumb.background", hex(p(Fg))),
        ("switch.background", hex(p(LineStrong))),
        ("skeleton.background", hex(p(Raised))),
        ("accordion.background", hex(p(Panel))),
        ("group_box.background", hex(p(Panel))),
        ("group_box.foreground", hex(p(Fg))),
        ("description_list_label.background", hex(p(Panel))),
        ("description_list_label.foreground", hex(p(Fg2))),
        ("chart.1", hex(p(Rush))),
        ("chart.2", hex(p(Animation))),
        ("chart.3", hex(p(Mixed))),
        ("chart.4", hex(p(Effect))),
        ("chart.5", hex(p(Sfx))),
        ("chart.grid", hex(p(Line))),
        ("chart.bullish", hex(p(Success))),
        ("chart.bearish", hex(p(Danger))),
        ("base.red", hex(p(Danger))),
        ("base.green", hex(p(Success))),
        ("base.blue", hex(p(Rush))),
        ("base.yellow", hex(p(Mixed))),
        ("base.magenta", hex(p(Animation))),
        ("base.cyan", hex(p(Sfx))),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), serde_json::Value::String(value)))
    .collect();

    let mode = if palette.is_dark() { "dark" } else { "light" };
    let set = json!({
        "name": "enfait",
        "themes": [{
            "is_default": true,
            "name": "enfait",
            "mode": mode,
            "font.family": fonts::SANS,
            "font.size": TEXT_SIZE,
            "mono_font.family": fonts::MONO,
            "mono_font.size": 12.0,
            "radius": 3,
            "radius.lg": 4,
            "window.radius": 0,
            "shadow": false,
            "colors": colors,
        }]
    });

    let set: ThemeSet = serde_json::from_value(set)?;
    let config = set
        .themes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("the enfait theme set is empty"))?;
    let config = Rc::new(config);
    Theme::update(cx, |theme| theme.apply_config(&config));
    let mode = if palette.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, None, cx);
    cx.refresh_windows();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_on_accent_is_readable() {
        for theme in ThemeId::ALL {
            for accent in Accent::ALL {
                let p = Palette::new(base(theme, WindowAppearance::Dark), accent);
                let (a, b) = (luminance(p.signal), luminance(p.on_signal));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 3.0, "{theme:?} {accent:?}: contrast {ratio:.2}");
            }
        }
    }
}
