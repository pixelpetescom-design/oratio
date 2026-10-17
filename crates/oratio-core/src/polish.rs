//! Deterministic, offline text clean-up applied to the joined transcript:
//! fillers removed, "i" fixed, sentences capitalised, spacing and terminal
//! punctuation normalised. The recogniser already supplies most punctuation.

/// "um", "uh", "er", "erm", "hmm", "mm" however many letters the recogniser stretches them to ("ummm", "errr").
fn is_filler(word: &str) -> bool {
    let squeezed: String = {
        let mut out = String::new();
        for c in word.chars() {
            if !out.ends_with(c) {
                out.push(c);
            }
        }
        out
    };
    if word == "err" {
        return false; // a real word: "err on the side of caution"
    }
    let is_mm = word.len() >= 2 && word.chars().all(|c| c == 'm');
    is_mm || matches!(squeezed.as_str(), "um" | "uh" | "uhm" | "er" | "erm" | "hm" | "hmm" | "ehm")
}

/// Small words people stumble on ("I, I, I think"). Words that are legitimately doubled ("that that",
/// "had had") are deliberately not here.
const STUTTERS: &[&str] = &["i", "the", "a", "an", "and", "to", "we", "you", "it", "is", "in", "of", "so", "but", "my", "he", "she", "they", "if", "or", "for", "on", "at", "with", "as", "this", "there"];

/// A sentence that ends on one of these has trailed off, so it gets no full stop: whatever is said
/// next carries on from it.
const DANGLING: &[&str] = &["and", "but", "or", "because", "the", "a", "an", "of", "to", "which", "if", "when", "while", "as", "than", "my", "your", "our", "their", "his", "her", "its", "this", "these", "those"];

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
        if is_filler(&c) {
            if let (Some(end), Some(prev)) = (raw.chars().last().filter(|c| is_terminal(*c)), tokens.last_mut()) {
                if !prev.ends_with(is_terminal) {
                    prev.push(end);
                }
            }
            continue;
        }
        // A stumble ("I, I, I think"): keep only the last of the repeats, which carries the real punctuation.
        if STUTTERS.contains(&c.as_str()) && tokens.last().is_some_and(|prev| core(prev) == c) {
            tokens.pop();
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
        Some(c) if c.is_alphanumeric() || c == ')' || c == '"' => {
            let last_word = out.split_whitespace().last().map(core).unwrap_or_default();
            if !DANGLING.contains(&last_word.as_str()) {
                out.push('.');
            }
        }
        Some(_) => {}
    }
    out
}

/// Words that keep their capital in the middle of a sentence: "I", acronyms, names in the user's
/// dictionary, days and months, and anything with a capital inside ("iPhone").
fn keeps_capital(word: &str, words: &[String]) -> bool {
    let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');
    let lower = bare.to_lowercase();
    const NAMES: &[&str] = &[
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january", "february", "march", "april", "june", "july", "august", "september", "october",
        "november", "december", "australia", "australian", "english",
    ];
    bare == "I"
        || bare.starts_with("I'")
        || (bare.chars().count() >= 2 && bare.chars().all(|c| !c.is_lowercase()))
        || bare.chars().skip(1).any(char::is_uppercase)
        || NAMES.contains(&lower.as_str())
        || words.iter().any(|w| w.eq_ignore_ascii_case(bare) && w.chars().any(char::is_uppercase))
}

fn lower_first(text: &str, words: &[String]) -> String {
    let Some(first) = text.split_whitespace().next() else { return text.to_string() };
    if keeps_capital(first, words) {
        return text.to_string();
    }
    let at = text.find(|c: char| c.is_alphabetic()).unwrap_or(0);
    let mut chars = text[at..].chars();
    match chars.next() {
        Some(c) => format!("{}{}{}", &text[..at], c.to_lowercase(), chars.as_str()),
        None => text.to_string(),
    }
}

/// Fits new dictation onto what is already on screen: if the previous text stopped mid-sentence
/// (no full stop, or a comma), the new words carry on in lower case; after a full stop they start a sentence.
pub fn follow_on(previous: &str, text: &str, words: &[String]) -> String {
    let before = previous.trim_end().trim_end_matches(['"', '\'', ')', ']']);
    let carries_on = !previous.ends_with(['\n', '\r']) && before.chars().last().is_some_and(|c| !matches!(c, '.' | '!' | '?'));
    if carries_on && !text.starts_with('\n') {
        lower_first(text, words)
    } else {
        text.to_string()
    }
}

