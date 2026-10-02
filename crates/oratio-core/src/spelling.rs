//! Australian English spelling, applied to the recogniser's (American-leaning) output.
//! Deterministic and conservative: an explicit table for irregular words plus a few
//! suffix families with exceptions. Words that differ by meaning rather than region
//! (license/licence, meter/metre for devices, program) are deliberately left alone.

/// Word stems that take "-our" in Australian English (US "-or").
const OUR_STEMS: &[(&str, &str)] = &[
    ("color", "colour"),
    ("favor", "favour"),
    ("honor", "honour"),
    ("humor", "humour"),
    ("labor", "labour"),
    ("neighbor", "neighbour"),
    ("behavior", "behaviour"),
    ("flavor", "flavour"),
    ("harbor", "harbour"),
    ("rumor", "rumour"),
    ("vapor", "vapour"),
    ("savior", "saviour"),
    ("odor", "odour"),
    ("armor", "armour"),
    ("endeavor", "endeavour"),
    ("glamor", "glamour"),
    ("tumor", "tumour"),
    ("vigor", "vigour"),
    ("rigor", "rigour"),
    ("splendor", "splendour"),
    ("parlor", "parlour"),
];
const OUR_PREFIXES: &[&str] = &["", "dis", "water", "multi"];
const OUR_SUFFIXES: &[&str] = &["", "s", "ed", "ing", "ful", "fully", "less", "able", "ably", "ite", "ites", "er", "ers", "hood", "hoods"];

/// Irregular words (lower case, US → AU).
const WORDS: &[(&str, &str)] = &[
    ("center", "centre"), ("centers", "centres"), ("centered", "centred"), ("centering", "centring"),
    ("theater", "theatre"), ("theaters", "theatres"),
    ("fiber", "fibre"), ("fibers", "fibres"),
    ("liter", "litre"), ("liters", "litres"),
    ("kilometer", "kilometre"), ("kilometers", "kilometres"),
    ("centimeter", "centimetre"), ("centimeters", "centimetres"),
    ("millimeter", "millimetre"), ("millimeters", "millimetres"),
    ("caliber", "calibre"), ("somber", "sombre"), ("meager", "meagre"), ("specter", "spectre"),
    ("defense", "defence"), ("defenses", "defences"), ("offense", "offence"), ("offenses", "offences"),
    ("pretense", "pretence"),
    ("traveling", "travelling"), ("traveled", "travelled"), ("traveler", "traveller"), ("travelers", "travellers"),
    ("canceled", "cancelled"), ("canceling", "cancelling"), ("cancelation", "cancellation"),
    ("labeled", "labelled"), ("labeling", "labelling"),
    ("modeled", "modelled"), ("modeling", "modelling"),
    ("leveled", "levelled"), ("leveling", "levelling"),
    ("signaled", "signalled"), ("signaling", "signalling"),
    ("fueled", "fuelled"), ("fueling", "fuelling"),
    ("totaled", "totalled"), ("totaling", "totalling"),
    ("marveled", "marvelled"), ("counseling", "counselling"), ("counselor", "counsellor"), ("counselors", "counsellors"),
    ("jeweler", "jeweller"), ("jewelry", "jewellery"),
    ("enroll", "enrol"), ("enrolls", "enrols"), ("enrollment", "enrolment"), ("enrollments", "enrolments"),
    ("fulfill", "fulfil"), ("fulfills", "fulfils"), ("fulfillment", "fulfilment"),
    ("skillful", "skilful"), ("willful", "wilful"), ("installment", "instalment"), ("installments", "instalments"),
    ("gray", "grey"), ("grays", "greys"), ("grayed", "greyed"), ("grayish", "greyish"),
    ("aluminum", "aluminium"), ("mom", "mum"), ("moms", "mums"), ("mommy", "mummy"),
    ("pajamas", "pyjamas"), ("plow", "plough"), ("plows", "ploughs"), ("plowed", "ploughed"),
    ("skeptic", "sceptic"), ("skeptics", "sceptics"), ("skeptical", "sceptical"), ("skepticism", "scepticism"),
    ("catalog", "catalogue"), ("catalogs", "catalogues"), ("analog", "analogue"),
    ("aging", "ageing"), ("artifact", "artefact"), ("artifacts", "artefacts"),
    ("esthetic", "aesthetic"), ("esthetics", "aesthetics"), ("anesthesia", "anaesthesia"), ("pediatric", "paediatric"),
    ("diarrhea", "diarrhoea"), ("maneuver", "manoeuvre"), ("maneuvers", "manoeuvres"),
    ("mustache", "moustache"), ("yogurt", "yoghurt"), ("cozy", "cosy"), ("specialty", "speciality"),
    ("smolder", "smoulder"), ("mold", "mould"), ("molds", "moulds"), ("moldy", "mouldy"),
    ("ax", "axe"), ("tire", "tyre"), ("tires", "tyres"),
];

