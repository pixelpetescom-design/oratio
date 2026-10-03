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

/// Where a voice search goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Google,
    Bing,
    DuckDuckGo,
    YouTube,
    Maps,
    Wikipedia,
    /// Google Images, large pictures only.
    Images,
}

impl Engine {
    pub fn parse(id: &str) -> Option<Engine> {
        Some(match id.to_lowercase().as_str() {
            "google" => Engine::Google,
            "bing" => Engine::Bing,
            "duckduckgo" => Engine::DuckDuckGo,
            "youtube" => Engine::YouTube,
            "maps" => Engine::Maps,
            "wikipedia" => Engine::Wikipedia,
            "images" | "image" => Engine::Images,
            _ => return None,
        })
    }

    fn template(self) -> &'static str {
        match self {
            Engine::Google => "https://www.google.com/search?q={q}",
            Engine::Bing => "https://www.bing.com/search?q={q}",
            Engine::DuckDuckGo => "https://duckduckgo.com/?q={q}",
            Engine::YouTube => "https://www.youtube.com/results?search_query={q}",
            Engine::Maps => "https://www.google.com/maps/search/{q}",
            Engine::Wikipedia => "https://en.wikipedia.org/w/index.php?search={q}",
            Engine::Images => "https://www.google.com/search?tbm=isch&tbs=isz:l&q={q}",
        }
    }
}

fn encode(query: &str) -> String {
    let mut out = String::new();
    for b in query.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The address for `query` on `engine`, safely percent-encoded.
pub fn url(engine: Engine, query: &str) -> String {
    engine.template().replace("{q}", &encode(query))
}

/// A user-supplied address template, e.g. `https://example.com/?s={q}`. Only https addresses
/// containing `{q}` are accepted.
pub fn custom_url(template: &str, query: &str) -> Option<String> {
    let t = template.trim();
    (t.starts_with("https://") && t.contains("{q}")).then(|| t.replace("{q}", &encode(query)))
}

/// Lets the spoken words pick the engine: "youtube cute cats" searches YouTube for "cute cats".
/// Otherwise the default engine is used and the query is unchanged.
pub fn route(query: &str, default: Engine) -> (Engine, String) {
    let mut words = query.split_whitespace();
    if let (Some(first), true) = (words.next(), query.split_whitespace().count() > 1) {
        if let Some(engine) = Engine::parse(first.trim_matches(|c: char| !c.is_alphanumeric())) {
            return (engine, words.collect::<Vec<_>>().join(" "));
        }
    }
    (default, query.to_string())
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
        assert_eq!(url(Engine::Google, "best pizza near me"), "https://www.google.com/search?q=best+pizza+near+me");
        assert_eq!(url(Engine::Google, "rust & c++ #1?"), "https://www.google.com/search?q=rust+%26+c%2B%2B+%231%3F");
    }

    #[test]
    fn encodes_non_ascii_as_utf8_bytes() {
        assert_eq!(url(Engine::Google, "café"), "https://www.google.com/search?q=caf%C3%A9");
    }

    #[test]
    fn each_engine_has_its_own_address() {
        assert_eq!(url(Engine::YouTube, "cute cats"), "https://www.youtube.com/results?search_query=cute+cats");
        assert_eq!(url(Engine::Maps, "coffee near me"), "https://www.google.com/maps/search/coffee+near+me");
        assert_eq!(url(Engine::Wikipedia, "Sydney"), "https://en.wikipedia.org/w/index.php?search=Sydney");
        assert_eq!(url(Engine::Images, "red panda"), "https://www.google.com/search?tbm=isch&tbs=isz:l&q=red+panda");
        assert_eq!(route("images red panda", Engine::Google), (Engine::Images, "red panda".into()));
        assert_eq!(Engine::parse("DuckDuckGo"), Some(Engine::DuckDuckGo));
        assert_eq!(Engine::parse("altavista"), None);
    }

    #[test]
    fn a_spoken_prefix_picks_the_engine() {
        assert_eq!(route("YouTube cute cats", Engine::Google), (Engine::YouTube, "cute cats".into()));
        assert_eq!(route("maps, coffee near me", Engine::Google), (Engine::Maps, "coffee near me".into()));
        assert_eq!(route("best pizza", Engine::Bing), (Engine::Bing, "best pizza".into()));
        assert_eq!(route("youtube", Engine::Google), (Engine::Google, "youtube".into()), "a lone engine name is just a query");
    }

    #[test]
    fn custom_addresses_must_be_https_and_contain_the_placeholder() {
        assert_eq!(custom_url("https://example.com/?s={q}", "a b").as_deref(), Some("https://example.com/?s=a+b"));
        assert_eq!(custom_url("http://example.com/?s={q}", "x"), None);
        assert_eq!(custom_url("https://example.com/", "x"), None);
        assert_eq!(custom_url("javascript:alert({q})", "x"), None);
    }

    #[test]
    fn cannot_inject_extra_url_parts() {
        let u = url(Engine::Google, "a&q=b#frag/../x");
        assert_eq!(u.matches('&').count(), 0);
        assert_eq!(u.matches('#').count(), 0);
        assert!(u.starts_with("https://www.google.com/search?q="));
    }
}
