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

/// Speech models sometimes get stuck on near-silence and repeat one phrase over and over
/// ("Listening. Listening. Listening. …"). Collapses any run of 4+ identical consecutive phrases
/// (up to 8 words long) to a single copy, and reports the longest run it found so the caller can
/// treat an extreme run as a hallucination and discard it.
pub fn collapse_repeats(text: &str) -> (String, usize) {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let cores: Vec<String> = tokens.iter().map(|t| core(t)).collect();
    let (mut out, mut longest, mut i) = (Vec::new(), 1usize, 0usize);
    while i < tokens.len() {
        let mut collapsed = false;
        for period in 1..=8usize.min((tokens.len() - i) / 2) {
            let mut runs = 1;
            while i + (runs + 1) * period <= tokens.len() && cores[i..i + period] == cores[i + runs * period..i + (runs + 1) * period] {
                runs += 1;
            }
            if runs >= 4 {
                out.extend_from_slice(&tokens[i..i + period]);
                longest = longest.max(runs);
                i += runs * period;
                collapsed = true;
                break;
            }
        }
        if !collapsed {
            out.push(tokens[i]);
            i += 1;
        }
    }
    (out.join(" "), longest)
}

/// Text to insert when it continues what the previous dictation typed in the same place:
/// a leading space, unless it begins with closing punctuation that belongs to the previous words.
pub fn continuation(text: &str) -> String {
    if text.starts_with(['.', ',', ';', ':', '!', '?', ')']) {
        text.to_string()
    } else {
        format!(" {text}")
    }
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
    fn a_stuck_model_repeating_one_phrase_is_collapsed_and_measured() {
        let stuck = "Listening. ".repeat(110);
        assert_eq!(collapse_repeats(&stuck), ("Listening.".to_string(), 110));
        assert_eq!(collapse_repeats("thank you thank you thank you thank you so much"), ("thank you so much".to_string(), 4));
    }

    #[test]
    fn ordinary_repetition_is_left_alone() {
        for s in ["no no no", "It was very very good.", "bye bye", "one two three four"] {
            assert_eq!(collapse_repeats(s), (s.to_string(), 1));
        }
    }

    #[test]
    fn continuation_adds_a_space_unless_punctuation_leads() {
        assert_eq!(continuation("And then we left."), " And then we left.");
        assert_eq!(continuation(", right?"), ", right?");
        assert_eq!(continuation("?"), "?");
    }

    #[test]
    fn dangling_comma_becomes_a_period() {
        assert_eq!(polish(&["first of all,"]), "First of all.");
    }
}
