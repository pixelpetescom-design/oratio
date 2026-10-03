//! Pure text helpers behind "learning": turning a user's edit into replacement
//! rules, applying those rules, and building the hint prompt for the recogniser.

use crate::lexicon::Fix;

const MAX_SPAN: usize = 3;
const MAX_PROMPT_CHARS: usize = 300;

fn display(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric())
}

fn norm(token: &str) -> String {
    display(token).to_lowercase()
}

/// Word-level diff of `old` → `new`: every place where a short run of words was
/// replaced by another run becomes a `Fix`. Pure insertions/deletions teach nothing.
pub fn diff_fixes(old: &str, new: &str) -> Vec<Fix> {
    let a: Vec<&str> = old.split_whitespace().filter(|t| !display(t).is_empty()).collect();
    let b: Vec<&str> = new.split_whitespace().filter(|t| !display(t).is_empty()).collect();
    // Compare exact spelling (not lower-cased) so case-only fixes like iphone → iPhone are seen.
    let (na, nb): (Vec<&str>, Vec<&str>) = (a.iter().map(|t| display(t)).collect(), b.iter().map(|t| display(t)).collect());

    // Longest common subsequence on normalised words.
    let mut lcs = vec![vec![0usize; nb.len() + 1]; na.len() + 1];
    for i in (0..na.len()).rev() {
        for j in (0..nb.len()).rev() {
            lcs[i][j] = if na[i] == nb[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }

    let mut fixes = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut span_a, mut span_b) = (i, j);
    let mut flush = |sa: usize, i: usize, sb: usize, j: usize| {
        let (from_toks, to_toks) = (&a[sa..i], &b[sb..j]);
        if from_toks.is_empty() || to_toks.is_empty() || from_toks.len() > MAX_SPAN || to_toks.len() > MAX_SPAN {
            return;
        }
        let from = from_toks.iter().map(|t| norm(t)).collect::<Vec<_>>().join(" ");
        let to = to_toks.iter().map(|t| display(t)).collect::<Vec<_>>().join(" ");
        let case_only = from == to.to_lowercase();
        // "this" → "This" is just sentence-start capitalisation; only keep case fixes like "iPhone".
        let meaningful_case = to.chars().skip(1).any(char::is_uppercase);
        if from.chars().count() >= 2 && from != to && (!case_only || meaningful_case) {
            fixes.push(Fix { from, to });
        }
    };
    while i < na.len() && j < nb.len() {
        if na[i] == nb[j] {
            flush(span_a, i, span_b, j);
            i += 1;
            j += 1;
            (span_a, span_b) = (i, j);
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    flush(span_a, na.len(), span_b, nb.len());
    fixes
}

/// Applies learned replacements to whole words, case-insensitively, longest rule first.
pub fn apply_fixes(text: &str, fixes: &[Fix]) -> String {
    if fixes.is_empty() {
        return text.to_string();
    }
    // Line by line, so line breaks (spoken "new line", or a snippet's own) are preserved.
    text.split('\n').map(|line| apply_fixes_to_line(line, fixes)).collect::<Vec<_>>().join("\n")
}

fn apply_fixes_to_line(text: &str, fixes: &[Fix]) -> String {
    let mut rules: Vec<(Vec<&str>, &str)> = fixes.iter().map(|f| (f.from.split(' ').collect(), f.to.as_str())).collect();
    rules.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    'next: while i < tokens.len() {
        for (from, to) in &rules {
            let n = from.len();
            if i + n <= tokens.len() && tokens[i..i + n].iter().zip(from).all(|(t, f)| norm(t) == *f) {
                let first = tokens[i];
                let lead = &first[..first.find(display(first)).unwrap_or(0)];
                let last = tokens[i + n - 1];
                let trail = &last[last.rfind(display(last)).map_or(last.len(), |p| p + display(last).len())..];
                let mut replacement = (*to).to_string();
                let was_capitalised = display(first).chars().next().is_some_and(char::is_uppercase);
                if was_capitalised && !replacement.chars().any(char::is_uppercase) {
                    let mut c = replacement.chars();
                    replacement = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
                }
                out.push(format!("{lead}{replacement}{trail}"));
                i += n;
                continue 'next;
            }
        }
        out.push(tokens[i].to_string());
        i += 1;
    }
    out.join(" ")
}

/// Hint text for the recogniser: the user's own words, newest first, kept short enough
/// for the model's prompt window.
pub fn glossary_prompt<S: AsRef<str>>(words: &[S]) -> String {
    let mut list = String::new();
    for w in words {
        let w = w.as_ref().trim();
        if w.is_empty() {
            continue;
        }
        if list.len() + w.len() + 2 > MAX_PROMPT_CHARS {
            break;
        }
        if !list.is_empty() {
            list.push_str(", ");
        }
        list.push_str(w);
    }
    if list.is_empty() {
        String::new()
    } else {
        format!("Vocabulary: {list}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(from: &str, to: &str) -> Fix {
        Fix { from: from.into(), to: to.into() }
    }

    #[test]
    fn learns_a_one_word_correction() {
        assert_eq!(diff_fixes("Open the post is app.", "Open the Postiz app."), vec![fix("post is", "Postiz")]);
    }

    #[test]
    fn learns_several_corrections_in_one_edit() {
        let f = diff_fixes("we use cooper nettys and pie torch", "we use Kubernetes and PyTorch");
        assert_eq!(f, vec![fix("cooper nettys", "Kubernetes"), fix("pie torch", "PyTorch")]);
    }

    #[test]
    fn ignores_pure_insertions_deletions_and_punctuation_changes() {
        assert!(diff_fixes("hello world", "hello brave new world").is_empty());
        assert!(diff_fixes("hello big world", "hello world").is_empty());
        assert!(diff_fixes("hello world", "Hello, world!").is_empty());
    }

    #[test]
    fn sentence_capitalisation_is_not_a_rule_but_inner_capitals_are() {
        assert!(diff_fixes("this works", "This works").is_empty());
        assert_eq!(diff_fixes("my iphone broke", "my iPhone broke"), vec![fix("iphone", "iPhone")]);
    }

    #[test]
    fn applies_whole_words_only_and_keeps_punctuation() {
        let rules = [fix("post is", "Postiz")];
        assert_eq!(apply_fixes("I use post is, daily.", &rules), "I use Postiz, daily.");
        assert_eq!(apply_fixes("The compost is ready.", &rules), "The compost is ready.");
        assert_eq!(apply_fixes("Post is great.", &rules), "Postiz great.");
    }

    #[test]
    fn longest_rule_wins_and_capitalises_sentence_starts() {
        let rules = [fix("cooper", "Cooper"), fix("cooper nettys", "kubernetes")];
        assert_eq!(apply_fixes("Cooper nettys rocks.", &rules), "Kubernetes rocks.");
        assert_eq!(apply_fixes("no rules here", &[]), "no rules here");
    }

    #[test]
    fn glossary_prompt_is_bounded_and_skips_blanks() {
        assert_eq!(glossary_prompt::<&str>(&[]), "");
        assert_eq!(glossary_prompt(&["Postiz", " ", "Tauri"]), "Vocabulary: Postiz, Tauri.");
        let many: Vec<String> = (0..200).map(|i| format!("word{i}")).collect();
        assert!(glossary_prompt(&many).len() <= MAX_PROMPT_CHARS + 20);
    }
}
