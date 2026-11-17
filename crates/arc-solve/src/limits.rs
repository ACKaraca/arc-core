//! The limits of parsing and evaluation (CORE-SOLVE-021).

use arc_base::Limits;

/// The bounds every parse and evaluation enforces. Exceeding one fails with
/// `solve.expr.limit_exceeded`, whose detail `limit` names it (`text_bytes`, `depth`,
/// `list_length`, `steps` or `dimension_exponent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprLimits {
    /// The length in bytes of the input text and of every text value.
    pub text_bytes: usize,
    /// The nesting depth of the parsed expression and of list values; capped at
    /// [`ExprLimits::MAX_DEPTH`].
    pub depth: u32,
    /// The number of elements of a list and of arguments of a call.
    pub list_length: usize,
    /// The number of evaluation steps: one per evaluated node, list element and admitted value.
    pub steps: u64,
}

impl ExprLimits {
    /// The highest depth any limit allows (the nesting depth of the loosest `arc-base` limits
    /// profile), so that parsing and evaluation never exhaust the stack, including on WebAssembly.
    /// At this depth they need less than 1 MiB of stack in unoptimized builds and less than
    /// 256 KiB in optimized ones.
    pub const MAX_DEPTH: u32 = 128;

    /// The defaults: 64 KiB of text, depth 64 (as `arc_base::Limits::DEFAULT.nesting_depth`),
    /// 10,000 list elements and 1,000,000 steps.
    pub const DEFAULT: Self = Self {
        text_bytes: 64 * 1024,
        depth: 64,
        list_length: 10_000,
        steps: 1_000_000,
    };

    /// These limits lowered to a hostile-input profile: text to its string length, depth to its
    /// nesting depth and list length to its array length.
    #[must_use]
    pub fn tightened_by(self, limits: &Limits) -> Self {
        let size = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
        Self {
            text_bytes: self.text_bytes.min(size(limits.string_bytes)),
            depth: self.depth.min(limits.nesting_depth),
            list_length: self.list_length.min(size(limits.array_length)),
            steps: self.steps,
        }
    }

    /// The depth limit in force: [`ExprLimits::depth`] capped at [`ExprLimits::MAX_DEPTH`].
    #[must_use]
    pub fn effective_depth(&self) -> u32 {
        self.depth.min(Self::MAX_DEPTH)
    }
}

impl Default for ExprLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tightening_takes_the_lower_limits_and_depth_is_capped() {
        let strict = ExprLimits::DEFAULT.tightened_by(&Limits::STRICT);
        assert_eq!(strict.depth, 32);
        assert_eq!(strict.text_bytes, ExprLimits::DEFAULT.text_bytes);
        let deep = ExprLimits {
            depth: u32::MAX,
            ..ExprLimits::DEFAULT
        };
        assert_eq!(deep.effective_depth(), ExprLimits::MAX_DEPTH);
        assert_eq!(ExprLimits::default().effective_depth(), 64);
    }
}
