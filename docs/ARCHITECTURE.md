# Vox architecture

Local, offline push-to-dictate. Press a hotkey, speak, press it again: polished text is on the clipboard.
Nothing leaves the machine (the webview CSP allows no network origins and no adapter has a network dependency).

## Dependency rule (enforced by `crates/vox-core/tests/architecture.rs`)

```
            src-tauri  (shell: wiring, hotkeys, clipboard, tray, windows)
           /    |     \        \       \
    vox-audio vox-stt vox-store vox-paste vox-keys   adapters: one third-party integration each
           \    |     /
            vox-core                  pure: domain, state machine, ports, engine
```

* `vox-core` depends only on `serde` + `thiserror`. No OS, audio, ML, SQL or UI.
* Adapters depend only on `vox-core` + their one integration, never on each other.
* The shell is the only place that knows about all of them.
* Swapping an engine (e.g. NVIDIA Parakeet) = a new adapter implementing `Transcriber`. Nothing else changes.

## Ports (in `vox-core`)

| Port | Implemented by | Purpose |
|---|---|---|
| `Transcriber` | `vox-stt` (whisper.cpp, English) | 16 kHz audio → text |
| `History` | `vox-store` (SQLite, WAL) | durable recordings + segments |

`vox-keys` watches for the Ctrl+Win chord (modifier-only combos can't be OS hotkeys, so it polls key state
and feeds a pure `ChordDetector` in the core). `vox-paste` presses Ctrl+V in the focused app (clipboard first, so a failed paste never loses text).
Microphone capture (`vox-audio`) is a plain function: it yields 16 kHz mono chunks to a sink.

## The lifecycle is a pure state machine (`vox_core::session`)

`step(state, input, now, grace) -> (state, Option<Effect>)`. No clock, threads or I/O inside, so every rule is a unit test.

```
Loading ──ready──▶ Idle ──toggle──▶ Recording ──toggle──▶ Finalizing ──done──▶ Idle
                                     │   ▲  ▲
                                  Esc│   │Esc (resume)
                                     ▼   │  │
                                  CancelPending ──toggle──▶ Finalizing (keep the text)
                                     │
                                     └──timer expires──▶ Idle   (recording discarded)
```

* Recording **continues** during the cancel countdown, so resuming loses no audio.
* Escape is registered as a global key **only** while Recording/CancelPending (`State::wants_escape`).
* Stale timers are harmless: expiry is checked against the state's own deadline.

## Speed

The engine segments the live stream at natural pauses (`Segmenter`) and transcribes each utterance
**while the user is still talking**. On stop, only the final fragment remains. The model is loaded and
warmed up at startup; the hotkey is ignored (and the UI says so) until it is ready.

## Reliability

* **One writer.** All inputs (hotkeys, timers, engine events, UI button) go through one channel into one
  controller thread (`src-tauri/src/controller.rs`). No locks held across effects, no input races.
* **Ordered audio.** Audio, `Begin`, `Finish` share one command channel; stopping the mic joins its
  thread before `Finish` is sent, so no chunk can arrive late.
* **Text is never lost to a secondary failure.** Each utterance is written to history the moment it is
  recognised. A crash leaves `Recording` rows that `recover_interrupted` turns into recoverable
  entries on next launch. If the database is unusable the app falls back to memory-only and says so.
  If the clipboard write fails the text is still in history.
* **No panics cross threads.** Model loading and each inference run under `catch_unwind` and become
  events. Workspace lints deny `unwrap`/`panic`/`todo` in production code.
* **Everything surfaces.** Errors go to a `problems` list shown in the window (e.g. missing model lists
  every path searched).

## Text quality

whisper.cpp already emits punctuation and casing; an initial prompt biases it toward punctuated prose.
`polish()` then deterministically removes fillers, fixes "i"→"I", capitalises sentences, normalises
spacing and terminal punctuation. Heavier grammar rewriting by an LLM is deliberately out of the MVP.

## Not in the MVP (known, deliberate)

Settings UI / rebinding the hotkey · spooling raw audio to disk for
re-transcription · single-instance lock · Parakeet adapter · macOS/Linux packaging (code is portable, untested).
