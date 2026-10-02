//! Deterministic, offline text clean-up applied to the joined transcript:
//! fillers removed, "i" fixed, sentences capitalised, spacing and terminal
//! punctuation normalised. The recogniser already supplies most punctuation.

const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "erm", "er", "hmm", "mm"];

fn core(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase()
}

fn is_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

pub fn polish<S: AsRef<str>>(segments: &[S]) -> String {
    let joined = segments.iter().map(|s| s.as_ref().trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");

    // Tokenise on whitespace, dropping fillers but keeping any sentence end they carried.
    let mut tokens: Vec<String> = Vec::new();
    for raw in joined.split_whitespace() {
        let c = core(raw);
        if FILLERS.contains(&c.as_str()) {
            if let (Some(end), Some(prev)) = (raw.chars().last().filter(|c| is_terminal(*c)), tokens.last_mut()) {
                if !prev.ends_with(is_terminal) {
                    prev.push(end);
                }
            }
            continue;
        }
        let fixed = if c == "i" || c.starts_with("i'") {
            let mut s = raw.to_string();
            if let Some(i) = s.char_indices().find(|(_, ch)| *ch == 'i').map(|(i, _)| i) {
                s.replace_range(i..i + 1, "I");
            }
            s
        } else {
            raw.to_string()
        };
        tokens.push(fixed);
    }
    let mut text = tokens.join(" ");

    // No space before closing punctuation.
    for p in [',', '.', ';', ':', '!', '?'] {
        text = text.replace(&format!(" {p}"), &p.to_string());
    }

    // Capitalise the first letter and the first letter after a sentence end.
    let mut out = String::with_capacity(text.len() + 1);
    let mut cap = true;
    for ch in text.chars() {
        if cap && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            cap = false;
        } else {
            if is_terminal(ch) {
                cap = true;
            } else if ch.is_alphanumeric() {
                cap = false;
            }
            out.push(ch);
        }
    }

    // Terminal punctuation.
    match out.chars().last() {
        None => {}
        Some(c) if is_terminal(c) => {}
        Some(',' | ';' | ':') => {
            out.pop();
            out.push('.');
        }
        Some(c) if c.is_alphanumeric() || c == ')' || c == '"' => out.push('.'),
        Some(_) => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stays_empty() {
        assert_eq!(polish::<&str>(&[]), "");
        assert_eq!(polish(&["  ", ""]), "");
    }

    #[test]
    fn capitalises_and_punctuates() {
        assert_eq!(polish(&["hello there how are you"]), "Hello there how are you.");
    }

    #[test]
    fn joins_segments_and_keeps_existing_punctuation() {
        assert_eq!(polish(&["Hello there.", "how are you?"]), "Hello there. How are you?");
    }

    #[test]
    fn fixes_the_pronoun_i() {
        assert_eq!(polish(&["yes i think i'm ready and i'll go"]), "Yes I think I'm ready and I'll go.");
        assert_eq!(polish(&["it is in India"]), "It is in India.");
    }

    #[test]
    fn removes_fillers() {
        assert_eq!(polish(&["so um I think uh we should go"]), "So I think we should go.");
        assert_eq!(polish(&["we are done. Um."]), "We are done.");
    }

    #[test]
    fn tidies_spacing_before_punctuation() {
        assert_eq!(polish(&["wait , what ?"]), "Wait, what?");
    }

    #[test]
    fn dangling_comma_becomes_a_period() {
        assert_eq!(polish(&["first of all,"]), "First of all.");
    }
}
