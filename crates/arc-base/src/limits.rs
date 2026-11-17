//! Hostile-input limits profiles (CORE-BASE-013).

use serde::{Deserialize, Serialize};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// The limits a reader of untrusted input enforces. Every field is one named limit.
///
/// Start from a profile constant and either tighten it with [`Limits::tightened`], which can only
/// lower limits, or override a named limit explicitly with struct update syntax:
///
/// ```
/// use arc_base::Limits;
/// let custom = Limits { nesting_depth: 256, ..Limits::DEFAULT };
/// assert_eq!(custom.string_bytes, Limits::DEFAULT.string_bytes);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Input file size in bytes.
    pub input_bytes: u64,
    /// Total decompressed size in bytes over all compressed streams.
    pub decompressed_bytes: u64,
    /// Decompression ratio per stream, as N in N : 1.
    pub decompression_ratio: u32,
    /// Number of records or entities.
    pub record_count: u64,
    /// Nesting depth of blocks, elements, aggregations and compositions.
    pub nesting_depth: u32,
    /// Length of a single string in bytes.
    pub string_bytes: u64,
    /// Length of a single array (vertices, points per entity, knots).
    pub array_length: u64,
    /// Memory budget of the import in bytes; `None` leaves the budget to the caller, which
    /// derives it from the memory available on the machine.
    pub memory_bytes: Option<u64>,
    /// Wall-clock time in seconds; `None` is unlimited.
    pub wall_clock_seconds: Option<u64>,
}

impl Limits {
    /// The `strict` profile, for input of unknown origin.
    pub const STRICT: Self = Self {
        input_bytes: 256 * MIB,
        decompressed_bytes: GIB,
        decompression_ratio: 100,
        record_count: 1_000_000,
        nesting_depth: 32,
        string_bytes: MIB,
        array_length: 1_000_000,
        memory_bytes: Some(2 * GIB),
        wall_clock_seconds: Some(60),
    };

    /// The `default` profile.
    pub const DEFAULT: Self = Self {
        input_bytes: 4 * GIB,
        decompressed_bytes: 16 * GIB,
        decompression_ratio: 200,
        record_count: 50_000_000,
        nesting_depth: 64,
        string_bytes: 16 * MIB,
        array_length: 50_000_000,
        memory_bytes: Some(16 * GIB),
        wall_clock_seconds: None,
    };

    /// The `large-models` profile. Its memory budget is the available memory minus a 2 GiB
    /// reserve, which the caller computes.
    pub const LARGE_MODELS: Self = Self {
        input_bytes: 64 * GIB,
        decompressed_bytes: 256 * GIB,
        decompression_ratio: 500,
        record_count: 1_000_000_000,
        nesting_depth: 128,
        string_bytes: 64 * MIB,
        array_length: 500_000_000,
        memory_bytes: None,
        wall_clock_seconds: None,
    };

    /// The `constrained` profile, the default in browser, tablet and phone builds.
    pub const CONSTRAINED: Self = Self {
        input_bytes: 512 * MIB,
        decompressed_bytes: 2 * GIB,
        decompression_ratio: 100,
        record_count: 10_000_000,
        nesting_depth: 32,
        string_bytes: 4 * MIB,
        array_length: 10_000_000,
        memory_bytes: Some(GIB),
        wall_clock_seconds: Some(120),
    };

    /// Looks a profile up by its name: `strict`, `default`, `large-models` or `constrained`.
    #[must_use]
    pub fn profile(name: &str) -> Option<Self> {
        match name {
            "strict" => Some(Self::STRICT),
            "default" => Some(Self::DEFAULT),
            "large-models" => Some(Self::LARGE_MODELS),
            "constrained" => Some(Self::CONSTRAINED),
            _ => None,
        }
    }

    /// Combines two profiles into one that is at least as tight as both: every limit is the
    /// lower of the two.
    #[must_use]
    pub fn tightened(self, other: Self) -> Self {
        // `None` is the loosest value of an optional limit.
        let lower = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (limit, None) | (None, limit) => limit,
        };
        Self {
            input_bytes: self.input_bytes.min(other.input_bytes),
            decompressed_bytes: self.decompressed_bytes.min(other.decompressed_bytes),
            decompression_ratio: self.decompression_ratio.min(other.decompression_ratio),
            record_count: self.record_count.min(other.record_count),
            nesting_depth: self.nesting_depth.min(other.nesting_depth),
            string_bytes: self.string_bytes.min(other.string_bytes),
            array_length: self.array_length.min(other.array_length),
            memory_bytes: lower(self.memory_bytes, other.memory_bytes),
            wall_clock_seconds: lower(self.wall_clock_seconds, other.wall_clock_seconds),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_found_by_name() {
        assert_eq!(Limits::profile("strict"), Some(Limits::STRICT));
        assert_eq!(Limits::profile("default"), Some(Limits::default()));
        assert_eq!(Limits::profile("large-models"), Some(Limits::LARGE_MODELS));
        assert_eq!(Limits::profile("constrained"), Some(Limits::CONSTRAINED));
        assert_eq!(Limits::profile("unbounded"), None);
    }

    #[test]
    fn profile_values_match_the_documented_table() {
        assert_eq!(Limits::STRICT.input_bytes, 268_435_456);
        assert_eq!(Limits::DEFAULT.decompressed_bytes, 17_179_869_184);
        assert_eq!(Limits::LARGE_MODELS.nesting_depth, 128);
        assert_eq!(Limits::CONSTRAINED.wall_clock_seconds, Some(120));
    }

    #[test]
    fn tightening_never_loosens_a_limit() {
        assert_eq!(
            Limits::STRICT.tightened(Limits::LARGE_MODELS),
            Limits::STRICT
        );
        assert_eq!(
            Limits::LARGE_MODELS.tightened(Limits::STRICT),
            Limits::STRICT
        );

        let mixed = Limits::CONSTRAINED.tightened(Limits::STRICT);
        assert_eq!(mixed.input_bytes, Limits::STRICT.input_bytes);
        assert_eq!(mixed.memory_bytes, Limits::CONSTRAINED.memory_bytes);
        assert_eq!(mixed.wall_clock_seconds, Some(60));
    }
}
