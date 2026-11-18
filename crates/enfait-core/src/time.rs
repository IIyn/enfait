//! Temps exact, sans flottants.
//!
//! L'unité est le *flick* : 1/705 600 000 de seconde. Ce nombre se divise
//! exactement par les cadences courantes (24, 25, 30, 48, 50, 60, 120 i/s et
//! leurs variantes NTSC x/1001) et par les fréquences audio courantes
//! (8, 16, 22,05, 24, 32, 44,1, 48, 88,2, 96, 192 kHz). Une durée d'image ou
//! d'échantillon tombe donc sur un nombre entier de flicks : pas de dérive.

use crate::error::TimeError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Nombre de flicks dans une seconde.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

const FLICKS_PER_MILLI: i64 = 705_600;

/// Un instant ou une durée, en flicks.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Flicks(i64);

impl Flicks {
    pub const ZERO: Self = Self(0);

    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn from_secs(secs: i64) -> Result<Self, TimeError> {
        secs.checked_mul(FLICKS_PER_SECOND)
            .map(Self)
            .ok_or(TimeError::Overflow)
    }

    pub fn from_millis(millis: i64) -> Result<Self, TimeError> {
        millis
            .checked_mul(FLICKS_PER_MILLI)
            .map(Self)
            .ok_or(TimeError::Overflow)
    }

    /// Valeur approchée en secondes, pour l'affichage uniquement.
    pub fn as_secs_f64(self) -> f64 {
        // Conversion volontairement approchée : jamais réutilisée pour calculer.
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    pub fn checked_add(self, other: Self) -> Result<Self, TimeError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(TimeError::Overflow)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, TimeError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(TimeError::Overflow)
    }

    /// Instant du `samples`-ième échantillon à `sample_rate` Hz (arrondi à l'inférieur
    /// si la fréquence ne divise pas exactement la seconde en flicks).
    pub fn from_samples(samples: i64, sample_rate: u32) -> Result<Self, TimeError> {
        if sample_rate == 0 {
            return Err(TimeError::InvalidSampleRate);
        }
        let value = i128::from(samples)
            .checked_mul(i128::from(FLICKS_PER_SECOND))
            .and_then(|v| v.checked_div_euclid(i128::from(sample_rate)))
            .ok_or(TimeError::Overflow)?;
        narrow(value).map(Self)
    }

    /// Index de l'échantillon qui contient cet instant.
    pub fn to_sample_floor(self, sample_rate: u32) -> Result<i64, TimeError> {
        if sample_rate == 0 {
            return Err(TimeError::InvalidSampleRate);
        }
        let value = i128::from(self.0)
            .checked_mul(i128::from(sample_rate))
            .and_then(|v| v.checked_div_euclid(i128::from(FLICKS_PER_SECOND)))
            .ok_or(TimeError::Overflow)?;
        narrow(value)
    }
}

fn narrow(value: i128) -> Result<i64, TimeError> {
    i64::try_from(value).map_err(|_| TimeError::Overflow)
}

/// Cadence d'images, sous forme de fraction `num / den` images par seconde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawFrameRate", into = "RawFrameRate")]
pub struct FrameRate {
    num: u32,
    den: u32,
}

#[derive(Serialize, Deserialize)]
struct RawFrameRate {
    num: u32,
    den: u32,
}

impl TryFrom<RawFrameRate> for FrameRate {
    type Error = TimeError;

    fn try_from(raw: RawFrameRate) -> Result<Self, Self::Error> {
        Self::new(raw.num, raw.den)
    }
}

impl From<FrameRate> for RawFrameRate {
    fn from(rate: FrameRate) -> Self {
        Self {
            num: rate.num,
            den: rate.den,
        }
    }
}

impl FrameRate {
    pub const FPS_24: Self = Self { num: 24, den: 1 };
    pub const FPS_25: Self = Self { num: 25, den: 1 };
    pub const FPS_30: Self = Self { num: 30, den: 1 };
    pub const FPS_50: Self = Self { num: 50, den: 1 };
    pub const FPS_60: Self = Self { num: 60, den: 1 };
    pub const NTSC_24: Self = Self {
        num: 24_000,
        den: 1001,
    };
    pub const NTSC_30: Self = Self {
        num: 30_000,
        den: 1001,
    };
    pub const NTSC_60: Self = Self {
        num: 60_000,
        den: 1001,
    };

    pub fn new(num: u32, den: u32) -> Result<Self, TimeError> {
        if num == 0 || den == 0 {
            return Err(TimeError::InvalidFrameRate { num, den });
        }
        Ok(Self { num, den })
    }

    pub const fn num(self) -> u32 {
        self.num
    }

    pub const fn den(self) -> u32 {
        self.den
    }

