const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let level = 0;
let countdown = null;

function show(kind, text) {
  $("dot").className = `dot ${kind}`;
  $("label").textContent = text;
}

listen("state", ({ payload }) => {
  clearInterval(countdown);
  switch (payload.kind) {
    case "recording": show("recording", "Listening…"); break;
    case "finalizing": show("finalizing", "Transcribing…"); break;
    case "cancel_pending": {
      const end = Date.now() + payload.remaining_ms;
      const tick = () => show("cancel_pending", `Cancelling in ${Math.ceil(Math.max(0, end - Date.now()) / 1000)}s · Esc to resume`);
      tick();
      countdown = setInterval(tick, 200);
      break;
    }
    case "idle": break; // the "finished" event sets the closing message
  }
  if (payload.kind !== "recording") $("bar").style.width = "0";
});

listen("level", ({ payload }) => {
  level = level * 0.5 + Math.min(1, Math.sqrt(payload)) * 0.5;
  $("bar").style.width = `${Math.round(level * 100)}%`;
});

function took(p) {
  return p.elapsed_ms != null ? ` · ${(p.elapsed_ms / 1000).toFixed(1)} s` : "";
}

listen("finished", ({ payload }) => {
  if (!payload.text) show("idle", "No speech detected");
  else show("idle", payload.pasted ? "Typed into your app ✓" + took(payload) : payload.copied ? "Copied — ready to paste" + took(payload) : "Done (clipboard unavailable — see history)");
  $("bar").style.width = "0";
});
listen("problem", () => show("unavailable", "Something went wrong — open Vox"));
