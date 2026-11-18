//! Embedded fonts (IBM Plex, OFL 1.1 licence: `assets/fonts/OFL.txt`).

use gpui_kit::App;
use std::borrow::Cow;

pub const SANS: &str = "IBM Plex Sans";
pub const CONDENSED: &str = "IBM Plex Sans Condensed";
pub const MONO: &str = "IBM Plex Mono";

const FILES: [&[u8]; 6] = [
    include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexSansCondensed-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../../../assets/fonts/IBMPlexMono-Medium.ttf"),
];

pub fn register(cx: &mut App) -> anyhow::Result<()> {
    let fonts = FILES.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    cx.text_system().add_fonts(fonts)?;
    Ok(())
}
