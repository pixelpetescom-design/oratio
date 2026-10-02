const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const LABELS = {
  loading: "Loading speech model…",
  unavailable: "Unavailable",
  idle: "Ready",
  off: "Dictation is off",
  recording: "Listening…",
  cancel_pending: "Cancelling…",
  finalizing: "Transcribing…",
};

let phase = "loading";
let liveText = "";

function renderState(kind, remainingMs) {
  phase = kind;
  $("dot").className = `dot ${kind}`;
  $("status").textContent = kind === "cancel_pending" ? `Cancelling in ${Math.ceil((remainingMs ?? 0) / 1000)}s — Esc to resume` : LABELS[kind] ?? kind;
  $("toggle").textContent = kind === "recording" || kind === "cancel_pending" ? "Stop" : "Start";
  $("toggle").disabled = !["idle", "recording", "cancel_pending"].includes(kind);
  if (kind === "recording" && liveText === "") $("live").textContent = "Listening…";
  $("enabled").checked = kind !== "off";
  $("enabled").disabled = kind !== "idle" && kind !== "off";
  applyStoredEnabled(kind);
}

// "Dictation on/off" is remembered by the UI and re-applied once the engine is ready.
let appliedPreference = false;
function applyStoredEnabled(kind) {
  if (appliedPreference || (kind !== "idle" && kind !== "off")) return;
  appliedPreference = true;
  if (kind === "idle" && localStorage.getItem("dictation") === "off") invoke("set_enabled", { enabled: false });
}

function renderProblems(list) {
  const el = $("problems");
  el.style.display = list.length ? "block" : "none";
  el.textContent = list.join("\n\n");
}

