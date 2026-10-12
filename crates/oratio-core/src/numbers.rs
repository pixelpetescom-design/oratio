//! Spoken numbers written the way people write them: "twenty five dollars" → "$25",
//! "fifty percent" → "50%", "ten thirty a.m." → "10:30 am", "March twenty fifth" → "March 25th".
//! Small stand-alone numbers ("one", "two of them") stay as words; anything the rules are not
//! sure about is left exactly as the recogniser wrote it.

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Unit,
    Teen,
    Tens,
    Hundred,
    Scale,
}

const MONTHS: &[&str] = &["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

fn cardinal(word: &str) -> Option<(Kind, u64)> {
    const UNITS: [&str; 9] = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
    const TEENS: [&str; 10] = ["ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"];
    const TENS: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    if let Some(i) = UNITS.iter().position(|w| *w == word) {
        return Some((Kind::Unit, i as u64 + 1));
    }
    if let Some(i) = TEENS.iter().position(|w| *w == word) {
        return Some((Kind::Teen, i as u64 + 10));
    }
    if let Some(i) = TENS.iter().position(|w| *w == word) {
        return Some((Kind::Tens, (i as u64 + 2) * 10));
    }
    match word {
        "hundred" => Some((Kind::Hundred, 100)),
        "thousand" => Some((Kind::Scale, 1_000)),
        "million" => Some((Kind::Scale, 1_000_000)),
        _ => None,
    }
}

/// "fifth" → 5, "twentieth" → 20 (used only right after a month name).
fn ordinal(word: &str) -> Option<u64> {
    const NAMES: [&str; 19] = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth", "eleventh", "twelfth", "thirteenth", "fourteenth", "fifteenth", "sixteenth",
        "seventeenth", "eighteenth", "nineteenth",
    ];
    if let Some(i) = NAMES.iter().position(|w| *w == word) {
        return Some(i as u64 + 1);
    }
    match word {
        "twentieth" => Some(20),
        "thirtieth" => Some(30),
        _ => None,
    }
}

fn suffix(n: u64) -> &'static str {
    match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    }
}

struct Tok<'a> {
    lead: &'a str,
    core: &'a str,
    trail: &'a str,
}

fn split(token: &str) -> Tok<'_> {
    let start = token.find(|c: char| c.is_alphanumeric()).unwrap_or(token.len());
    let end = token.rfind(|c: char| c.is_alphanumeric()).map_or(start, |i| i + token[i..].chars().next().map_or(1, char::len_utf8));
    Tok { lead: &token[..start], core: &token[start..end], trail: &token[end..] }
}

/// What a run of words means numerically.
struct Number {
    value: u64,
    /// How many whitespace tokens it used.
    tokens: usize,
    ordinal: bool,
    /// Written with digits already ("25"), not spelled out.
    digits: Option<String>,
}

fn is_digits(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.') && !s.ends_with(',') && !s.ends_with('.')
}

/// Reads a number starting at `tokens[0]`. Only the last token may carry trailing punctuation.
fn read(tokens: &[Tok<'_>]) -> Option<Number> {
    let first = tokens.first()?;
    if is_digits(first.core) {
        let value = first.core.replace(',', "").parse::<f64>().ok()? as u64;
        return Some(Number { value, tokens: 1, ordinal: false, digits: Some(first.core.to_string()) });
    }

    let (mut total, mut current, mut last) = (0u64, 0u64, None::<Kind>);
    let mut used = 0usize;
    let mut ordinal_hit = false;
    'tokens: for (n, tok) in tokens.iter().enumerate() {
        if n > 0 && (!tokens[n - 1].trail.is_empty() || !tok.lead.is_empty()) {
            break; // punctuation ends a number: "twenty, five"
        }
        let lower = tok.core.to_lowercase();
        let parts: Vec<&str> = lower.split('-').collect();
        for (p, part) in parts.iter().enumerate() {
            let at_end = p + 1 == parts.len();
            if *part == "and" {
                let after_big = matches!(last, Some(Kind::Hundred | Kind::Scale));
                let next_ok = tokens.get(n + 1).is_some_and(|t| cardinal(&t.core.to_lowercase()).is_some_and(|(k, _)| matches!(k, Kind::Unit | Kind::Teen | Kind::Tens)));
                if after_big && next_ok && parts.len() == 1 && tok.trail.is_empty() {
                    used = n + 1;
                    continue 'tokens;
                }
                break 'tokens;
            }
            if let (Some(v), true, None | Some(Kind::Tens)) = (ordinal(part), at_end, last) {
                if last == Some(Kind::Tens) && v >= 10 {
                    break 'tokens;
                }
                current += v;
                ordinal_hit = true;
                used = n + 1;
                break 'tokens;
            }
            let Some((kind, v)) = cardinal(part) else { break 'tokens };
            let allowed = match kind {
                Kind::Unit => matches!(last, None | Some(Kind::Hundred | Kind::Scale)) || (last == Some(Kind::Tens) && current % 10 == 0),
                Kind::Teen | Kind::Tens => matches!(last, None | Some(Kind::Hundred | Kind::Scale)),
                Kind::Hundred => matches!(last, Some(Kind::Unit | Kind::Teen)),
                Kind::Scale => matches!(last, Some(Kind::Unit | Kind::Teen | Kind::Tens | Kind::Hundred)),
            };
            if !allowed {
                break 'tokens;
            }
            match kind {
                Kind::Unit | Kind::Teen | Kind::Tens => current += v,
                Kind::Hundred => current *= 100,
                Kind::Scale => {
                    total += current * v;
                    current = 0;
                }
            }
            last = Some(kind);
            used = n + 1;
        }
    }
    (used > 0).then_some(Number { value: total + current, tokens: used, ordinal: ordinal_hit, digits: None })
}

fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn text_of(n: &Number) -> String {
    n.digits.clone().unwrap_or_else(|| if n.value >= 1000 { with_commas(n.value) } else { n.value.to_string() })
}

fn meridiem(core: &str) -> Option<&'static str> {
    match core.to_lowercase().replace('.', "").as_str() {
        "am" => Some("am"),
        "pm" => Some("pm"),
        _ => None,
    }
}

