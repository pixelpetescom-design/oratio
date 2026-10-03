//! The only part of Oratio that can talk to the internet, and only when the user asks: it lists the
//! models published on the public whisper.cpp repository and downloads one, checking it against the
//! checksum the host publishes. Nothing the user says or types is ever sent.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use oratio_core::models::RemoteFile;
use oratio_core::CoreError;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Where models are listed and downloaded from.
#[derive(Debug, Clone)]
pub struct Hub {
    /// Returns a JSON array describing the files in the repository.
    pub listing: String,
    /// Prefix for downloads: `{files}/{file name}`.
    pub files: String,
}

impl Hub {
    pub fn huggingface() -> Hub {
        Hub {
            listing: "https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main".into(),
            files: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main".into(),
        }
    }
}

fn net_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Network(e.to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).user_agent("oratio").build()
}

/// Turns the host's listing into files with sizes and published checksums.
pub fn parse_listing(listing: &serde_json::Value) -> Result<Vec<RemoteFile>, CoreError> {
    let items = listing.as_array().ok_or_else(|| net_err("unexpected reply from the model host"))?;
    Ok(items
        .iter()
        .filter(|i| i["type"] == "file")
        .filter_map(|i| {
            let path = i["path"].as_str()?.to_string();
            // Large files are stored with Git LFS, which carries the real size and SHA-256.
            let size = i["lfs"]["size"].as_u64().or_else(|| i["size"].as_u64())?;
            let sha256 = i["lfs"]["oid"].as_str().map(str::to_lowercase);
            Some(RemoteFile { path, size, sha256 })
        })
        .collect())
}

/// Lists everything in the repository (the caller decides which files are usable models).
pub fn list(hub: &Hub) -> Result<Vec<RemoteFile>, CoreError> {
    let body: serde_json::Value = agent().get(&hub.listing).call().map_err(net_err)?.into_json().map_err(net_err)?;
    parse_listing(&body)
}

/// File names come from the network, so they must be plain model file names, never paths.
fn is_plain_model_name(name: &str) -> bool {
    !name.is_empty() && name.ends_with(".bin") && !name.contains(['/', '\\', ':']) && !name.contains("..")
}

/// Downloads `file` into `dest_dir`, reporting `(done, total)` bytes, and keeps it only if it matches
/// the published checksum. The download goes to a `.part` file first, so a failed or cancelled
/// attempt never leaves a half-written model that could be mistaken for a good one.
pub fn download(
    hub: &Hub,
    file: &RemoteFile,
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, CoreError> {
    if !is_plain_model_name(&file.path) {
        return Err(net_err(format!("refusing to download {:?}", file.path)));
    }
    fs::create_dir_all(dest_dir).map_err(net_err)?;
    let (part, dest) = (dest_dir.join(format!("{}.part", file.path)), dest_dir.join(&file.path));

    let outcome = (|| -> Result<(), CoreError> {
        let response = agent().get(&format!("{}/{}", hub.files, file.path)).call().map_err(net_err)?;
        let total = response.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(file.size);
        let mut reader = response.into_reader();
        let mut out = File::create(&part).map_err(net_err)?;
        let (mut hasher, mut done, mut buf) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024]);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(net_err("cancelled"));
            }
            let n = reader.read(&mut buf).map_err(net_err)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(net_err)?;
            done += n as u64;
            progress(done, total);
        }
        out.flush().map_err(net_err)?;
        match &file.sha256 {
            Some(expected) if format!("{:x}", hasher.finalize()) != *expected => Err(net_err("the download did not match its checksum; it was discarded")),
            _ => Ok(()),
        }
    })();

    match outcome {
        Ok(()) => {
            fs::rename(&part, &dest).map_err(net_err)?;
            Ok(dest)
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// A throwaway local web server: each route is (path, body).
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap_or(0);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone());
                let (status, body) = match body {
                    Some(b) => ("200 OK", b),
                    None => ("404 Not Found", b"missing".to_vec()),
                };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    fn hub(base: &str) -> Hub {
        Hub { listing: format!("{base}/listing"), files: format!("{base}/files") }
    }

    fn sha_of(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oratio-models-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn parses_the_hosts_listing_using_lfs_size_and_checksum() {
        let json = serde_json::json!([
            {"type": "file", "path": "ggml-base.bin", "size": 134, "lfs": {"oid": "ABC123", "size": 147_951_465}},
            {"type": "file", "path": "README.md", "size": 900},
            {"type": "directory", "path": "models"}
        ]);
        let files = parse_listing(&json).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], RemoteFile { path: "ggml-base.bin".into(), size: 147_951_465, sha256: Some("abc123".into()) });
        assert_eq!(files[1].sha256, None);
        assert!(parse_listing(&serde_json::json!({"error": "nope"})).is_err());
    }

    #[test]
    fn lists_and_downloads_a_model_with_progress() {
        let data = vec![7u8; 300_000];
        let base = serve(vec![("/listing", br#"[{"type":"file","path":"ggml-tiny.bin","size":300000}]"#.to_vec()), ("/files/ggml-tiny.bin", data.clone())]);
        let hub = hub(&base);
        let files = list(&hub).unwrap();
        assert_eq!(files[0].path, "ggml-tiny.bin");

        let dir = temp("ok");
        let file = RemoteFile { path: "ggml-tiny.bin".into(), size: 300_000, sha256: Some(sha_of(&data)) };
        let mut last = (0, 0);
        let path = download(&hub, &file, &dir, &AtomicBool::new(false), |d, t| last = (d, t)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), data);
        assert_eq!(last, (300_000, 300_000));
        assert!(!dir.join("ggml-tiny.bin.part").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_download_is_rejected_and_leaves_nothing_behind() {
        let base = serve(vec![("/files/ggml-tiny.bin", vec![1u8; 1000])]);
        let dir = temp("bad");
        let file = RemoteFile { path: "ggml-tiny.bin".into(), size: 1000, sha256: Some(sha_of(b"something else")) };
        let err = download(&hub(&base), &file, &dir, &AtomicBool::new(false), |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("checksum"));
        assert!(!dir.join("ggml-tiny.bin").exists() && !dir.join("ggml-tiny.bin.part").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_stops_the_download_and_cleans_up() {
        let base = serve(vec![("/files/ggml-tiny.bin", vec![1u8; 500_000])]);
        let dir = temp("cancel");
        let file = RemoteFile { path: "ggml-tiny.bin".into(), size: 500_000, sha256: None };
        let err = download(&hub(&base), &file, &dir, &AtomicBool::new(true), |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert!(!dir.join("ggml-tiny.bin.part").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hostile_file_names_are_refused_before_any_request() {
        let dir = temp("names");
        for name in ["../evil.bin", "a/b.bin", "C:\\x.bin", "model.exe", "..\\..\\x.bin", ""] {
            let file = RemoteFile { path: name.into(), size: 1, sha256: None };
            assert!(download(&Hub { listing: String::new(), files: "http://127.0.0.1:1".into() }, &file, &dir, &AtomicBool::new(false), |_, _| {}).is_err(), "{name:?}");
        }
        assert!(!dir.exists());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_crash() {
        let base = serve(vec![]);
        let dir = temp("404");
        let file = RemoteFile { path: "ggml-nope.bin".into(), size: 1, sha256: None };
        assert!(download(&hub(&base), &file, &dir, &AtomicBool::new(false), |_, _| {}).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