function fmtTime(ms) {
  return new Date(ms).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

async function renderHistory() {
  let entries = [];
  try { entries = await invoke("list_history"); } catch (e) { renderProblems([String(e)]); return; }
  const list = $("list");
  list.replaceChildren();
  $("clear").disabled = entries.length === 0;
  if (!entries.length) {
    const p = document.createElement("div");
    p.className = "empty";
    p.textContent = "Your transcriptions will appear here.";
    list.append(p);
    return;
  }
  for (const e of entries) {
    const card = document.createElement("div");
    card.className = `entry ${e.status}`;
    const meta = document.createElement("div");
    meta.className = "meta";
    const when = document.createElement("span");
    when.className = "grow";
    when.textContent = fmtTime(e.started_at_ms) + (e.status === "failed" ? " · recovered" : e.status === "recording" ? " · in progress" : "");
    const copy = document.createElement("button");
    copy.textContent = "Copy";
    copy.onclick = async () => { await invoke("copy_text", { text: e.text }); copy.textContent = "Copied ✓"; setTimeout(() => (copy.textContent = "Copy"), 1200); };
    const edit = document.createElement("button");
    edit.textContent = "Edit";
    edit.onclick = () => startEdit(card, e);
    const del = document.createElement("button");
    del.textContent = "Delete";
    del.onclick = async () => { await invoke("delete_entry", { id: e.id }); renderHistory(); };
    meta.append(when, copy, edit, del);
    const text = document.createElement("div");
    text.className = "text";
    text.textContent = e.text || "(no text)";
    card.append(meta, text);
    list.append(card);
  }
}

function startEdit(card, entry) {
  const box = document.createElement("textarea");
  box.value = entry.text;
  const save = document.createElement("button");
  save.textContent = "Save & learn";
  const cancel = document.createElement("button");
  cancel.textContent = "Cancel";
  const row = document.createElement("div");
  row.className = "meta";
  row.append(save, cancel);
  card.querySelector(".text").replaceWith(box);
  card.append(row);
  box.focus();
  cancel.onclick = () => renderHistory();
  save.onclick = async () => {
    try {
      const learned = await invoke("edit_entry", { id: entry.id, text: box.value });
      $("live").textContent = learned ? `Saved. Learned ${learned} correction${learned === 1 ? "" : "s"}.` : "Saved.";
    } catch (err) { renderProblems([String(err)]); }
    renderHistory();
    renderLexicon();
  };
}

function chip(label, onRemove) {
  const c = document.createElement("span");
  c.className = "chip";
  c.append(label);
  const x = document.createElement("button");
  x.textContent = "×";
  x.title = "Remove";
  x.onclick = onRemove;
  c.append(x);
  return c;
}

async function renderLexicon() {
  let lex;
  try { lex = await invoke("list_lexicon"); } catch (e) { renderProblems([String(e)]); return; }
  $("words").replaceChildren(...lex.words.map((w) => chip(w, async () => { await invoke("remove_word", { word: w }); renderLexicon(); })));
  $("fixes").replaceChildren(...lex.fixes.map((f) => chip(`${f.from} → ${f.to}`, async () => { await invoke("remove_fix", { from: f.from }); renderLexicon(); })));
}

$("wordform").onsubmit = async (ev) => {
  ev.preventDefault();
  const word = $("wordinput").value.trim();
  if (!word) return;
  $("wordinput").value = "";
  try { await invoke("add_word", { word }); } catch (e) { renderProblems([String(e)]); }
  renderLexicon();
};

$("toggle").onclick = () => invoke("toggle_recording");
$("enabled").onchange = () => {
  localStorage.setItem("dictation", $("enabled").checked ? "on" : "off");
  invoke("set_enabled", { enabled: $("enabled").checked });
};

// Clearing is permanent, so the first click arms the button and the second one confirms.
let disarm = null;
function resetClear() {
  clearTimeout(disarm);
  $("clear").classList.remove("armed");
  $("clear").textContent = "Clear all text";
}
$("clear").onclick = async () => {
  if (!$("clear").classList.contains("armed")) {
    $("clear").classList.add("armed");
    $("clear").textContent = "Click again to delete everything";
    disarm = setTimeout(resetClear, 3000);
    return;
  }
  resetClear();
  try { await invoke("clear_history"); } catch (e) { renderProblems([String(e)]); }
  liveText = "";
  $("live").textContent = "Cleared.";
  renderHistory();
};

// The preference lives in the UI's storage and is pushed to the backend on load and on change.
const stored = localStorage.getItem("autopaste");
$("autopaste").checked = stored === null ? true : stored === "true";
invoke("set_auto_paste", { enabled: $("autopaste").checked });
$("autopaste").onchange = () => {
  localStorage.setItem("autopaste", String($("autopaste").checked));
  invoke("set_auto_paste", { enabled: $("autopaste").checked });
};

let countdown = null;
listen("state", ({ payload }) => {
  clearInterval(countdown);
  renderState(payload.kind, payload.remaining_ms);
  if (payload.kind === "cancel_pending") {
    const end = Date.now() + payload.remaining_ms;
    countdown = setInterval(() => renderState("cancel_pending", Math.max(0, end - Date.now())), 200);
  }
  if (payload.kind === "recording" && phase !== "cancel_pending") { liveText = ""; }
});
listen("segment", ({ payload }) => { liveText += (liveText ? " " : "") + payload; $("live").textContent = liveText; });
listen("finished", ({ payload }) => {
  $("live").textContent = payload.text ? (payload.pasted ? "Typed: " : payload.copied ? "Copied to clipboard: " : "") + payload.text + (payload.elapsed_ms != null ? `  (${(payload.elapsed_ms / 1000).toFixed(1)} s)` : "") : "No speech detected.";
  liveText = "";
});
listen("problem", ({ payload }) => invoke("get_snapshot").then((s) => renderProblems(s.problems)));
listen("history", renderHistory);

(async () => {
  const s = await invoke("get_snapshot");
  $("hotkey").textContent = s.hotkey;
  renderState(s.state.kind, null);
  renderProblems(s.problems);
  renderHistory();
  renderLexicon();
})();