/// Rewrites spoken numbers in `text`. Line breaks are preserved.
pub fn format(text: &str) -> String {
    text.split('\n').map(format_line).collect::<Vec<_>>().join("\n")
}

fn format_line(line: &str) -> String {
    let raw: Vec<&str> = line.split_whitespace().collect();
    let toks: Vec<Tok<'_>> = raw.iter().map(|t| split(t)).collect();
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    let mut i = 0;
    // Set after a run like "twenty twenty": the pieces are left alone rather than half-converted.
    let mut hold = false;
    while i < raw.len() {
        let rest = &toks[i..];
        // 1. A clock time: "ten thirty a.m.", "3 pm", "10:30 p.m."
        if let Some((text, used)) = clock(rest) {
            let last = &toks[i + used - 1];
            // The full stop of "a.m." belongs to the abbreviation unless the sentence ends there.
            let trail = if last.trail == "." && last.core.contains('.') && i + used < raw.len() { "" } else { last.trail };
            out.push(format!("{}{}{}", toks[i].lead, text, trail));
            i += used;
            continue;
        }
        // 2. A month and a day: "March twenty fifth" → "March 25th"
        if MONTHS.contains(&toks[i].core.to_lowercase().as_str()) && toks[i].trail.is_empty() {
            if let Some(n) = read(&rest[1..]).filter(|n| n.digits.is_none() && (1..=31).contains(&n.value)) {
                let day = if n.ordinal { format!("{}{}", n.value, suffix(n.value)) } else { n.value.to_string() };
                out.push(format!("{} {}{}", toks[i].core_with_lead(), day, toks[i + n.tokens].trail));
                i += 1 + n.tokens;
                continue;
            }
        }
        // 3. Money, percentages and plain numbers.
        if let Some(n) = read(rest).filter(|n| !n.ordinal) {
            let after = i + n.tokens;
            let next = toks.get(after).map(|t| t.core.to_lowercase());
            let last_trail_clear = toks[after - 1].trail.is_empty();
            if last_trail_clear {
                if matches!(next.as_deref(), Some("percent")) {
                    out.push(format!("{}{}%{}", toks[i].lead, text_of(&n), toks[after].trail));
                    i = after + 1;
                    continue;
                }
                if matches!(next.as_deref(), Some("dollar" | "dollars")) {
                    let mut text = format!("${}", text_of(&n));
                    let mut end = after;
                    // "... and fifty cents"
                    if toks[after].trail.is_empty() && toks.get(after + 1).is_some_and(|t| t.core.eq_ignore_ascii_case("and")) {
                        if let Some(c) = read(&toks[after + 2..]).filter(|c| c.digits.is_none() && c.value < 100) {
                            if toks.get(after + 2 + c.tokens).is_some_and(|t| matches!(t.core.to_lowercase().as_str(), "cent" | "cents")) {
                                text = format!("{text}.{:02}", c.value);
                                end = after + 2 + c.tokens;
                            }
                        }
                    }
                    out.push(format!("{}{}{}", toks[i].lead, text, toks[end].trail));
                    i = end + 1;
                    continue;
                }
            }
            // Plain number words: only 10 and over, and only when it is not a run of separate numbers ("ten thirty").
            let adjacent = toks.get(after).is_some_and(|t| toks[after - 1].trail.is_empty() && read(std::slice::from_ref(t)).is_some_and(|_| cardinal(&t.core.to_lowercase()).is_some()));
            if hold || adjacent {
                hold = adjacent;
                out.extend(raw[i..after].iter().map(|t| t.to_string()));
                i = after;
                continue;
            }
            if n.digits.is_none() && n.value >= 10 {
                out.push(format!("{}{}{}", toks[i].lead, text_of(&n), toks[after - 1].trail));
                i = after;
                continue;
            }
        }
        hold = false;
        out.push(raw[i].to_string());
        i += 1;
    }
    out.join(" ")
}

