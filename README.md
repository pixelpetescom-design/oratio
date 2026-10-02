# Vox

Local, offline push-to-dictate for Windows. press **Ctrl + Win** to start, again to stop: polished English text
is on your clipboard and typed into whatever app you're in (toggle in the window; the clipboard copy always happens). No account, no network, no limits. MIT.

* While recording, **Esc** starts a 3-second cancel countdown; **Esc** again resumes; letting it expire discards the recording.
* Every transcription is saved locally (SQLite) so you can copy it later, even after a crash.
* Closing the window keeps Vox in the tray.

## Install (no tools needed)

Open the repo's **Actions** tab → *Build Windows installer* → the latest run → download **Vox-Windows-GPU-installer** (or **Vox-Windows-installer** for the CPU build), unzip and run the `.exe`.
The speech model is bundled; nothing else to install. The GPU build uses Vulkan (NVIDIA, AMD or Intel graphics) and bundles the larger, more accurate `large-v3-turbo` model; the CPU build uses a small English model. (Run the workflow with **Run workflow** for a fresh build.)

## Build from source (Windows)

Prereqs: Rust (MSVC), Visual Studio Build Tools (C++), CMake, LLVM, WebView2 (included in Windows 11), and `cargo install tauri-cli --version "^2"`.

    powershell -File scripts/get-model.ps1            # add "-Model large" for the GPU build
    cargo tauri build                                  # add "--features gpu" for the GPU build
    #        # installer in target/release/bundle
    cargo tauri dev          # run in development

## Test

    cargo test --workspace
    cargo clippy --workspace --all-targets
    # needs the model file:
    VOX_MODEL=models/ggml-base.en-q5_1.bin cargo test -p vox-stt -- --ignored

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
