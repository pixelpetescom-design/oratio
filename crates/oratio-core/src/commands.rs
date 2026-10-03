//! Spoken commands, applied to the finished text: "new line", "full stop", "question mark"…,
//! plus whole-utterance actions: "scratch that" (undo the last dictation) and a trailing
//! "press enter". Deliberately conservative: only phrases that are very unlikely in ordinary
//! speech ("period" and "quote" alone are NOT commands).

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub text: String,
    /// "scratch that": drop this dictation and undo the previous one.
    pub scratch: bool,
    /// A trailing "press enter": send Enter after typing.
    pub enter: bool,
}

#[derive(Clone, Copy)]
enum Piece {
    Punct(&'static str),
    /// Opening bracket/quote: no space after it.
    Open(&'static str),
    Break(usize),
}

const TWO_WORD: &[(&str, &str, Piece)] = &[
    ("new", "line", Piece::Break(1)),
    ("new", "paragraph", Piece::Break(2)),
    ("full", "stop", Piece::Punct(".")),
    ("question", "mark", Piece::Punct("?")),
    ("exclamation", "mark", Piece::Punct("!")),
    ("exclamation", "point", Piece::Punct("!")),
    ("open", "bracket", Piece::Open("(")),
    ("close", "bracket", Piece::Punct(")")),
    ("open", "quote", Piece::Open("\"")),
    ("close", "quote", Piece::Punct("\"")),
];
const ONE_WORD: &[(&str, Piece)] = &[
    ("newline", Piece::Break(1)),
    ("comma", Piece::Punct(",")),
    ("colon", Piece::Punct(":")),
    ("semicolon", Piece::Punct(";")),
];
const SCRATCH: &[&str] = &["scratch that", "delete that", "undo that", "cancel that"];

fn core(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

fn capitalise(word: &str) -> String {
    let mut done = false;
    word.chars()
        .map(|c| {
            if !done && c.is_alphabetic() {
                done = true;
                c.to_uppercase().collect::<String>()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub fn apply(text: &str) -> Parsed {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    let cores: Vec<String> = tokens.iter().map(|t| core(t)).collect();

    if SCRATCH.contains(&cores.join(" ").as_str()) {
        return Parsed { text: String::new(), scratch: true, enter: false };
    }

    let mut enter = false;
    if cores.len() >= 2 && cores[cores.len() - 2] == "press" && cores[cores.len() - 1] == "enter" {
        enter = true;
        tokens.truncate(tokens.len() - 2);
    }
    let cores: Vec<String> = tokens.iter().map(|t| core(t)).collect();

    let mut out = String::new();
    let mut capitalise_next = false;
    let mut glue_next = false;
    let mut i = 0;
    while i < tokens.len() {
        let two = cores.get(i + 1).and_then(|next| TWO_WORD.iter().find(|(a, b, _)| *a == cores[i] && b == next));
        let piece = if let Some((_, _, p)) = two {
            i += 2;
            Some(*p)
        } else if let Some((_, p)) = ONE_WORD.iter().find(|(w, _)| *w == cores[i]) {
            i += 1;
            Some(*p)
        } else {
            None
        };
        match piece {
            Some(Piece::Punct(p)) => {
                out.truncate(out.trim_end().len());
                // A spoken mark replaces one the recogniser already guessed, rather than doubling it.
                if matches!(p, "." | "?" | "!" | "," | ":" | ";") && out.ends_with(['.', '?', '!', ',', ':', ';']) {
                    out.pop();
                }
                out.push_str(p);
                capitalise_next = matches!(p, "." | "?" | "!");
                glue_next = false;
            }
            Some(Piece::Open(p)) => {
                if !out.is_empty() && !out.ends_with(['\n', '(', ' ']) {
                    out.push(' ');
                }
                out.push_str(p);
                glue_next = true;
            }
            Some(Piece::Break(n)) => {
                out.truncate(out.trim_end_matches(' ').len());
                out.push_str(&"\n".repeat(n));
                capitalise_next = true;
                glue_next = false;
            }
            None => {
                let word = if capitalise_next { capitalise(tokens[i]) } else { tokens[i].to_string() };
                if !out.is_empty() && !glue_next && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(&word);
                capitalise_next = false;
                glue_next = false;
                i += 1;
            }
        }
    }
    let mut text = out.trim_end().to_string();
    if enter {
        // "Send it, press enter." leaves a dangling comma; end cleanly instead.
        while text.ends_with([',', ';', ':']) {
            text.pop();
        }
    }
    Parsed { text, scratch: false, enter }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> String {
        apply(s).text
    }

    #[test]
    fn punctuation_words_become_punctuation() {
        assert_eq!(t("Hello comma how are you question mark"), "Hello, how are you?");
        assert_eq!(t("Stop full stop Next thing"), "Stop. Next thing");
        assert_eq!(t("Wow exclamation mark"), "Wow!");
    }

    #[test]
    fn works_with_the_punctuation_the_recogniser_already_added() {
        assert_eq!(t("Hello. Full stop. Next."), "Hello. Next.");
        assert_eq!(t("Dear John, new line, thanks."), "Dear John,\nThanks.");
    }

    #[test]
    fn new_line_and_new_paragraph() {
        assert_eq!(t("One new line two"), "One\nTwo");
        assert_eq!(t("One new paragraph two"), "One\n\nTwo");
        assert_eq!(t("One newline two"), "One\nTwo");
    }

    #[test]
    fn brackets_and_quotes_hug_their_contents() {
        assert_eq!(t("See open bracket page four close bracket now"), "See (page four) now");
        assert_eq!(t("He said open quote hi close quote"), "He said \"hi\"");
    }

    #[test]
    fn ordinary_speech_is_untouched() {
        for s in ["A period of time.", "He gave a quote.", "Open the new file.", "It is a long line."] {
            assert_eq!(t(s), s);
        }
    }

    #[test]
    fn scratch_that_is_a_whole_utterance_action() {
        assert!(apply("Scratch that.").scratch);
        assert!(apply("delete that").scratch);
        assert!(!apply("Please scratch that itch.").scratch);
    }

    #[test]
    fn trailing_press_enter_sets_the_flag_and_is_removed() {
        let p = apply("Send the report, press enter.");
        assert_eq!((p.text.as_str(), p.enter), ("Send the report", true));
        let p = apply("Press enter.");
        assert_eq!((p.text.as_str(), p.enter), ("", true));
        assert!(!apply("Press enter to continue.").enter, "only a trailing command counts");
    }

    #[test]
    fn newlines_survive_the_rest_of_the_pipeline() {
        use crate::lexicon::Fix;
        let fixes = [Fix { from: "post is".into(), to: "Postiz".into() }];
        assert_eq!(crate::vocab::apply_fixes(&t("Open post is new line done"), &fixes), "Open Postiz\nDone");
    }
}