impl Tok<'_> {
    fn core_with_lead(&self) -> String {
        format!("{}{}", self.lead, self.core)
    }
}

/// "ten thirty am", "3 pm", "10:30 a.m." → ("10:30 am", tokens used)
fn clock(toks: &[Tok<'_>]) -> Option<(String, usize)> {
    // Already "10:30"? Then only the am/pm needs tidying.
    if let Some((h, m)) = toks[0].core.split_once(':') {
        let ok = h.parse::<u64>().is_ok_and(|h| (1..=12).contains(&h)) && m.len() == 2 && m.parse::<u64>().is_ok_and(|m| m < 60);
        let ap = toks.get(1).and_then(|t| meridiem(t.core))?;
        return (ok && toks[0].trail.is_empty()).then(|| (format!("{} {}", toks[0].core, ap), 2));
    }
    let hour = read(toks)?;
    if hour.ordinal || !toks[hour.tokens - 1].trail.is_empty() {
        return None;
    }
    if !(1..=12).contains(&hour.value) {
        return None;
    }
    let (minutes, used) = match read(&toks[hour.tokens..]).filter(|m| m.digits.is_none() && (10..60).contains(&m.value) && !m.ordinal) {
        Some(m) if toks[hour.tokens + m.tokens - 1].trail.is_empty() => (Some(m.value), hour.tokens + m.tokens),
        _ => (None, hour.tokens),
    };
    let ap = meridiem(toks.get(used)?.core)?;
    let time = match minutes {
        Some(m) => format!("{}:{:02} {}", hour.value, m, ap),
        None => format!("{} {}", hour.value, ap),
    };
    Some((time, used + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_become_digits_from_ten_up() {
        assert_eq!(format("I have twenty five cats."), "I have 25 cats.");
        assert_eq!(format("Twenty-five thousand and six hundred people came."), "25,600 people came.");
        assert_eq!(format("One hundred and five."), "105.");
        assert_eq!(format("There were fourteen hundred of them."), "There were 1,400 of them.");
    }

    #[test]
    fn small_numbers_stay_as_words() {
        assert_eq!(format("One of them has two apples."), "One of them has two apples.");
        assert_eq!(format("Pick one, two or three."), "Pick one, two or three.");
    }

    #[test]
    fn money_and_percentages() {
        assert_eq!(format("It costs twenty five dollars."), "It costs $25.");
        assert_eq!(format("It costs twenty five dollars and fifty cents."), "It costs $25.50.");
        assert_eq!(format("About fifty percent of people agree."), "About 50% of people agree.");
        assert_eq!(format("Only five percent."), "Only 5%.");
        assert_eq!(format("That is 20 dollars."), "That is $20.");
        assert_eq!(format("Two dollars."), "$2.");
    }

    #[test]
    fn clock_times() {
        assert_eq!(format("Meet at ten thirty a.m. tomorrow."), "Meet at 10:30 am tomorrow.");
        assert_eq!(format("Call me at three pm."), "Call me at 3 pm.");
        assert_eq!(format("Be there at 9:15 a.m."), "Be there at 9:15 am.");
        assert_eq!(format("It was ten thirty."), "It was ten thirty.", "no am/pm, so not clearly a time");
    }

    #[test]
    fn dates() {
        assert_eq!(format("See you on March twenty fifth."), "See you on March 25th.");
        assert_eq!(format("It is due on June first."), "It is due on June 1st.");
        assert_eq!(format("It is due on May twelve."), "It is due on May 12.");
        assert_eq!(format("The third of June."), "The third of June.");
    }

    #[test]
    fn leaves_everything_else_alone() {
        let s = "Hello there, how are you doing today?\nSecond line.";
        assert_eq!(format(s), s);
        assert_eq!(format("twenty twenty"), "twenty twenty", "two separate numbers are not merged");
        assert_eq!(format("Fifty, sixty."), "50, 60.");
    }
}
