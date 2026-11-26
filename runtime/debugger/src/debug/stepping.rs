//! URI globs used only for deciding step stops; never for hiding real pauses.
use super::{DebugError, DebugLimits, StepPolicy};

const MATCH_CELLS: usize = 65_536;

pub(super) fn validate(policy: &StepPolicy, limits: &DebugLimits) -> Result<(), DebugError> {
    if policy.skip_sources.len() > 64 {
        return Err(DebugError::Capacity);
    }
    let mut bytes = policy
        .skip_sources
        .capacity()
        .checked_mul(std::mem::size_of::<String>())
        .ok_or(DebugError::Capacity)?;
    for pattern in &policy.skip_sources {
        if pattern.is_empty() || pattern.contains("***") || pattern.chars().any(char::is_control) {
            return Err(DebugError::InvalidSource);
        }
        bytes = bytes
            .checked_add(pattern.capacity())
            .ok_or(DebugError::Capacity)?;
        if bytes > limits.condition_bytes {
            return Err(DebugError::Capacity);
        }
    }
    Ok(())
}

pub(super) fn matches(policy: &StepPolicy, origin: &str) -> Result<bool, DebugError> {
    let mut remaining = MATCH_CELLS;
    for pattern in &policy.skip_sources {
        if pattern.is_empty() {
            return Err(DebugError::InvalidSource);
        }
        let width = origin.len().checked_add(1).ok_or(DebugError::Capacity)?;
        let cells = pattern
            .len()
            .checked_mul(width)
            .ok_or(DebugError::Capacity)?;
        remaining = remaining.checked_sub(cells).ok_or(DebugError::Capacity)?;
        let row = || {
            let mut row = Vec::new();
            row.try_reserve_exact(width)
                .map_err(|_| DebugError::Capacity)?;
            row.resize(width, false);
            Ok::<_, DebugError>(row)
        };
        let mut previous = row()?;
        let mut next = row()?;
        previous[0] = true;
        let mut pattern = pattern.as_bytes().iter().copied().peekable();
        while let Some(token) = pattern.next() {
            next.fill(false);
            if token == b'*' {
                let cross_slash = pattern.peek() == Some(&b'*');
                if cross_slash {
                    pattern.next();
                }
                next[0] = previous[0];
                for (index, byte) in origin.bytes().enumerate() {
                    next[index + 1] =
                        previous[index + 1] || (next[index] && (cross_slash || byte != b'/'));
                }
            } else {
                for (index, byte) in origin.bytes().enumerate() {
                    next[index + 1] = previous[index] && byte == token;
                }
            }
            std::mem::swap(&mut previous, &mut next);
        }
        if previous[origin.len()] {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stars_distinguish_path_segments_and_keep_unicode_literal() {
        let policy = |pattern: &str| StepPolicy {
            skip_sources: vec![pattern.into()],
        };
        assert!(matches(&policy("file:///lib/*.js"), "file:///lib/a.js").unwrap());
        assert!(!matches(&policy("file:///lib/*.js"), "file:///lib/sub/a.js").unwrap());
        assert!(matches(&policy("**/lib/**"), "file:///lib/sub/🔥.js").unwrap());
        assert!(matches(&policy("**/🔥*.js"), "file:///lib/🔥x.js").unwrap());
        assert!(!matches(&policy("**/foo*bar"), "file:///foo/a").unwrap());
        assert!(matches(&policy("**/foo*bar"), "file:///foo/sub/fooXbar").unwrap());
        assert!(!matches(&policy("file:///LIB/**"), "file:///lib/a.js").unwrap());
    }
    #[test]
    fn matching_and_policy_storage_have_explicit_limits() {
        let policy = StepPolicy {
            skip_sources: vec!["*".repeat(3)],
        };
        assert!(matches!(
            validate(&policy, &DebugLimits::default()),
            Err(DebugError::InvalidSource)
        ));
        let policy = StepPolicy {
            skip_sources: vec!["a".repeat(1024)],
        };
        assert!(matches!(
            matches(&policy, &"a".repeat(1024)),
            Err(DebugError::Capacity)
        ));
    }
}
