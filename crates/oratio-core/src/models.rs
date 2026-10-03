//! Speech models: turning the file names in the public whisper.cpp model repository into something a
//! person can choose from ("Large v3 Turbo · q5_0 · 574 MB"). Pure parsing; the network lives in
//! `oratio-models`.

/// A file as listed by the model host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    /// SHA-256 published by the host, used to verify the download.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub file: String,
    /// "Large v3 Turbo", "Base (English)"…
    pub name: String,
    /// "q5_0", "q8_0" or "f16" (full precision).
    pub quant: String,
    pub english_only: bool,
    pub size: u64,
    pub sha256: Option<String>,
    /// Smaller = faster and less accurate; used to sort.
    pub rank: u32,
}

const FAMILIES: &[&str] = &["tiny", "base", "small", "medium", "large-v1", "large-v2", "large-v3", "large-v3-turbo"];
const SKIP: &[&str] = &["encoder", "tdrz", "silero", "test", "coreml", "openvino"];

fn title(word: &str) -> String {
    let is_version = word.starts_with('v') && word[1..].chars().all(|c| c.is_ascii_digit());
    if is_version {
        word.to_string()
    } else {
        let mut c = word.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    }
}

fn is_quant(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next() == Some('q') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// Describes a listed file, or `None` if it isn't a usable speech model (encoders, test files, other formats…).
pub fn describe(file: &RemoteFile) -> Option<ModelInfo> {
    let stem = file.path.strip_prefix("ggml-")?.strip_suffix(".bin")?;
    if SKIP.iter().any(|s| stem.contains(s)) {
        return None;
    }
    let (family_part, quant) = match stem.rsplit_once('-') {
        Some((rest, last)) if is_quant(last) => (rest, last.to_string()),
        _ => (stem, "f16".to_string()),
    };
    let english_only = family_part.ends_with(".en");
    let family = family_part.trim_end_matches(".en");
    let rank = FAMILIES.iter().position(|f| *f == family).map_or(FAMILIES.len() as u32, |p| p as u32) + 1;
    let words = family.split('-').map(title).collect::<Vec<_>>().join(" ");
    let name = if english_only { format!("{words} (English)") } else { words };
    Some(ModelInfo { file: file.path.clone(), name, quant, english_only, size: file.size, sha256: file.sha256.clone(), rank })
}

/// The models worth offering: everything usable, smallest/fastest first.
pub fn offered(files: &[RemoteFile]) -> Vec<ModelInfo> {
    let mut models: Vec<ModelInfo> = files.iter().filter_map(describe).collect();
    models.sort_by_key(|m| (m.rank, !m.english_only, m.size));
    models
}

/// Download address for a file on the public host.
pub fn download_url(file: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rf(path: &str, size: u64) -> RemoteFile {
        RemoteFile { path: path.into(), size, sha256: Some("abc".into()) }
    }

    #[test]
    fn describes_the_models_we_ship_and_new_ones() {
        let m = describe(&rf("ggml-large-v3-turbo-q5_0.bin", 574_000_000)).unwrap();
        assert_eq!((m.name.as_str(), m.quant.as_str(), m.english_only), ("Large v3 Turbo", "q5_0", false));
        let m = describe(&rf("ggml-base.en-q5_1.bin", 60_000_000)).unwrap();
        assert_eq!((m.name.as_str(), m.quant.as_str(), m.english_only), ("Base (English)", "q5_1", true));
        let m = describe(&rf("ggml-large-v3.bin", 3_000_000_000)).unwrap();
        assert_eq!((m.name.as_str(), m.quant.as_str()), ("Large v3", "f16"));
        // A model family we have never heard of still shows up, sorted last.
        let future = describe(&rf("ggml-large-v4-q8_0.bin", 1)).unwrap();
        assert_eq!((future.name.as_str(), future.rank), ("Large v4", FAMILIES.len() as u32 + 1));
    }

    #[test]
    fn ignores_files_that_are_not_usable_models() {
        for f in ["ggml-base.en-encoder.mlmodelc.zip", "README.md", "ggml-small.en-tdrz.bin", "for-tests-ggml-base.bin", "ggml-base-encoder-openvino.bin", "model.safetensors"] {
            assert_eq!(describe(&rf(f, 1)), None, "{f}");
        }
    }

    #[test]
    fn offers_smallest_family_first_then_english_then_smaller_files() {
        let files = [rf("ggml-large-v3-turbo-q5_0.bin", 574), rf("ggml-base-q5_1.bin", 57), rf("ggml-base.en-q5_1.bin", 58), rf("ggml-tiny.bin", 75), rf("readme.txt", 1), rf("ggml-base.en.bin", 148)];
        let order: Vec<_> = offered(&files).into_iter().map(|m| m.file).collect();
        assert_eq!(order, vec!["ggml-tiny.bin", "ggml-base.en-q5_1.bin", "ggml-base.en.bin", "ggml-base-q5_1.bin", "ggml-large-v3-turbo-q5_0.bin"]);
    }

    #[test]
    fn download_addresses_point_at_the_public_repository() {
        assert_eq!(download_url("ggml-tiny.bin"), "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin");
    }
}