    /// Nombre d'images par seconde arrondi au supérieur (30 pour 29,97).
    /// C'est la base des timecodes non-drop.
    pub fn nominal_fps(self) -> u32 {
        self.num.div_ceil(self.den)
    }

    /// Début de l'image `frame`.
    pub fn frame_to_flicks(self, frame: i64) -> Result<Flicks, TimeError> {
        let value = i128::from(frame)
            .checked_mul(i128::from(FLICKS_PER_SECOND))
            .and_then(|v| v.checked_mul(i128::from(self.den)))
            .and_then(|v| v.checked_div_euclid(i128::from(self.num)))
            .ok_or(TimeError::Overflow)?;
        narrow(value).map(Flicks)
    }

    /// Index de l'image affichée à l'instant `t`.
    pub fn flicks_to_frame_floor(self, t: Flicks) -> Result<i64, TimeError> {
        let divisor = i128::from(FLICKS_PER_SECOND)
            .checked_mul(i128::from(self.den))
            .ok_or(TimeError::Overflow)?;
        let value = i128::from(t.get())
            .checked_mul(i128::from(self.num))
            .and_then(|v| v.checked_div_euclid(divisor))
            .ok_or(TimeError::Overflow)?;
        narrow(value)
    }

    /// Première image qui commence à `t` ou après.
    pub fn flicks_to_frame_ceil(self, t: Flicks) -> Result<i64, TimeError> {
        let floor = self.flicks_to_frame_floor(t)?;
        if self.frame_to_flicks(floor)? < t {
            floor.checked_add(1).ok_or(TimeError::Overflow)
        } else {
            Ok(floor)
        }
    }

    /// Images dont le début tombe dans `range` : `[premier, dernier + 1)`.
    pub fn frames_in(self, range: TimeRange) -> Result<std::ops::Range<i64>, TimeError> {
        Ok(self.flicks_to_frame_ceil(range.start())?..self.flicks_to_frame_ceil(range.end())?)
    }
}

/// Intervalle semi-ouvert `[start, end)`, non vide, qui commence à 0 ou après.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRange", into = "RawRange")]
pub struct TimeRange {
    start: Flicks,
    end: Flicks,
}

#[derive(Serialize, Deserialize)]
struct RawRange {
    start: Flicks,
    end: Flicks,
}

impl TryFrom<RawRange> for TimeRange {
    type Error = TimeError;

    fn try_from(raw: RawRange) -> Result<Self, Self::Error> {
        Self::new(raw.start, raw.end)
    }
}

