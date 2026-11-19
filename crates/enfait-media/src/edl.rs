//! Edit decision list: pieces of the source media played end to end.
//!
//! The player works in "edit" time (from 0 to the sum of the pieces);
//! sound and frames are looked up in the source by [`Edl::to_source`].
//! Without an edit, a single part covers the whole media.

/// A piece: `[source_start, source_end)` in the source, placed at `start`
/// in the edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub source_start: f64,
    pub source_end: f64,
    pub start: f64,
}

impl Part {
    pub fn duration(&self) -> f64 {
        self.source_end - self.source_start
    }

    pub fn end(&self) -> f64 {
        self.start + self.duration()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edl {
    parts: Vec<Part>,
}

impl Edl {
    /// The whole media, in one piece.
    pub fn identity(duration: f64) -> Self {
        Self::from_ranges(&[(0.0, duration.max(0.0))])
    }

    /// Pieces `(start, end)` of the source, in edit order. Empty or invalid
    /// pieces are ignored.
    pub fn from_ranges(ranges: &[(f64, f64)]) -> Self {
        let mut start = 0.0;
        let mut parts = Vec::with_capacity(ranges.len());
        for &(a, b) in ranges {
            if !(a.is_finite() && b.is_finite()) || b <= a {
                continue;
            }
            let a = a.max(0.0);
            parts.push(Part {
                source_start: a,
                source_end: b,
                start,
            });
            start += b - a;
        }
        Self { parts }
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Single-piece edit starting at the start of the source?
    pub fn is_identity(&self) -> bool {
        matches!(self.parts.as_slice(), [p] if p.source_start == 0.0)
    }

    pub fn duration(&self) -> f64 {
        self.parts.last().map_or(0.0, Part::end)
    }

    /// Piece containing edit time `t` (the next one at a boundary; the last
    /// one beyond the end).
    pub fn locate(&self, t: f64) -> Option<(usize, &Part)> {
        let i = self.parts.partition_point(|p| p.end() <= t);
        let i = i.min(self.parts.len().saturating_sub(1));
        self.parts.get(i).map(|p| (i, p))
    }

    /// Source time played at edit time `t`.
    pub fn to_source(&self, t: f64) -> f64 {
        match self.locate(t) {
            Some((_, p)) => (p.source_start + (t - p.start).max(0.0)).min(p.source_end),
            None => 0.0,
        }
    }

    /// Edit time where source time `source` is heard (first piece containing
    /// it).
    pub fn to_output(&self, source: f64) -> Option<f64> {
        self.parts
            .iter()
            .find(|p| source >= p.source_start && source < p.source_end)
            .map(|p| p.start + source - p.source_start)
    }

    /// Distance from `t` to the nearest cut (boundary between two pieces not
    /// contiguous in the source); infinite if there is none.
    pub fn cut_distance(&self, t: f64) -> f64 {
        self.parts
            .windows(2)
            .filter_map(|w| match w {
                [a, b] if (a.source_end - b.source_start).abs() > 1e-6 => Some(b.start),
                _ => None,
            })
            .map(|cut| (t - cut).abs())
            .fold(f64::INFINITY, f64::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_between_output_and_source() {
        let e = Edl::from_ranges(&[(2.0, 3.0), (10.0, 10.5), (5.0, 5.0), (0.0, 1.0)]);
        assert_eq!(e.parts().len(), 3, "empty piece ignored");
        assert_eq!(e.duration(), 2.5);
        assert_eq!(e.to_source(0.0), 2.0);
        assert_eq!(e.to_source(0.99), 2.99);
        assert_eq!(e.to_source(1.0), 10.0, "boundary: next piece");
        assert_eq!(e.to_source(1.25), 10.25);
        assert_eq!(e.to_source(9.0), 1.0, "beyond: end of the last one");
        assert_eq!(e.to_output(10.25), Some(1.25));
        assert_eq!(e.to_output(7.0), None);
        assert!((e.cut_distance(1.1) - 0.1).abs() < 1e-9);
        assert!((e.cut_distance(1.45) - 0.05).abs() < 1e-9);
    }

    #[test]
    fn identity_and_contiguous_parts() {
        let id = Edl::identity(4.0);
        assert!(id.is_identity());
        assert_eq!(id.to_source(1.5), 1.5);
        assert_eq!(id.cut_distance(1.0), f64::INFINITY);
        // Two pieces following each other in the source: no cut.
        let joined = Edl::from_ranges(&[(0.0, 1.0), (1.0, 2.0)]);
        assert_eq!(joined.cut_distance(1.0), f64::INFINITY);
    }
}
