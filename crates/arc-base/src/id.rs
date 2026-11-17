//! Persistent identifiers (CORE-BASE-001 to CORE-BASE-004).
//!
//! Identifiers are version 7 UUIDs: a 48-bit millisecond timestamp followed by random bits. This
//! crate makes no operating-system calls, so the wall clock and the entropy of a generator come
//! from the [`TimeSource`] and [`RandomSource`] the application supplies; a seeded generator
//! needs neither and is identical on every run and platform.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result, codes};

/// Supplies the wall-clock time of an [`IdGenerator`].
pub trait TimeSource {
    /// Milliseconds since the Unix epoch. The clock may stand still or step backwards; the
    /// generator stays monotonic regardless.
    fn unix_millis(&mut self) -> u64;
}

/// Supplies the random bits of an [`IdGenerator`].
pub trait RandomSource {
    /// The next 64 random bits.
    fn next_u64(&mut self) -> u64;
}

const MILLIS_MASK: u64 = (1 << 48) - 1;
/// The 74 bits of a version 7 UUID that are neither timestamp, version nor variant.
const RANDOM_MASK: u128 = (1 << 74) - 1;
const VARIANT_FIELD_MASK: u128 = (1 << 62) - 1;

/// The first timestamp of the logical clock of seeded generators: 2020-01-01T00:00:00Z.
pub const SEEDED_EPOCH_MILLIS: u64 = 1_577_836_800_000;

/// The logical clock of seeded generators: one millisecond per identifier.
struct LogicalClock(u64);

impl TimeSource for LogicalClock {
    fn unix_millis(&mut self) -> u64 {
        let now = self.0;
        self.0 = self.0.saturating_add(1);
        now
    }
}

/// The SplitMix64 generator: small, fast and fully specified, so seeded sequences are the same
/// on every platform. Not for security purposes.
struct SplitMix64(u64);

impl RandomSource for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Generates version 7 identifiers.
///
/// Identifiers from one generator are always distinct and strictly increasing, also within one
/// millisecond and when the clock steps backwards: the random field then continues as a counter.
pub struct IdGenerator {
    time: Box<dyn TimeSource + Send>,
    random: Box<dyn RandomSource + Send>,
    /// Timestamp and random field of the previous identifier.
    last: Option<(u64, u128)>,
}

impl fmt::Debug for IdGenerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdGenerator")
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl IdGenerator {
    /// Creates a generator on the application's clock and entropy.
    #[must_use]
    pub fn new(
        time: impl TimeSource + Send + 'static,
        random: impl RandomSource + Send + 'static,
    ) -> Self {
        Self {
            time: Box::new(time),
            random: Box::new(random),
            last: None,
        }
    }

    /// Creates a generator that produces the same sequence for the same seed on every run and
    /// platform. Its timestamps come from a logical clock that starts at
    /// [`SEEDED_EPOCH_MILLIS`] and advances one millisecond per identifier.
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        Self::new(LogicalClock(SEEDED_EPOCH_MILLIS), SplitMix64(seed))
    }

    /// Generates the next identifier of the requested type.
    pub fn generate<I: Identifier>(&mut self) -> I {
        I::from_u128(self.next_raw())
    }

    fn next_raw(&mut self) -> u128 {
        let now = self.time.unix_millis() & MILLIS_MASK;
        let (millis, random) = match self.last {
            Some((last_millis, last_random)) if now <= last_millis => {
                if last_random < RANDOM_MASK {
                    (last_millis, last_random + 1)
                } else {
                    // The counter is exhausted: borrow the next millisecond.
                    (
                        last_millis.saturating_add(1) & MILLIS_MASK,
                        self.fresh_random(),
                    )
                }
            }
            _ => (now, self.fresh_random()),
        };
        self.last = Some((millis, random));
        (u128::from(millis) << 80)
            | (0x7 << 76)
            | ((random >> 62) << 64)
            | (0b10 << 62)
            | (random & VARIANT_FIELD_MASK)
    }

    /// Random bits with the top bit clear, which leaves room for the counter.
    fn fresh_random(&mut self) -> u128 {
        let high = u128::from(self.random.next_u64());
        let low = u128::from(self.random.next_u64());
        ((high << 64) | low) & (RANDOM_MASK >> 1)
    }
}

/// A typed identifier. The four identifier types share this interface but are distinct types: a
/// value of one becomes a value of another only through an explicit `as_u128` and `from_u128`.
pub trait Identifier: Copy + Ord + fmt::Display + FromStr<Err = Error> {
    /// Wraps a raw 128-bit value.
    fn from_u128(raw: u128) -> Self;
    /// The raw 128-bit value.
    fn as_u128(self) -> u128;
}

