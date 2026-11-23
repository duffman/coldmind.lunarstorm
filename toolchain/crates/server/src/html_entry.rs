//! HTML pages: the React Refresh preamble and the HMR client go in before any app script.

/// React Refresh preamble: installs the global hook before React loads.
pub const PREAMBLE_SCRIPT: &str =
    "<script type=\"module\" src=\"/@devtools/refresh-preamble.js\"></script>";

/// The built-in HMR client, which opens the socket.
pub const CLIENT_SCRIPT: &str = "<script type=\"module\" src=\"/@devtools/client.js\"></script>";

/// Inserts the head scripts right after `<head ...>`, else after `<html ...>`, else at the top.
/// With `shell`, the app's `@coldmind/devtools-shell` owns the socket, so only the preamble goes in.
pub fn transform(html: &str, shell: bool) -> String {
    let lower = html.to_ascii_lowercase();
    let at = open_tag_end(&lower, "<head")
        .or_else(|| open_tag_end(&lower, "<html"))
        .unwrap_or(0);
    let scripts = if shell {
        PREAMBLE_SCRIPT.to_string()
    } else {
        format!("{PREAMBLE_SCRIPT}{CLIENT_SCRIPT}")
    };
    let mut out = String::with_capacity(html.len() + scripts.len());
    out.push_str(&html[..at]);
    out.push_str(&scripts);
    out.push_str(&html[at..]);
    out
}

/// Byte offset just past the `>` of the first `<tag` open tag.
fn open_tag_end(lower: &str, tag: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = lower[from..].find(tag) {
        let start = from + i;
        let next = lower.as_bytes().get(start + tag.len()).copied();
        if matches!(next, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')) {
            return lower[start..].find('>').map(|j| start + j + 1);
        }
        from = start + tag.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> String {
        format!("{PREAMBLE_SCRIPT}{CLIENT_SCRIPT}")
    }

    #[test]
    fn scripts_go_first_in_head() {
        let out = transform(
            "<!doctype html><html lang=\"en\"><head><title>x</title></head><body></body></html>",
            false,
        );
        assert!(out.contains(&format!("<head>{}<title>", all())), "{out}");
    }

    #[test]
    fn header_is_not_head_and_pages_without_head_still_get_scripts() {
        let out = transform("<html><body><header>h</header></body></html>", false);
        assert!(out.starts_with(&format!("<html>{}<body>", all())), "{out}");
        assert!(transform("<p>x</p>", false).starts_with(&all()));
    }

    #[test]
    fn shell_pages_get_only_the_preamble() {
        let out = transform("<html><head></head><body></body></html>", true);
        assert!(
            out.contains(&format!("<head>{PREAMBLE_SCRIPT}</head>")),
            "{out}"
        );
        assert!(!out.contains(CLIENT_SCRIPT), "{out}");
    }
}