/// The relaxed style for chat apps: starts in lower case and has no closing full stop.
pub fn casual(text: &str, words: &[String]) -> String {
    let trimmed = text.trim_end();
    let without_stop = trimmed.strip_suffix('.').filter(|rest| !rest.ends_with('.')).unwrap_or(trimmed);
    if without_stop.starts_with('\n') {
        without_stop.to_string()
    } else {
        lower_first(without_stop, words)
    }
}

/// Speech models sometimes get stuck on near-silence and repeat one phrase over and over
/// ("Listening. Listening. Listening. ..." or "the name of the name of the name of ..."). A short
/// phrase repeated 4+ times, or a 3-8 word phrase repeated 3+ times, is collapsed to one copy; a run of
/// 8+ is certainly a loop, so it is cut out entirely and the real words around it are kept.
/// Returns the cleaned text and the longest run found.
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
            // One or two words repeated ("no no no") can be genuine emphasis, so those need 4+; a longer
            // phrase said 3 times in a row is almost always the model stuck.
            if runs >= if period <= 2 { 4 } else { 3 } {
                if runs < LOOP_RUN {
                    out.extend_from_slice(&tokens[i..i + period]);
                }
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

/// A repeat this long is a hallucination, not speech.
pub const LOOP_RUN: usize = 8;

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
    fn a_sentence_that_trails_off_gets_no_full_stop() {
        assert_eq!(polish(&["we went to the shop and"]), "We went to the shop and");
        assert_eq!(polish(&["I think that it is because"]), "I think that it is because");
        assert_eq!(polish(&["I think so"]), "I think so.");
        assert_eq!(polish(&["what are you looking for"]), "What are you looking for.");
    }

    #[test]
    fn new_dictation_carries_on_from_the_previous_text() {
        let none: [String; 0] = [];
        assert_eq!(follow_on("We went to the shop and", "Bought some milk.", &none), "bought some milk.");
        assert_eq!(follow_on("Dear John,", "Thanks for the note.", &none), "thanks for the note.");
        assert_eq!(follow_on("That was it.", "Then we left.", &none), "Then we left.");
        assert_eq!(follow_on("What now?", "Let us go.", &none), "Let us go.");
        assert_eq!(follow_on("Line one\n", "Next.", &none), "Next.");
        assert_eq!(follow_on("We went to the shop and", "I bought milk.", &none), "I bought milk.");
        assert_eq!(follow_on("Then we met", "Peter at the station.", &["Peter".to_string()]), "Peter at the station.");
        assert_eq!(follow_on("Then we met", "NASA staff.", &none), "NASA staff.");
        assert_eq!(follow_on("Then we met", "Friday morning.", &none), "Friday morning.");
        assert_eq!(follow_on("He said \"stop.\"", "Then left.", &none), "Then left.", "a closing quote after a full stop still ends the sentence");
    }

    #[test]
    fn casual_style_drops_the_capital_and_the_full_stop() {
        let none: [String; 0] = [];
        assert_eq!(casual("Sounds good, see you at six.", &none), "sounds good, see you at six");
        assert_eq!(casual("Are you coming?", &none), "are you coming?");
        assert_eq!(casual("I will be there.", &none), "I will be there");
        assert_eq!(casual("Wait... what.", &none), "wait... what");
    }

    #[test]
    fn removes_stretched_fillers_but_not_real_words() {
        assert_eq!(polish(&["Ummm, so errr, I think mmm hmm we should go"]), "So I think we should go.");
        assert_eq!(polish(&["We should err on the side of caution"]), "We should err on the side of caution.");
        assert_eq!(polish(&["Um, hello there"]), "Hello there.");
    }

    #[test]
    fn collapses_stutters_but_keeps_real_doubles() {
        assert_eq!(polish(&["I, I, I think we should, we should go"]), "I think we should, we should go.");
        assert_eq!(polish(&["So the the plan is to to wait"]), "So the plan is to wait.");
        assert_eq!(polish(&["I said that that was fine and he had had enough"]), "I said that that was fine and he had had enough.");
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
        assert_eq!(collapse_repeats(&stuck), (String::new(), 110), "a long loop is cut out entirely");
        assert_eq!(collapse_repeats("Listening. Listening. Listening. Listening. Listening."), ("Listening.".to_string(), 5));
        assert_eq!(collapse_repeats("thank you thank you thank you thank you so much"), ("thank you so much".to_string(), 4));
    }

    #[test]
    fn a_loop_in_the_middle_of_real_words_is_cut_out_and_the_words_survive() {
        let said = format!("Peter, space. And my address. {}Peter, space. And then my address.", "The name of the ".repeat(60));
        assert_eq!(collapse_repeats(&said), ("Peter, space. And my address. Peter, space. And then my address.".to_string(), 60));
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
