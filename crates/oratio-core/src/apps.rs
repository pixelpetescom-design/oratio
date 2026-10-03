//! Per-app behaviour: what Oratio does after dictating depends on which app has focus
//! (e.g. never type into a game; press Enter after typing into a chat window).

use crate::CoreError;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppAction {
    /// Put the text on the clipboard but don't type it into the app.
    CopyOnly,
    /// Type it, then press Enter (chat boxes, search bars).
    EnterAfter,
}

impl AppAction {
    pub fn parse(id: &str) -> Option<AppAction> {
        match id {
            "copy_only" => Some(AppAction::CopyOnly),
            "enter_after" => Some(AppAction::EnterAfter),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            AppAction::CopyOnly => "copy_only",
            AppAction::EnterAfter => "enter_after",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppRule {
    /// Matched (case-insensitively) against the app's name and window title.
    pub pattern: String,
    pub action: AppAction,
}

/// The app that currently has focus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActiveApp {
    pub name: String,
    pub title: String,
}

/// Port: durable app rules, newest first.
pub trait AppRules: Send + Sync {
    fn rules(&self) -> Result<Vec<AppRule>, CoreError>;
    fn add_rule(&self, rule: &AppRule) -> Result<(), CoreError>;
    fn remove_rule(&self, pattern: &str) -> Result<(), CoreError>;
}

/// The first rule whose pattern appears in the app's name or window title.
pub fn action_for(rules: &[AppRule], app: &ActiveApp) -> Option<AppAction> {
    let (name, title) = (app.name.to_lowercase(), app.title.to_lowercase());
    rules
        .iter()
        .find(|r| {
            let p = r.pattern.trim().to_lowercase();
            !p.is_empty() && (name.contains(&p) || title.contains(&p))
        })
        .map(|r| r.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, title: &str) -> ActiveApp {
        ActiveApp { name: name.into(), title: title.into() }
    }
    fn rule(p: &str, a: AppAction) -> AppRule {
        AppRule { pattern: p.into(), action: a }
    }

    #[test]
    fn matches_on_name_or_title_ignoring_case() {
        let rules = [rule("discord", AppAction::EnterAfter), rule("valorant", AppAction::CopyOnly)];
        assert_eq!(action_for(&rules, &app("Discord.exe", "#general")), Some(AppAction::EnterAfter));
        assert_eq!(action_for(&rules, &app("chrome.exe", "VALORANT tracker - Chrome")), Some(AppAction::CopyOnly));
        assert_eq!(action_for(&rules, &app("notepad.exe", "Untitled")), None);
    }

    #[test]
    fn first_matching_rule_wins_and_blank_patterns_never_match() {
        let rules = [rule("  ", AppAction::CopyOnly), rule("code", AppAction::EnterAfter), rule("code", AppAction::CopyOnly)];
        assert_eq!(action_for(&rules, &app("Code.exe", "main.rs")), Some(AppAction::EnterAfter));
    }

    #[test]
    fn action_ids_round_trip() {
        for a in [AppAction::CopyOnly, AppAction::EnterAfter] {
            assert_eq!(AppAction::parse(a.id()), Some(a));
        }
        assert_eq!(AppAction::parse("nope"), None);
    }
}