fn parse(text: &str) -> Result<Uuid> {
    // Only the hyphenated (36) and plain (32) forms; braces and URN prefixes are rejected.
    if !matches!(text.len(), 32 | 36) {
        return Err(Error::new(codes::ID_INVALID));
    }
    Uuid::try_parse(text).map_err(|cause| Error::new(codes::ID_INVALID).with_source(cause))
}

macro_rules! identifier {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// The text form is the lowercase hyphenated form; parsing accepts upper- or lowercase,
        /// hyphenated or not, and rejects everything else with `base.id.invalid`. Ordering
        /// follows the raw value, so identifiers sort by generation time.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl Identifier for $name {
            fn from_u128(raw: u128) -> Self {
                Self(Uuid::from_u128(raw))
            }

            fn as_u128(self) -> u128 {
                self.0.as_u128()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self.0.as_hyphenated(), f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(text: &str) -> Result<Self> {
                parse(text).map(Self)
            }
        }
    };
}

identifier!(
    /// Identifies an object of a document, everywhere outside one snapshot.
    ObjectId
);
identifier!(
    /// Identifies a committed transaction.
    TransactionId
);
identifier!(
    /// Identifies a document.
    DocumentId
);
identifier!(
    /// Identifies an editing or automation session.
    SessionId
);

#[cfg(test)]
mod tests {
    use proptest::prelude::{any, prop_assert, prop_assert_eq, proptest};

    use super::*;

    struct FixedClock(u64);

    impl TimeSource for FixedClock {
        fn unix_millis(&mut self) -> u64 {
            self.0
        }
    }

    /// Replays a list of timestamps, repeating the last one.
    struct Replay(Vec<u64>, usize);

    impl TimeSource for Replay {
        fn unix_millis(&mut self) -> u64 {
            let at = self.1.min(self.0.len() - 1);
            self.1 += 1;
            self.0[at]
        }
    }

    struct AllOnes;

    impl RandomSource for AllOnes {
        fn next_u64(&mut self) -> u64 {
            u64::MAX
        }
    }

    fn millis(id: ObjectId) -> u64 {
        u64::try_from(id.as_u128() >> 80).unwrap()
    }

