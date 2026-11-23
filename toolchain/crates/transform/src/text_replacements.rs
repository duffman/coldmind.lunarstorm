//! Literal substitutions on the raw text, before parsing. Also rewrites strings and comments.

use crate::source_edits::Edits;

/// Edits for every match of every pair, in pair order; a match overlapping an earlier one is skipped.
pub fn replace_edits(pairs: &[(String, String)], source: &str) -> Edits {
    let mut edits = Edits::default();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (from, to) in pairs {
        for (start, _) in source.match_indices(from.as_str()) {
            let end = start + from.len();
            if taken.iter().any(|&(s, e)| start < e && s < end) {
                continue;
            }
            taken.push((start, end));
            edits.replace(start as u32, end as u32, to.as_str());
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::replace_edits;

    fn apply(pairs: &[(&str, &str)], source: &str) -> String {
        let pairs: Vec<_> = pairs
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect();
        replace_edits(&pairs, source).apply(source)
    }

    #[test]
    fn earlier_pairs_win_overlaps() {
        assert_eq!(apply(&[("__A__", "1"), ("A", "2")], "__A__ A"), "1 2");
    }
}
