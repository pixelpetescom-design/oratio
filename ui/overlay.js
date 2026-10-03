const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const wave = Wave.create($("wave"));

let countdown = null;

function show(mode, text) {
  wave.setMode(mode);
  $("label").textContent = text;
}

function took(p) {
  return p.elapsed_ms != null ? ` · ${(p.elapsed_ms / 1000).toFixed(1)} s` : "";
}

listen("state", ({ payload }) => {
  clearInterval(countdown);
  switch (payload.kind) {
    case "recording": show("listening", "Listening…"); break;
    case "finalizing": show("thinking", "Transcribing…"); break;
    case "cancel_pending": {
      const end = Date.now() + payload.remaining_ms;
      const tick = () => show("cancel", `Cancelling in ${Math.ceil(Math.max(0, end - Date.now()) / 1000)}s · Esc to resume`);
      tick();
      countdown = setInterval(tick, 200);
      break;
    }
    case "idle": break; // the "finished" event sets the closing message
  }
});

listen("level", ({ payload }) => wave.setLevel(payload));

listen("finished", ({ payload }) => {
  if (!payload.text) show("idle", "No speech detected");
  else show("idle", payload.searched ? "Searching ✓" : payload.copy_only ? "Copied — typing off for this app" : payload.pasted ? "Typed into your app ✓" + took(payload) : payload.copied ? "Copied — ready to paste" + took(payload) : "Done (clipboard unavailable — see history)");
});
listen("scratched", ({ payload }) => show("idle", payload ? "Undone ✓" : "Nothing to undo"));
listen("problem", () => show("idle", "Something went wrong — open Oratio"));
