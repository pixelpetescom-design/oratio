//! Self-corrections while speaking: "meet tomorrow, no wait, Friday" is written as "meet Friday".
//!
//! Only unmistakable spoken markers are used ("no wait", "I meant", "or rather"), because a marker
//! like plain "actually" or "I mean" appears in ordinary sentences and would eat real words.

/// Words that introduce a phrase ("at five", "on Friday") rather than being part of the thing being corrected.
const FUNCTION: &[&str] = &["at", "on", "in", "to", "for", "by", "from", "with", "of", "the", "a", "an", "about", "around", "before", "after"];

const MARKERS: &[&[&str]] = &[&["sorry", "i", "meant"], &["no", "wait"], &["wait", "no"], &["i", "meant"], &["or", "rather"]];

/// A fragment this short or shorter replaces just the last words said; a longer one restates the whole clause.
const REPLACE_WORDS: usize = 3;

fn plain(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase()
}

fn ends_sentence(token: &str) -> bool {
    token.ends_with(['.', '!', '?'])
}

fn marker_at(tokens: &[&str], i: usize) -> Option<usize> {
    MARKERS.iter().find(|m| i + m.len() < tokens.len() && m.iter().enumerate().all(|(k, w)| plain(tokens[i + k]) == *w)).map(|m| m.len())
}

/// Applies every self-correction in `text`.
pub fn apply(text: &str) -> String {
    let mut current = text.to_string();
    // Each pass removes one marker; a handful is plenty for speech.
    for _ in 0..4 {
        let next = apply_once(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn apply_once(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for i in 1..tokens.len() {
        // The marker must start a clause: after a comma or a sentence end, as in "..., no wait, ...".
        if !(tokens[i - 1].ends_with([',', '.', '!', '?', '-', '—']) || ends_sentence(tokens[i - 1])) {
            continue;
        }
        let Some(len) = marker_at(&tokens, i) else { continue };

        let mut before: Vec<String> = tokens[..i].iter().map(|t| t.to_string()).collect();
        if let Some(last) = before.last_mut() {
            *last = last.trim_end_matches([',', '-', '—']).to_string();
        }
        let rest = &tokens[i + len..];
        let end = rest.iter().position(|t| ends_sentence(t)).map_or(rest.len(), |p| p + 1);
        let (fragment, after) = rest.split_at(end);
        if fragment.is_empty() {
            continue;
        }

        // The clause being corrected: back to the previous sentence end.
        let clause_start = before.iter().rposition(|t| ends_sentence(t)).map_or(0, |p| p + 1);
        let clause = &before[clause_start..];
        let keep = if fragment.len() > REPLACE_WORDS {
            // A whole new statement replaces the whole clause.
            clause_start
        } else {
            let starts_with_function = FUNCTION.contains(&plain(fragment[0]).as_str());
            // Words after the clause's last function word are the "thing" ("five" in "at five").
            let thing = clause.iter().rposition(|t| FUNCTION.contains(&plain(t).as_str())).map_or(clause.len(), |p| clause.len() - p - 1);
            let replaced = if starts_with_function { (fragment.len() - 1).min(thing).max(1).min(clause.len()) } else { fragment.len().min(thing.max(1)).min(clause.len()) };
            let mut keep = before.len() - replaced;
            if starts_with_function && keep > clause_start && FUNCTION.contains(&plain(&before[keep - 1]).as_str()) {
                keep -= 1; // "at five" → "at six": the new phrase brings its own "at"
            }
            keep
        };
        let mut out: Vec<String> = before[..keep].to_vec();
        out.extend(fragment.iter().map(|t| t.to_string()));
        out.extend(after.iter().map(|t| t.to_string()));
        return out.join(" ");
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_correction_replaces_the_last_words() {
        assert_eq!(apply("We should meet tomorrow, no wait, Friday."), "We should meet Friday.");
        assert_eq!(apply("Send it to John, I meant Jane."), "Send it to Jane.");
        assert_eq!(apply("Use the red one, or rather the blue one"), "Use the blue one");
    }

    #[test]
    fn a_new_phrase_keeps_or_swaps_its_preposition() {
        assert_eq!(apply("Call me at five, no wait, six thirty."), "Call me at six thirty.");
        assert_eq!(apply("Call me at five, no wait, at six."), "Call me at six.");
        assert_eq!(apply("Meet tomorrow, wait no, on Friday."), "Meet on Friday.");
    }

    #[test]
    fn a_long_restatement_replaces_the_whole_clause() {
        assert_eq!(apply("I will see you tomorrow, no wait, I will see you on Friday at noon."), "I will see you on Friday at noon.");
        assert_eq!(apply("Thanks for that. It costs ten, no wait, it costs a lot more than that."), "Thanks for that. it costs a lot more than that.");
    }

    #[test]
    fn ordinary_sentences_are_untouched() {
        for s in ["Wait, no one told me.", "I mean it.", "No wait is needed here.", "She said no, wait for me.", "Actually that works."] {
            assert_eq!(apply(s), s, "{s}");
        }
        assert_eq!(apply("Hello, no wait"), "Hello, no wait", "nothing after the marker to replace with");
    }
}
