//! Voice search: turning dictated text into a web search address.

/// The key held with the Ctrl+Win chord to search instead of type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKey {
    Shift,
    Alt,
}

/// What to look up: the dictated text without the full stop the polisher added.
pub fn query(text: &str) -> String {
    text.trim().trim_end_matches('.').trim().to_string()
}

/// A Google search address for `query`, safely percent-encoded.
pub fn google_url(query: &str) -> String {
    let mut out = String::from("https://www.google.com/search?q=");
    for b in query.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_trailing_full_stop_only() {
        assert_eq!(query("  What is the capital of Australia.  "), "What is the capital of Australia");
        assert_eq!(query("Is it raining?"), "Is it raining?");
    }

    #[test]
    fn encodes_spaces_and_special_characters() {
        assert_eq!(google_url("best pizza near me"), "https://www.google.com/search?q=best+pizza+near+me");
        assert_eq!(google_url("rust & c++ #1?"), "https://www.google.com/search?q=rust+%26+c%2B%2B+%231%3F");
    }

    #[test]
    fn encodes_non_ascii_as_utf8_bytes() {
        assert_eq!(google_url("café"), "https://www.google.com/search?q=caf%C3%A9");
    }

    #[test]
    fn cannot_inject_extra_url_parts() {
        let u = google_url("a&q=b#frag/../x");
        assert_eq!(u.matches('&').count(), 0);
        assert_eq!(u.matches('#').count(), 0);
        assert!(u.starts_with("https://www.google.com/search?q="));
    }
}