const IZE_SUFFIXES: &[(&str, &str)] = &[
    ("izations", "isations"), ("ization", "isation"), ("izing", "ising"), ("izers", "isers"), ("izer", "iser"),
    ("ized", "ised"), ("izes", "ises"), ("ize", "ise"),
];
const YZE_SUFFIXES: &[(&str, &str)] = &[("yzing", "ysing"), ("yzed", "ysed"), ("yzes", "yses"), ("yze", "yse")];

/// Words where the "z" is correct in Australian English too (size, seize, prize…).
fn keeps_z(lower: &str) -> bool {
    ["size", "sized", "sizes", "sizing", "seize", "seized", "seizes", "seizing", "prize", "prized", "prizes"]
        .iter()
        .any(|s| lower.ends_with(s))
}

fn australian_word(lower: &str) -> Option<String> {
    if let Some((_, au)) = WORDS.iter().find(|(us, _)| *us == lower) {
        return Some((*au).to_string());
    }
    for (us_stem, au_stem) in OUR_STEMS {
        for prefix in OUR_PREFIXES {
            for suffix in OUR_SUFFIXES {
                if lower.len() == prefix.len() + us_stem.len() + suffix.len()
                    && lower.starts_with(prefix)
                    && lower[prefix.len()..].starts_with(us_stem)
                    && lower.ends_with(suffix)
                {
                    return Some(format!("{prefix}{au_stem}{suffix}"));
                }
            }
        }
    }
    if !keeps_z(lower) {
        for (us, au) in IZE_SUFFIXES {
            if let Some(stem) = lower.strip_suffix(us) {
                if stem.chars().count() >= 3 {
                    return Some(format!("{stem}{au}"));
                }
            }
        }
    }
    for (us, au) in YZE_SUFFIXES {
        if let Some(stem) = lower.strip_suffix(us) {
            if stem.chars().count() >= 3 {
                return Some(format!("{stem}{au}"));
            }
        }
    }
    None
}

/// Carries the original word's capitalisation over to its replacement.
fn match_case(original: &str, replacement: &str) -> String {
    let letters: Vec<char> = original.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        replacement.to_uppercase()
    } else if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut c = replacement.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    } else {
        replacement.to_string()
    }
}

/// Rewrites American spellings to Australian ones. Words in `protected` (the user's own
/// vocabulary, e.g. a product name) are never changed.
pub fn to_australian<S: AsRef<str>>(text: &str, protected: &[S]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let lower = word.to_lowercase();
        let is_protected = protected.iter().any(|p| p.as_ref().eq_ignore_ascii_case(word));
        match australian_word(&lower) {
            Some(au) if !is_protected => out.push_str(&match_case(word, &au)),
            _ => out.push_str(word),
        }
        word.clear();
    };
    for ch in text.chars() {
        // Apostrophes join words ("don't"); everything else ends a word.
        if ch.is_alphabetic() || (ch == '\'' && !word.is_empty()) {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
            out.push(ch);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn au(s: &str) -> String {
        to_australian::<&str>(s, &[])
    }

    #[test]
    fn our_family() {
        assert_eq!(au("My favorite color is gray."), "My favourite colour is grey.");
        assert_eq!(au("The neighbors were colorful and honorable."), "The neighbours were colourful and honourable.");
        assert_eq!(au("Behavior and humor."), "Behaviour and humour.");
    }

    #[test]
    fn ize_family_and_exceptions() {
        assert_eq!(au("We organize and realize the analysis; they analyze it."), "We organise and realise the analysis; they analyse it.");
        assert_eq!(au("Optimization and organizations."), "Optimisation and organisations.");
        assert_eq!(au("Resize the size of the prize; seize it."), "Resize the size of the prize; seize it.");
        assert_eq!(au("Maize."), "Maize.");
    }

    #[test]
    fn re_and_doubled_l() {
        assert_eq!(au("The center of the theater is 5 kilometers away."), "The centre of the theatre is 5 kilometres away.");
        assert_eq!(au("She canceled while traveling."), "She cancelled while travelling.");
        assert_eq!(au("Fulfill the enrollment."), "Fulfil the enrolment.");
    }

    #[test]
    fn everyday_words() {
        assert_eq!(au("Mom bought aluminum pajamas."), "Mum bought aluminium pyjamas.");
    }

    #[test]
    fn leaves_dialect_neutral_and_meaning_dependent_words_alone() {
        let same = "The labor laboratory has a program, a license, a parking meter and humorous rumors of a thermometer.";
        assert_eq!(au(same), "The labour laboratory has a program, a license, a parking meter and humorous rumours of a thermometer.");
    }

    #[test]
    fn keeps_capitalisation_and_punctuation() {
        assert_eq!(au("COLOR, Color; color!"), "COLOUR, Colour; colour!");
        assert_eq!(au("Don't organize it."), "Don't organise it.");
    }

    #[test]
    fn protected_words_are_untouched() {
        assert_eq!(to_australian("Open Color Picker and organize.", &["color"]), "Open Color Picker and organise.");
    }
}
