#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("speech recognition: {0}")]
    Stt(String),
    #[error("history: {0}")]
    History(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("keyboard: {0}")]
    Input(String),
    #[error("network: {0}")]
    Network(String),
}
