//! Span edits spliced into the original source.
//!
//! Hooks rewrite only what they match and keep every other byte, so untouched code keeps its
//! position. `onBeforeParse` cannot return a sourcemap; this keeps Bun's own map close to the file.

/// Byte-range replacements against one source text.
#[derive(Default)]
pub struct Edits {
    list: Vec<(u32, u32, String)>,
}

impl Edits {
    /// Replaces `start..end` (UTF-8 byte offsets) with `text`.
    pub fn replace(&mut self, start: u32, end: u32, text: impl Into<String>) {
        self.list.push((start, end, text.into()));
    }

    /// Inserts `text` at byte offset `at`.
    pub fn insert(&mut self, at: u32, text: impl Into<String>) {
        self.replace(at, at, text);
    }

    /// Moves every edit of `other` into `self`.
    pub fn extend(&mut self, other: Edits) {
        self.list.extend(other.list);
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Applies source-ordered edits, dropping overlaps against already consumed input.
    /// Sorting spans first makes each untouched byte copy exactly once; insertions
    /// preserve source-relative offsets rather than shifting later visitor spans.
    pub fn apply(mut self, source: &str) -> String {
        self.list.sort_by_key(|(start, end, _)| (*start, *end));
        let extra: usize = self.list.iter().map(|(_, _, text)| text.len()).sum();
        let mut output = String::with_capacity(source.len() + extra);
        let mut cursor = 0usize;
        for (start, end, text) in &self.list {
            let (start, end) = (*start as usize, *end as usize);
            if start < cursor || end > source.len() {
                continue;
            }
            output.push_str(&source[cursor..start]);
            output.push_str(text);
            cursor = end;
        }
        output.push_str(&source[cursor..]);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::Edits;

    #[test]
    fn splices_in_source_order() {
        let mut edits = Edits::default();
        edits.replace(6, 11, "there");
        edits.insert(0, ">");
        assert_eq!(edits.apply("hello world"), ">hello there");
    }

    #[test]
    fn drops_overlapping_edits() {
        let mut edits = Edits::default();
        edits.replace(0, 5, "A");
        edits.replace(3, 7, "B");
        assert_eq!(edits.apply("0123456789"), "A56789");
    }
}
