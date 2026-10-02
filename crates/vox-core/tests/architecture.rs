#![allow(clippy::unwrap_used, clippy::panic)]
//! Guardrail: enforces the dependency rule from docs/ARCHITECTURE.md.
//! `vox-core` is pure; each adapter depends only on `vox-core` plus its one
//! third-party integration; adapters never depend on each other or on the shell.

use std::collections::BTreeSet;
use std::fs;

fn deps(crate_dir: &str) -> BTreeSet<String> {
    let path = format!("{}/../{crate_dir}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut in_deps = false;
    let mut out = BTreeSet::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_deps = line == "[dependencies]";
        } else if in_deps && !line.is_empty() && !line.starts_with('#') {
            if let Some((name, _)) = line.split_once('=') {
                out.insert(name.trim().to_string());
            }
        }
    }
    out
}

fn assert_exactly(crate_dir: &str, allowed: &[&str]) {
    let actual = deps(crate_dir);
    let allowed: BTreeSet<String> = allowed.iter().map(|s| s.to_string()).collect();
    let extra: Vec<_> = actual.difference(&allowed).collect();
    assert!(extra.is_empty(), "{crate_dir} has forbidden dependencies: {extra:?} (allowed: {allowed:?})");
}

#[test]
fn core_is_pure() {
    assert_exactly("vox-core", &["serde", "thiserror"]);
}

#[test]
fn adapters_depend_only_on_core_and_their_integration() {
    assert_exactly("vox-audio", &["vox-core", "cpal"]);
    assert_exactly("vox-stt", &["vox-core", "whisper-rs"]);
    assert_exactly("vox-store", &["vox-core", "rusqlite"]);
}
