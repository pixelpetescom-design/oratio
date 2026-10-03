//! In-memory fakes of the ports, shared by unit tests.

use crate::apps::{AppRule, AppRules};
use crate::history::{Entry, History, RecordingId, Status};
use crate::lexicon::{Fix, Lexicon};
use crate::CoreError;
use std::sync::Mutex;

#[derive(Default)]
pub struct MemHistory {
    pub rows: Mutex<Vec<Entry>>,
    pub fail_writes: bool,
}

impl MemHistory {
    fn with<T>(&self, id: RecordingId, f: impl FnOnce(&mut Entry) -> T) -> Result<T, CoreError> {
        let mut rows = self.rows.lock().unwrap();
        rows.iter_mut().find(|e| e.id == id).map(f).ok_or_else(|| CoreError::History("missing".into()))
    }
}

impl History for MemHistory {
    fn begin(&self, started_at_ms: i64) -> Result<RecordingId, CoreError> {
        let mut r = self.rows.lock().unwrap();
        let id = r.len() as i64 + 1;
        r.push(Entry { id, started_at_ms, status: Status::Recording, segments: vec![], final_text: None, error: None });
        Ok(id)
    }
    fn append_segment(&self, id: RecordingId, text: &str) -> Result<(), CoreError> {
        if self.fail_writes {
            return Err(CoreError::History("disk full".into()));
        }
        self.with(id, |e| e.segments.push(text.into()))
    }
    fn complete(&self, id: RecordingId, t: &str) -> Result<(), CoreError> {
        if self.fail_writes {
            return Err(CoreError::History("disk full".into()));
        }
        self.with(id, |e| {
            e.status = Status::Completed;
            e.final_text = Some(t.into());
        })
    }
    fn fail(&self, _: RecordingId, _: &str) -> Result<(), CoreError> {
        Ok(())
    }
    fn delete(&self, id: RecordingId) -> Result<(), CoreError> {
        self.rows.lock().unwrap().retain(|e| e.id != id);
        Ok(())
    }
    fn clear(&self) -> Result<usize, CoreError> {
        let mut r = self.rows.lock().unwrap();
        let before = r.len();
        r.retain(|e| e.status == Status::Recording);
        Ok(before - r.len())
    }
    fn get(&self, id: RecordingId) -> Result<Option<Entry>, CoreError> {
        Ok(self.rows.lock().unwrap().iter().find(|e| e.id == id).cloned())
    }
    fn update_text(&self, id: RecordingId, text: &str) -> Result<(), CoreError> {
        self.with(id, |e| e.final_text = Some(text.into()))
    }
    fn list(&self, _: u32) -> Result<Vec<Entry>, CoreError> {
        Ok(self.rows.lock().unwrap().clone())
    }
    fn recover_interrupted(&self) -> Result<usize, CoreError> {
        Ok(0)
    }
}

#[derive(Default)]
pub struct MemLexicon {
    pub words: Mutex<Vec<String>>,
    pub fixes: Mutex<Vec<Fix>>,
    pub snippets: Mutex<Vec<Fix>>,
}

impl Lexicon for MemLexicon {
    fn words(&self) -> Result<Vec<String>, CoreError> {
        Ok(self.words.lock().unwrap().clone())
    }
    fn add_word(&self, word: &str) -> Result<(), CoreError> {
        let mut w = self.words.lock().unwrap();
        if !w.iter().any(|x| x.eq_ignore_ascii_case(word)) {
            w.insert(0, word.into());
        }
        Ok(())
    }
    fn remove_word(&self, word: &str) -> Result<(), CoreError> {
        self.words.lock().unwrap().retain(|x| !x.eq_ignore_ascii_case(word));
        Ok(())
    }
    fn fixes(&self) -> Result<Vec<Fix>, CoreError> {
        Ok(self.fixes.lock().unwrap().clone())
    }
    fn add_fix(&self, fix: &Fix) -> Result<(), CoreError> {
        let mut f = self.fixes.lock().unwrap();
        f.retain(|x| x.from != fix.from);
        f.push(fix.clone());
        Ok(())
    }
    fn remove_fix(&self, from: &str) -> Result<(), CoreError> {
        self.fixes.lock().unwrap().retain(|x| x.from != from);
        Ok(())
    }
    fn snippets(&self) -> Result<Vec<Fix>, CoreError> {
        Ok(self.snippets.lock().unwrap().clone())
    }
    fn add_snippet(&self, snippet: &Fix) -> Result<(), CoreError> {
        let mut s = self.snippets.lock().unwrap();
        s.retain(|x| x.from != snippet.from);
        s.push(snippet.clone());
        Ok(())
    }
    fn remove_snippet(&self, trigger: &str) -> Result<(), CoreError> {
        self.snippets.lock().unwrap().retain(|x| x.from != trigger);
        Ok(())
    }
}

#[derive(Default)]
pub struct MemRules {
    pub rules: Mutex<Vec<AppRule>>,
}

impl AppRules for MemRules {
    fn rules(&self) -> Result<Vec<AppRule>, CoreError> {
        Ok(self.rules.lock().unwrap().clone())
    }
    fn add_rule(&self, rule: &AppRule) -> Result<(), CoreError> {
        let mut r = self.rules.lock().unwrap();
        r.retain(|x| x.pattern != rule.pattern);
        r.insert(0, rule.clone());
        Ok(())
    }
    fn remove_rule(&self, pattern: &str) -> Result<(), CoreError> {
        self.rules.lock().unwrap().retain(|x| x.pattern != pattern);
        Ok(())
    }
}
