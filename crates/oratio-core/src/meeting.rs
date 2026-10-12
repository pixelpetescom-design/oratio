//! Meetings: a long recording of a call, written up as a transcript with who said what.
//!
//! Who is who comes from where the sound arrived: the microphone is `You`, whatever the computer
//! is playing (the other people on the call) is `Others`. That is exact, because the two never mix.

use crate::CoreError;
use serde::Serialize;

pub type MeetingId = i64;

/// Where a line of speech came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The microphone.
    You,
    /// The computer's own audio output: everyone else on the call.
    Others,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::You => "you",
            Source::Others => "others",
        }
    }

    pub fn parse(id: &str) -> Option<Source> {
        match id {
            "you" => Some(Source::You),
            "others" => Some(Source::Others),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::You => "You",
            Source::Others => "Others",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    pub source: Source,
    /// Milliseconds since the meeting started.
    pub start_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeetingInfo {
    pub id: MeetingId,
    pub started_at_ms: i64,
    /// None while it is still recording (or if the app was closed mid-meeting).
    pub ended_at_ms: Option<i64>,
    pub title: String,
    pub lines: u32,
}

/// Port: durable storage for meetings. Lines are written as they are recognised, so a crash loses
/// at most the sentence being transcribed.
pub trait Meetings: Send + Sync {
    fn begin_meeting(&self, started_at_ms: i64) -> Result<MeetingId, CoreError>;
    fn append_line(&self, id: MeetingId, line: &Line) -> Result<(), CoreError>;
    fn finish_meeting(&self, id: MeetingId, ended_at_ms: i64) -> Result<(), CoreError>;
    fn rename_meeting(&self, id: MeetingId, title: &str) -> Result<(), CoreError>;
    fn delete_meeting(&self, id: MeetingId) -> Result<(), CoreError>;
    /// Newest first.
    fn list_meetings(&self, limit: u32) -> Result<Vec<MeetingInfo>, CoreError>;
    /// In the order they were spoken.
    fn meeting_lines(&self, id: MeetingId) -> Result<Vec<Line>, CoreError>;
}

/// "0:07", "12:03", "1:02:03".
pub fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The transcript as plain text, one line per utterance, in the order spoken. Neighbouring lines
/// from the same source are joined, so a long answer is one paragraph under one time stamp.
pub fn transcript(lines: &[Line]) -> String {
    let mut ordered: Vec<&Line> = lines.iter().collect();
    ordered.sort_by_key(|l| l.start_ms);
    let mut out: Vec<(Source, u64, String)> = Vec::new();
    for l in ordered {
        match out.last_mut() {
            Some((source, _, text)) if *source == l.source => {
                text.push(' ');
                text.push_str(&l.text);
            }
            _ => out.push((l.source, l.start_ms, l.text.clone())),
        }
    }
    out.iter().map(|(s, at, text)| format!("[{}] {}: {}", clock(*at), s.label(), text)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(source: Source, start_ms: u64, text: &str) -> Line {
        Line { source, start_ms, text: text.into() }
    }

    #[test]
    fn clock_reads_naturally() {
        assert_eq!(clock(7_400), "0:07");
        assert_eq!(clock(723_000), "12:03");
        assert_eq!(clock(3_723_000), "1:02:03");
    }

    #[test]
    fn transcript_orders_by_time_and_joins_a_speakers_neighbouring_lines() {
        let lines = [
            line(Source::Others, 9_000, "Sounds good."),
            line(Source::You, 1_000, "Hi everyone."),
            line(Source::You, 3_000, "Thanks for joining."),
            line(Source::Others, 5_000, "Hello."),
        ];
        assert_eq!(transcript(&lines), "[0:01] You: Hi everyone. Thanks for joining.\n[0:05] Others: Hello. Sounds good.");
    }

    #[test]
    fn sources_round_trip_through_their_ids() {
        for s in [Source::You, Source::Others] {
            assert_eq!(Source::parse(s.id()), Some(s));
        }
        assert_eq!(Source::parse("robot"), None);
    }
}
