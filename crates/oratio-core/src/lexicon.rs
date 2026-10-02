//! Port + orchestration for what Oratio learns about the user: a personal vocabulary
//! (hints for the recogniser) and correction rules (applied to finished text).

use crate::history::{History, RecordingId};
use crate::vocab::diff_fixes;
use crate::CoreError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    /// Lower-case words to look for, e.g. "post is".
    pub from: String,
    /// Exact replacement, e.g. "Postiz".
    pub to: String,
}

pub trait Lexicon: Send + Sync {
    /// Newest first.
    fn words(&self) -> Result<Vec<String>, CoreError>;
    fn add_word(&self, word: &str) -> Result<(), CoreError>;
    fn remove_word(&self, word: &str) -> Result<(), CoreError>;
    fn fixes(&self) -> Result<Vec<Fix>, CoreError>;
    fn add_fix(&self, fix: &Fix) -> Result<(), CoreError>;
    fn remove_fix(&self, from: &str) -> Result<(), CoreError>;
}

/// Words shorter than this are too common to be worth hinting.
const MIN_HINT_LEN: usize = 4;

/// Saves the user's edit of a transcript and learns from it. Returns how many
/// corrections were learned.
pub fn learn_from_edit(history: &dyn History, lexicon: &dyn Lexicon, id: RecordingId, new_text: &str) -> Result<usize, CoreError> {
    let new_text = new_text.trim();
    if new_text.is_empty() {
        return Err(CoreError::History("a transcript can't be empty; use Delete instead".into()));
    }
    let entry = history.get(id)?.ok_or_else(|| CoreError::History("that entry no longer exists".into()))?;
    history.update_text(id, new_text)?;
    let fixes = diff_fixes(&entry.text(), new_text);
    for fix in &fixes {
        lexicon.add_fix(fix)?;
        for word in fix.to.split_whitespace().filter(|w| w.chars().count() >= MIN_HINT_LEN) {
            lexicon.add_word(word)?;
        }
    }
    Ok(fixes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MemHistory, MemLexicon};

    #[test]
    fn editing_saves_the_text_and_teaches_fix_and_vocabulary() {
        let (h, l) = (MemHistory::default(), MemLexicon::default());
        let id = h.begin(1).unwrap();
        h.append_segment(id, "open the post is app").unwrap();
        h.complete(id, "Open the post is app.").unwrap();

        assert_eq!(learn_from_edit(&h, &l, id, "Open the Postiz app.").unwrap(), 1);
        assert_eq!(h.get(id).unwrap().unwrap().text(), "Open the Postiz app.");
        assert_eq!(l.fixes().unwrap(), vec![Fix { from: "post is".into(), to: "Postiz".into() }]);
        assert_eq!(l.words().unwrap(), vec!["Postiz".to_string()]);
    }

    #[test]
    fn rejects_empty_and_missing() {
        let (h, l) = (MemHistory::default(), MemLexicon::default());
        let id = h.begin(1).unwrap();
        assert!(learn_from_edit(&h, &l, id, "   ").is_err());
        assert!(learn_from_edit(&h, &l, 999, "hello").is_err());
    }

    #[test]
    fn an_edit_with_no_wording_change_teaches_nothing() {
        let (h, l) = (MemHistory::default(), MemLexicon::default());
        let id = h.begin(1).unwrap();
        h.complete(id, "Hello there.").unwrap();
        assert_eq!(learn_from_edit(&h, &l, id, "Hello there!").unwrap(), 0);
        assert!(l.fixes().unwrap().is_empty() && l.words().unwrap().is_empty());
    }
}