    #[test]
    fn splitmix64_matches_its_published_test_vector() {
        let mut random = SplitMix64(0);
        assert_eq!(random.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(random.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(random.next_u64(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn identifiers_are_version_7_with_the_rfc_variant() {
        let mut generator = IdGenerator::new(FixedClock(0x0123_4567_89AB), AllOnes);
        let id: ObjectId = generator.generate();
        let uuid = Uuid::from_u128(id.as_u128());
        assert_eq!(uuid.get_version_num(), 7);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(millis(id), 0x0123_4567_89AB);
    }

    #[test]
    fn seeded_generator_is_deterministic_on_every_platform() {
        let mut generator = IdGenerator::seeded(42);
        let ids: Vec<String> = (0..3)
            .map(|_| generator.generate::<ObjectId>().to_string())
            .collect();
        assert_eq!(ids, SEED_42_GOLDEN);

        let mut again = IdGenerator::seeded(42);
        assert_eq!(again.generate::<ObjectId>().to_string(), SEED_42_GOLDEN[0]);
        let mut other = IdGenerator::seeded(43);
        assert_ne!(other.generate::<ObjectId>().to_string(), SEED_42_GOLDEN[0]);
    }

    /// The first identifiers of seed 42; the same on every platform CI runs on.
    const SEED_42_GOLDEN: [&str; 3] = [
        "016f5e66-e800-7254-a8ef-e333b266f103",
        "016f5e66-e801-7549-981c-e1ff0e4ae394",
        "016f5e66-e802-77cb-9e44-31fa3c80db06",
    ];

    #[test]
    fn seeded_timestamps_follow_the_logical_clock() {
        let mut generator = IdGenerator::seeded(7);
        for step in 0..5 {
            assert_eq!(millis(generator.generate()), SEEDED_EPOCH_MILLIS + step);
        }
    }

    #[test]
    fn identifiers_in_one_millisecond_are_distinct_and_increasing() {
        let mut generator = IdGenerator::new(FixedClock(1_000), SplitMix64(1));
        let ids: Vec<ObjectId> = (0..1_000).map(|_| generator.generate()).collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.iter().all(|&id| millis(id) == 1_000));
    }

    #[test]
    fn identifiers_of_later_milliseconds_compare_in_generation_order() {
        let mut generator = IdGenerator::new(Replay(vec![10, 11, 12], 0), AllOnes);
        let (a, b, c): (ObjectId, ObjectId, ObjectId) = (
            generator.generate(),
            generator.generate(),
            generator.generate(),
        );
        assert!(a < b && b < c);
    }

    #[test]
    fn a_clock_stepping_backwards_keeps_identifiers_increasing() {
        let mut generator = IdGenerator::new(Replay(vec![500, 100, 100], 0), SplitMix64(9));
        let (a, b, c): (ObjectId, ObjectId, ObjectId) = (
            generator.generate(),
            generator.generate(),
            generator.generate(),
        );
        assert!(a < b && b < c);
        assert_eq!(millis(c), 500);
    }

    #[test]
    fn an_exhausted_counter_borrows_the_next_millisecond() {
        let mut generator = IdGenerator::new(FixedClock(1_000), AllOnes);
        generator.last = Some((1_000, RANDOM_MASK));
        let id: ObjectId = generator.generate();
        assert_eq!(millis(id), 1_001);
    }

    #[test]
    fn text_form_is_lowercase_hyphenated() {
        let id = ObjectId::from_u128(0x0190_A1B2_C3D4_7E5F_8A6B_0C1D_2E3F_4A5B);
        assert_eq!(id.to_string(), "0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b");
        assert_eq!(
            format!("{id:?}"),
            "ObjectId(0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b)"
        );
    }

    #[test]
    fn parsing_accepts_both_cases_with_and_without_hyphens() {
        let expected = ObjectId::from_u128(0x0190_A1B2_C3D4_7E5F_8A6B_0C1D_2E3F_4A5B);
        for text in [
            "0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b",
            "0190A1B2-C3D4-7E5F-8A6B-0C1D2E3F4A5B",
            "0190a1b2c3d47e5f8a6b0c1d2e3f4a5b",
            "0190A1B2C3D47E5F8A6B0C1D2E3F4A5B",
        ] {
            assert_eq!(text.parse::<ObjectId>().unwrap(), expected, "{text}");
        }
    }

    #[test]
    fn parsing_rejects_every_other_form() {
        for text in [
            "",
            "0190a1b2",
            "{0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b}",
            "urn:uuid:0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b",
            "0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5g",
            "0190a1b2c3d4-7e5f-8a6b-0c1d2e3f4a5b0",
            " 0190a1b2c3d47e5f8a6b0c1d2e3f4a5b",
            "0190a1b2c3d47e5f8a6b0c1d2e3f4a5\u{e9}",
        ] {
            let error = text.parse::<ObjectId>().unwrap_err();
            assert_eq!(error.code(), "base.id.invalid", "{text}");
        }
    }

    #[test]
    fn typed_identifiers_convert_only_explicitly() {
        let object = ObjectId::from_u128(77);
        let document = DocumentId::from_u128(object.as_u128());
        assert_eq!(document.as_u128(), 77);
        assert_eq!(
            TransactionId::from_u128(1).to_string(),
            SessionId::from_u128(1).to_string()
        );
    }

    #[test]
    fn identifiers_serialize_as_their_text_form() {
        let id = ObjectId::from_u128(0x0190_A1B2_C3D4_7E5F_8A6B_0C1D_2E3F_4A5B);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"0190a1b2-c3d4-7e5f-8a6b-0c1d2e3f4a5b\"");
        assert_eq!(serde_json::from_str::<ObjectId>(&json).unwrap(), id);
    }

    proptest! {
        #[test]
        fn parse_of_format_is_identity(raw in any::<u128>()) {
            let id = ObjectId::from_u128(raw);
            let text = id.to_string();
            prop_assert_eq!(text.parse::<ObjectId>().unwrap(), id);
            prop_assert_eq!(text.to_uppercase().parse::<ObjectId>().unwrap(), id);
            prop_assert_eq!(text.replace('-', "").parse::<ObjectId>().unwrap(), id);
        }

        #[test]
        fn order_follows_the_raw_value(a in any::<u128>(), b in any::<u128>()) {
            prop_assert_eq!(ObjectId::from_u128(a).cmp(&ObjectId::from_u128(b)), a.cmp(&b));
        }

        #[test]
        fn any_seed_and_clock_give_increasing_identifiers(
            seed in any::<u64>(),
            times in proptest::collection::vec(0_u64..5, 1..50),
        ) {
            let mut generator = IdGenerator::new(Replay(times.clone(), 0), SplitMix64(seed));
            let ids: Vec<ObjectId> = (0..times.len()).map(|_| generator.generate()).collect();
            prop_assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
