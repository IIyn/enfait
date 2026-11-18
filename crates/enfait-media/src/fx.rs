//! Post-FX globaux traduits en filtres ffmpeg.
//!
//! Même chaîne pour l'aperçu (images du proxy) et l'export : seules les
//! grandeurs en pixels (décalage des couches) suivent la hauteur de l'image.

use enfait_core::{Framing, PostFx};

/// Mise au cadre `width`×`height` d'une image quelconque (la prise) :
/// entière avec bandes noires, ou rognée pour remplir le cadre, décalée de
/// `pan` % de la marge (voir [`enfait_core::PAN_LIMIT`]). Se termine par
/// `setsar=1`.
pub fn framing(width: u32, height: u32, framing: Framing, pan: i8) -> String {
    let (w, h) = (width.max(2), height.max(2));
    match framing {
        Framing::Fit => format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1"
        ),
        Framing::Fill => {
            let k =
                0.5 + f64::from(pan.clamp(-enfait_core::PAN_LIMIT, enfait_core::PAN_LIMIT)) / 100.0;
            format!(
                "scale={w}:{h}:force_original_aspect_ratio=increase,\
                 crop={w}:{h}:(iw-{w})*{k:.3}:(ih-{h})*{k:.3},setsar=1"
            )
        }
    }
}

/// Chaîne de filtres (`-vf`) pour une image de `height` lignes ; `None` si
/// l'effet est neutre.
pub fn filter(fx: &PostFx, height: u32) -> Option<String> {
    let fx = fx.clamped();
    if fx.is_neutral() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if fx.contrast.abs() > 1e-3 || fx.saturation.abs() > 1e-3 {
        // Contraste de 0,6 à 1,4 ; saturation de 0 (noir et blanc) à 2.
        parts.push(format!(
            "eq=contrast={:.3}:saturation={:.3}",
            1.0 + 0.4 * fx.contrast,
            1.0 + fx.saturation
        ));
    }
    if fx.temperature.abs() > 1e-3 {
        // 6 500 K neutre ; chaud vers 3 500 K, froid vers 10 000 K.
        let kelvin = if fx.temperature > 0.0 {
            6500.0 - 3000.0 * fx.temperature
        } else {
            6500.0 - 3500.0 * fx.temperature
        };
        parts.push(format!("colortemperature=temperature={kelvin:.0}:mix=0.85"));
    }
    if fx.aberration > 1e-3 {
        // Jusqu'à 6 px à 1080 lignes, au moins 1 px.
        let shift = (fx.aberration * 6.0 * height as f32 / 1080.0)
            .round()
            .max(1.0);
        parts.push(format!("rgbashift=rh=-{shift:.0}:bh={shift:.0}"));
    }
    if fx.vignette > 1e-3 {
        // Angle de l'objectif simulé : plus il est grand, plus les bords
        // s'assombrissent.
        parts.push(format!("vignette=angle={:.3}", 0.2 + 0.75 * fx.vignette));
    }
    if fx.grain > 1e-3 {
        // Grain temporel (t) et uniforme (u), plus fort sur la luminance.
        parts.push(format!("noise=alls={:.0}:allf=t+u", 4.0 + 26.0 * fx.grain));
    }
    Some(parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::FX_PRESETS;

    #[test]
    fn neutral_has_no_filter() {
        assert_eq!(filter(&PostFx::NEUTRAL, 1080), None);
    }

    #[test]
    fn values_map_to_filters() {
        let fx = PostFx {
            grain: 0.5,
            aberration: 0.5,
            temperature: 1.0,
            ..PostFx::NEUTRAL
        };
        let f = filter(&fx, 540).unwrap();
        assert!(f.contains("colortemperature=temperature=3500"), "{f}");
        assert!(f.contains("rgbashift=rh=-2:bh=2"), "{f}");
        assert!(f.contains("noise=alls=17"), "{f}");
        assert!(!f.contains("eq="), "{f}");
        assert!(!f.starts_with(','), "{f}");
    }

    #[test]
    fn framings() {
        assert!(framing(1080, 1920, Framing::Fit, 0).contains("pad=1080:1920"));
        let fill = framing(1080, 1920, Framing::Fill, -50);
        assert!(
            fill.contains("increase,crop=1080:1920:(iw-1080)*0.000:(ih-1920)*0.000"),
            "{fill}"
        );
        assert!(framing(1080, 1920, Framing::Fill, 120).contains("*1.000"));
    }

    #[test]
    fn every_preset_but_neutral_has_a_filter() {
        for p in FX_PRESETS {
            assert_eq!(
                filter(&p.fx, 1080).is_some(),
                p.slug != "neutre",
                "{}",
                p.slug
            );
        }
    }
}
