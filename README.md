# Vox

Local, offline push-to-dictate for Windows. **Ctrl+Shift+Space** to start, again to stop: polished English text
is on your clipboard, ready to paste. No account, no network, no limits. MIT.

* While recording, **Esc** starts a 3-second cancel countdown; **Esc** again resumes; letting it expire discards the recording.
* Every transcription is saved locally (SQLite) so you can copy it later, even after a crash.
* Closing the window keeps Vox in the tray.

## Build (Windows)

Prereqs: Rust (MSVC), Visual Studio Build Tools (C++), CMake, LLVM, WebView2 (included in Windows 11), and `cargo install tauri-cli --version "^2"`.

    powershell -File scripts/get-model.ps1
    cargo tauri build        # installer in target/release/bundle
    cargo tauri dev          # run in development

## Test

    cargo test --workspace
    cargo clippy --workspace --all-targets
    # needs the model file:
    VOX_MODEL=models/ggml-base.en-q5_1.bin cargo test -p vox-stt -- --ignored

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