impl From<TimeRange> for RawRange {
    fn from(range: TimeRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

impl TimeRange {
    pub fn new(start: Flicks, end: Flicks) -> Result<Self, TimeError> {
        if start < Flicks::ZERO {
            return Err(TimeError::Negative);
        }
        if end <= start {
            return Err(TimeError::EmptyRange);
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> Flicks {
        self.start
    }

    pub const fn end(self) -> Flicks {
        self.end
    }

    pub fn duration(self) -> Flicks {
        // start >= 0 et end > start : la soustraction ne peut pas déborder.
        Flicks(self.end.0.saturating_sub(self.start.0))
    }

    pub fn contains(self, t: Flicks) -> bool {
        self.start <= t && t < self.end
    }

    /// Coupe en `[start, at)` et `[at, end)`. `at` doit être strictement à l'intérieur.
    pub fn split_at(self, at: Flicks) -> Result<(Self, Self), TimeError> {
        if at <= self.start || at >= self.end {
            return Err(TimeError::EmptyRange);
        }
        Ok((
            Self {
                start: self.start,
                end: at,
            },
            Self {
                start: at,
                end: self.end,
            },
        ))
    }
}

/// Timecode non-drop `HH:MM:SS:FF`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    hours: u32,
    minutes: u32,
    seconds: u32,
    frames: u32,
}

impl Timecode {
    pub fn from_flicks(t: Flicks, rate: FrameRate) -> Result<Self, TimeError> {
        if t < Flicks::ZERO {
            return Err(TimeError::Negative);
        }
        let frame = rate.flicks_to_frame_floor(t)?;
        let total = u64::try_from(frame).map_err(|_| TimeError::Negative)?;
        let fps = u64::from(rate.nominal_fps());
        let split = |value: u64, by: u64| -> Result<(u64, u64), TimeError> {
            let quotient = value.checked_div(by).ok_or(TimeError::Overflow)?;
            let rest = value.checked_rem(by).ok_or(TimeError::Overflow)?;
            Ok((quotient, rest))
        };
        let (total_secs, frames) = split(total, fps)?;
        let (total_mins, seconds) = split(total_secs, 60)?;
        let (hours, minutes) = split(total_mins, 60)?;
        let narrow = |v: u64| u32::try_from(v).map_err(|_| TimeError::Overflow);
        Ok(Self {
            hours: narrow(hours)?,
            minutes: narrow(minutes)?,
            seconds: narrow(seconds)?,
            frames: narrow(frames)?,
        })
    }

    pub const fn hours(self) -> u32 {
        self.hours
    }

    pub const fn minutes(self) -> u32 {
        self.minutes
    }

    pub const fn seconds(self) -> u32 {
        self.seconds
    }

    pub const fn frames(self) -> u32 {
        self.frames
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RATES: [FrameRate; 8] = [
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_30,
        FrameRate::FPS_50,
        FrameRate::FPS_60,
        FrameRate::NTSC_24,
        FrameRate::NTSC_30,
        FrameRate::NTSC_60,
    ];

    #[test]
    fn frames_in_covers_contiguous_ranges_without_gap() {
        let rate = FrameRate::NTSC_30;
        let a = TimeRange::new(Flicks::ZERO, Flicks::from_millis(1234).unwrap()).unwrap();
        let b = TimeRange::new(a.end(), Flicks::from_millis(5000).unwrap()).unwrap();
        let fa = rate.frames_in(a).unwrap();
        let fb = rate.frames_in(b).unwrap();
        assert_eq!(fa.start, 0);
        assert_eq!(fa.end, fb.start);
        assert!(fb.end > fb.start);
    }

    #[test]
    fn frame_durations_are_exact() {
        for rate in RATES {
            let one = rate.frame_to_flicks(1).unwrap().get();
            let expected = FLICKS_PER_SECOND * i64::from(rate.den());
            assert_eq!(
                one * i64::from(rate.num()),
                expected,
                "{}/{}",
                rate.num(),
                rate.den()
            );
        }
    }

    #[test]
    fn sample_durations_are_exact() {
        for hz in [
            8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000,
        ] {
            let t = Flicks::from_samples(i64::from(hz), hz).unwrap();
            assert_eq!(t, Flicks::from_secs(1).unwrap(), "{hz} Hz");
            assert_eq!(
                Flicks::from_samples(1, hz).unwrap().get() * i64::from(hz),
                FLICKS_PER_SECOND
            );
        }
    }

    #[test]
    fn timecode_formats_non_drop() {
        let t = FrameRate::FPS_30
            .frame_to_flicks(30 * 3600 + 30 * 61 + 2)
            .unwrap();
        let tc = Timecode::from_flicks(t, FrameRate::FPS_30).unwrap();
        assert_eq!(tc.to_string(), "01:01:01:02");
    }

    #[test]
    fn range_rejects_empty_and_negative() {
        let a = Flicks::new(10);
        assert_eq!(TimeRange::new(a, a), Err(TimeError::EmptyRange));
        assert_eq!(TimeRange::new(Flicks::new(-1), a), Err(TimeError::Negative));
    }

    #[test]
    fn range_split_bounds() {
        let r = TimeRange::new(Flicks::new(0), Flicks::new(10)).unwrap();
        assert!(r.split_at(Flicks::new(0)).is_err());
        assert!(r.split_at(Flicks::new(10)).is_err());
        let (a, b) = r.split_at(Flicks::new(4)).unwrap();
        assert_eq!(a.end(), b.start());
        assert_eq!(a.duration().get() + b.duration().get(), 10);
    }

    #[test]
    fn overflow_is_an_error() {
        assert_eq!(Flicks::from_secs(i64::MAX), Err(TimeError::Overflow));
        assert_eq!(
            Flicks::new(i64::MAX).checked_add(Flicks::new(1)),
            Err(TimeError::Overflow)
        );
    }

    #[test]
    fn invalid_frame_rate_is_rejected_on_deserialize() {
        let r: Result<FrameRate, _> = serde_json::from_str(r#"{"num":30,"den":0}"#);
        assert!(r.is_err());
    }

    proptest! {
        #[test]
        fn frame_roundtrip(frame in 0_i64..50_000_000, idx in 0_usize..8) {
            let rate = RATES[idx];
            let t = rate.frame_to_flicks(frame).unwrap();
            prop_assert_eq!(rate.flicks_to_frame_floor(t).unwrap(), frame);
        }

        #[test]
        fn instant_falls_in_its_frame(raw in 0_i64..(FLICKS_PER_SECOND * 36_000), idx in 0_usize..8) {
            let rate = RATES[idx];
            let t = Flicks::new(raw);
            let frame = rate.flicks_to_frame_floor(t).unwrap();
            prop_assert!(rate.frame_to_flicks(frame).unwrap() <= t);
            prop_assert!(rate.frame_to_flicks(frame + 1).unwrap() > t);
        }
    }
}
