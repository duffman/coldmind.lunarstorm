//! Pure parsing and matching for logical breakpoint hit counts.

use super::DebugError;
use std::num::NonZeroU64;

/// A breakpoint hit policy with a positive threshold or divisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitCondition {
    /// Match exactly the numbered hit.
    Equal(NonZeroU64),
    /// Match the numbered hit and every later hit.
    AtLeast(NonZeroU64),
    /// Match positive multiples of the divisor.
    Multiple(NonZeroU64),
}

impl HitCondition {
    /// Parse `N`, `==N`, `>=N`, or `%N`, where `N` contains only ASCII
    /// decimal digits and has a value in `1..=u64::MAX`.
    ///
    /// Outer whitespace is trimmed; whitespace within the expression is invalid.
    /// Leading zeroes are accepted when the resulting value is positive.
    /// Bare `N` and `==N` both select exactly the numbered hit.
    /// Invalid syntax, zero, and overflow return [`DebugError::InvalidRequest`].
    pub fn parse(expression: &str) -> Result<Self, DebugError> {
        let expression = expression.trim();
        let (digits, constructor): (_, fn(NonZeroU64) -> Self) =
            if let Some(digits) = expression.strip_prefix("==") {
                (digits, Self::Equal)
            } else if let Some(digits) = expression.strip_prefix(">=") {
                (digits, Self::AtLeast)
            } else if let Some(digits) = expression.strip_prefix('%') {
                (digits, Self::Multiple)
            } else {
                (expression, Self::Equal)
            };
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return Err(DebugError::InvalidRequest);
        }
        let value = digits
            .parse::<u64>()
            .ok()
            .and_then(NonZeroU64::new)
            .ok_or(DebugError::InvalidRequest)?;
        Ok(constructor(value))
    }

    /// Test a logical breakpoint's hit count. A count of zero never matches.
    pub fn matches(self, hits: u64) -> bool {
        hits != 0
            && match self {
                Self::Equal(value) => hits == value.get(),
                Self::AtLeast(value) => hits >= value.get(),
                Self::Multiple(value) => hits.is_multiple_of(value.get()),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::HitCondition;
    use crate::debug::DebugError;
    use std::num::NonZeroU64;

    #[test]
    fn parses_positive_decimal_grammar() {
        for (text, expected) in [
            ("3", HitCondition::Equal(NonZeroU64::new(3).unwrap())),
            ("==3", HitCondition::Equal(NonZeroU64::new(3).unwrap())),
            (">=3", HitCondition::AtLeast(NonZeroU64::new(3).unwrap())),
            ("%3", HitCondition::Multiple(NonZeroU64::new(3).unwrap())),
            (
                " \t==0003\r\n",
                HitCondition::Equal(NonZeroU64::new(3).unwrap()),
            ),
        ] {
            assert_eq!(HitCondition::parse(text), Ok(expected), "{text:?}");
        }
    }

    #[test]
    fn equal_matches_only_the_positive_nth_hit() {
        for text in ["3", "==3"] {
            let policy = HitCondition::parse(text).unwrap();
            for (hits, expected) in [
                (0, false),
                (1, false),
                (2, false),
                (3, true),
                (4, false),
                (6, false),
                (9, false),
            ] {
                assert_eq!(policy.matches(hits), expected, "{text} at {hits}");
            }
        }
    }

    #[test]
    fn at_least_includes_the_positive_threshold() {
        let policy = HitCondition::parse(">=3").unwrap();
        for (hits, expected) in [
            (0, false),
            (1, false),
            (2, false),
            (3, true),
            (4, true),
            (6, true),
            (9, true),
        ] {
            assert_eq!(policy.matches(hits), expected, "at {hits}");
        }
    }

    #[test]
    fn multiple_matches_only_positive_multiples() {
        let policy = HitCondition::parse("%3").unwrap();
        for (hits, expected) in [
            (0, false),
            (1, false),
            (2, false),
            (3, true),
            (4, false),
            (6, true),
            (9, true),
        ] {
            assert_eq!(policy.matches(hits), expected, "at {hits}");
        }
    }

    #[test]
    fn rejects_zero_malformed_and_overflow() {
        for text in [
            "",
            " ",
            "0",
            "000",
            "==0",
            ">=0",
            "%0",
            "+1",
            "-1",
            "1.0",
            "1e2",
            "0x10",
            "1_000",
            "١",
            "１",
            "==",
            ">=",
            "%",
            "=1",
            ">1",
            "<=1",
            "!=1",
            "%%1",
            "===1",
            ">==1",
            "== 1",
            ">=\t1",
            "% 1",
            "1 2",
            "1\n2",
            "1%",
            "18446744073709551616",
            "==18446744073709551616",
            ">=18446744073709551616",
            "%18446744073709551616",
        ] {
            assert_eq!(
                HitCondition::parse(text),
                Err(DebugError::InvalidRequest),
                "{text:?}"
            );
        }
    }

    #[test]
    fn matches_one_and_u64_max_without_overflow() {
        for prefix in ["", "==", ">=", "%"] {
            let one = HitCondition::parse(&format!("{prefix}1")).unwrap();
            assert!(!one.matches(0));
            assert!(one.matches(1));
            assert_eq!(one.matches(2), matches!(prefix, ">=" | "%"));
            assert_eq!(one.matches(u64::MAX), matches!(prefix, ">=" | "%"));

            let maximum = HitCondition::parse(&format!("{prefix}{}", u64::MAX)).unwrap();
            assert!(!maximum.matches(0));
            assert!(!maximum.matches(1));
            assert!(!maximum.matches(u64::MAX - 1));
            assert!(maximum.matches(u64::MAX));
        }
        let near_maximum = HitCondition::parse(&format!("%{}", u64::MAX - 1)).unwrap();
        assert!(near_maximum.matches(u64::MAX - 1));
        assert!(!near_maximum.matches(u64::MAX));
    }
}
