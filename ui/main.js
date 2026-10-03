const { invoke: rawInvoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// The window can finish loading before the app's back end has finished setting up (history, settings,
// hotkeys). Every call waits for that, so saved settings are applied reliably at startup instead of
// failing silently with "state not managed".
const backendReady = (async () => {
  for (let i = 0; i < 300; i++) {
    try { await rawInvoke("get_snapshot"); return; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
})();
const invoke = async (cmd, args) => { await backendReady; return rawInvoke(cmd, args); };

const $ = (id) => document.getElementById(id);
const wave = Wave.create($("wave"));
const WAVE_MODES = { recording: "listening", finalizing: "thinking", cancel_pending: "cancel" };
const LABELS = {
  loading: "Loading speech model…",
  unavailable: "Unavailable",
  idle: "Ready",
  off: "Dictation is off",
  recording: "Listening…",
  cancel_pending: "Cancelling…",
  finalizing: "Transcribing…",
};

const ICONS = {
  mic: '<svg viewBox="0 0 24 24"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/></svg>',
  stop: '<svg viewBox="0 0 24 24"><rect x="6.5" y="6.5" width="11" height="11" rx="2.5"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="11" height="11" rx="2.5"/><path d="M5 15V6a2 2 0 0 1 2-2h8"/></svg>',
  edit: '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
};
function icon(name) {
  const t = document.createElement("template");
  t.innerHTML = ICONS[name];
  return t.content.firstElementChild;
}

window.addEventListener("unhandledrejection", (e) => { toast(String(e.reason ?? "Something went wrong"), "bad"); });

// ---- navigation: icon rail, one view at a time ---------------------------------------------

function showView(name) {
  document.querySelectorAll(".view").forEach((v) => (v.hidden = v.id !== `view-${name}`));
  document.querySelectorAll(".nav-btn").forEach((b) => {
    const on = b.dataset.view === name;
    b.classList.toggle("active", on);
    b.setAttribute("aria-current", on ? "page" : "false");
  });
  if (name === "history") $("navHistory").classList.remove("badge");
  try { localStorage.setItem("view", name); } catch { /* storage unavailable */ }
  window.dispatchEvent(new Event("resize")); // the wave canvas needs a re-measure once visible
}
document.querySelectorAll(".nav-btn").forEach((b) => (b.onclick = () => showView(b.dataset.view)));
showView(["dictate", "history", "words", "settings"].includes(localStorage.getItem("view")) ? localStorage.getItem("view") : "dictate");

// ---- small interaction helpers -------------------------------------------------------------

function toast(message, kind = "ok") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  $("toasts").append(el);
  setTimeout(() => el.classList.add("out"), 2200);
  setTimeout(() => el.remove(), 2600);
}

// Ripple on any pressed button.
document.addEventListener("pointerdown", (e) => {
  const b = e.target.closest(".btn, .icon-btn, .orb");
  if (!b || b.disabled) return;
  const r = b.getBoundingClientRect();
  const size = Math.max(r.width, r.height);
  const rip = document.createElement("span");
  rip.className = "ripple";
  rip.style.cssText = `width:${size}px;height:${size}px;left:${e.clientX - r.left - size / 2}px;top:${e.clientY - r.top - size / 2}px`;
  b.append(rip);
  setTimeout(() => rip.remove(), 600);
});

function iconButton(name, title, onClick, extra = "") {
  const b = document.createElement("button");
  b.className = `icon-btn ${extra}`.trim();
  b.title = title;
  b.setAttribute("aria-label", title);
  b.append(icon(name));
  b.onclick = onClick;
  return b;
}

// ---- state ---------------------------------------------------------------------------------

let phase = "loading";
let liveText = "";
const latencies = []; // seconds from Stop to text, this session

function renderState(kind, remainingMs) {
  if (kind === "loading") appliedPreference = false;
  phase = kind;
  wave.setMode(WAVE_MODES[kind] ?? "idle");
  $("dot").className = `dot ${kind}`;
  $("status").textContent = kind === "cancel_pending" ? `Cancelling in ${Math.ceil((remainingMs ?? 0) / 1000)}s — Esc to resume` : LABELS[kind] ?? kind;

  const orb = $("toggle");
  const active = kind === "recording" || kind === "cancel_pending";
  orb.className = `orb ${kind}`;
  orb.replaceChildren(icon(active ? "stop" : "mic"));
  orb.disabled = !["idle", "recording", "cancel_pending"].includes(kind);
  document.querySelector(".ring").classList.toggle("spin", kind === "finalizing");

  if (kind === "recording" && liveText === "") setLive("Listening…");
  $("enabled").checked = kind !== "off";
  $("enabled").disabled = kind !== "idle" && kind !== "off";
  applyStoredEnabled(kind);
}

function setLive(text, hasText = false) {
  $("live").textContent = text;
  $("live").classList.toggle("has-text", hasText);
}

// "Dictation on/off" is remembered here and re-applied once the engine is ready.
let appliedPreference = false;
function applyStoredEnabled(kind) {
  if (appliedPreference || (kind !== "idle" && kind !== "off")) return;
  appliedPreference = true;
  if (kind === "idle" && localStorage.getItem("dictation") === "off") invoke("set_enabled", { enabled: false });
}

function renderProblems(list) {
  const el = $("problems");
  el.title = "Click to expand";
  el.onclick = () => el.classList.toggle("expanded");
  el.style.display = list.length ? "block" : "none";
  el.textContent = list.join("\n\n");
}

// Ring around the orb follows the mic level.
const CIRC = 339.29;
function tickRing() {
  const arc = $("arc");
  const lit = phase === "recording" || phase === "cancel_pending" ? 0.1 + 0.9 * wave.getLevel() : 0;
  arc.style.strokeDashoffset = String(CIRC * (1 - lit));
  requestAnimationFrame(tickRing);
}
requestAnimationFrame(tickRing);

// ---- speed stats ---------------------------------------------------------------------------

const GAUGE = 314.16;
function renderStats() {
  const last = latencies[latencies.length - 1];
  $("speed").textContent = last == null ? "–" : `${last.toFixed(1)}s`;
  // Full ring = instant, empty = 3 s or slower.
  $("gaugeArc").style.strokeDashoffset = String(GAUGE * (last == null ? 1 : Math.min(1, last / 3)));
  const bars = $("bars");
  bars.replaceChildren();
  const recent = latencies.slice(-8);
  const max = Math.max(1, ...recent);
  for (let i = 0; i < 8 - recent.length; i++) {
    const e = document.createElement("div");
    e.className = "bar empty";
    bars.append(e);
  }
  for (const v of recent) {
    const b = document.createElement("div");
    b.className = "bar";
    b.style.height = `${Math.max(8, (v / max) * 100)}%`;
    b.title = `${v.toFixed(1)} s`;
    bars.append(b);
  }
}

// ---- history -------------------------------------------------------------------------------

function fmtTime(ms) {
  return new Date(ms).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

let entryCount = 0;
async function renderHistory() {
  let entries = [];
  try { entries = await invoke("list_history"); } catch (e) { renderProblems([String(e)]); return; }
  entryCount = entries.length;
  $("clear").disabled = entries.length === 0;
  if (entries.length === 0) closeConfirm();
  const list = $("list");
  list.replaceChildren();
  if (!entries.length) {
    const p = document.createElement("div");
    p.className = "empty";
    p.textContent = "Your transcriptions will appear here.";
    list.append(p);
    return;
  }
  entries.forEach((e, i) => {
    const card = document.createElement("div");
    card.className = `card entry ${e.status}`;
    card.style.animationDelay = `${Math.min(i, 8) * 35}ms`;
    const meta = document.createElement("div");
    meta.className = "meta";
    const when = document.createElement("span");
    when.className = "grow";
    when.textContent = fmtTime(e.started_at_ms) + (e.status === "failed" ? " · recovered" : e.status === "recording" ? " · in progress" : "");
    const copy = iconButton("copy", "Copy", async () => {
      await invoke("copy_text", { text: e.text });
      copy.classList.add("ok");
      copy.replaceChildren(icon("check"));
      toast("Copied to clipboard");
      setTimeout(() => { copy.classList.remove("ok"); copy.replaceChildren(icon("copy")); }, 1400);
    });
    const edit = iconButton("edit", "Edit — Oratio learns from your corrections", () => startEdit(card, e));
    const del = iconButton("trash", "Delete", async () => {
      card.style.transition = "opacity 0.25s, transform 0.25s";
      card.style.opacity = "0";
      card.style.transform = "translateX(24px)";
      await invoke("delete_entry", { id: e.id });
      setTimeout(renderHistory, 220);
    }, "bad");
    meta.append(when, copy, edit, del);
    const text = document.createElement("div");
    text.className = "text";
    text.textContent = e.text || "(no text)";
    card.append(meta, text);
    list.append(card);
  });
}

function startEdit(card, entry) {
  const box = document.createElement("textarea");
  box.value = entry.text;
  const save = document.createElement("button");
  save.className = "btn primary small";
  save.textContent = "Save & learn";
  const cancel = document.createElement("button");
  cancel.className = "btn outline small";
  cancel.textContent = "Cancel";
  const row = document.createElement("div");
  row.className = "edit-row";
  row.append(save, cancel);
  card.querySelector(".text").replaceWith(box);
  card.querySelector(".meta").querySelectorAll(".icon-btn").forEach((b) => (b.disabled = true));
  card.append(row);
  box.focus();
  cancel.onclick = () => renderHistory();
  save.onclick = async () => {
    try {
      const learned = await invoke("edit_entry", { id: entry.id, text: box.value });
      toast(learned ? `Saved — learned ${learned} correction${learned === 1 ? "" : "s"}` : "Saved");
    } catch (err) { renderProblems([String(err)]); }
    renderHistory();
    renderLexicon();
  };
}

// Clear all: an explicit confirmation panel (no hidden double-click).
function closeConfirm() { $("confirm").hidden = true; }
$("clear").onclick = () => {
  $("confirmText").textContent = `Delete all ${entryCount} dictation${entryCount === 1 ? "" : "s"}? This can't be undone.`;
  $("confirm").hidden = false;
};
$("confirmNo").onclick = closeConfirm;
$("confirmYes").onclick = async () => {
  closeConfirm();
  try {
    const n = await invoke("clear_history");
    toast(`Cleared ${n} dictation${n === 1 ? "" : "s"}`);
  } catch (e) { renderProblems([String(e)]); }
  liveText = "";
  setLive("Cleared.");
  renderHistory();
};

// ---- vocabulary ----------------------------------------------------------------------------

function chip(label, onRemove, long = false) {
  const c = document.createElement("span");
  c.className = `chip${long ? " long" : ""}`;
  const text = document.createElement("span");
  text.className = "chip-text";
  text.textContent = label;
  text.title = label;
  c.append(text);
  const x = document.createElement("button");
  x.append(icon("x"));
  x.querySelector("svg").style.cssText = "width:12px;height:12px;stroke:currentColor;fill:none;stroke-width:2.4;stroke-linecap:round";
  x.title = "Remove";
  x.onclick = onRemove;
  c.append(x);
  return c;
}

async function renderLexicon() {
  let lex;
  try { lex = await invoke("list_lexicon"); } catch (e) { renderProblems([String(e)]); return; }
  $("words").replaceChildren(...lex.words.map((w) => chip(w, async () => { await invoke("remove_word", { word: w }); renderLexicon(); })));
  $("snippets").replaceChildren(...lex.snippets.map((s) => chip(`${s.from} → ${s.to.replace(/\n/g, " ⏎ ")}`, async () => { await invoke("remove_snippet", { trigger: s.from }); renderLexicon(); }, true)));
  $("fixes").replaceChildren(...lex.fixes.map((f) => chip(`${f.from} → ${f.to}`, async () => { await invoke("remove_fix", { from: f.from }); renderLexicon(); })));
}

$("snippetform").onsubmit = async (ev) => {
  ev.preventDefault();
  try {
    await invoke("add_snippet", { trigger: $("snippetTrigger").value, text: $("snippetText").value });
    $("snippetTrigger").value = "";
    $("snippetText").value = "";
    toast("Snippet added");
  } catch (e) { toast(String(e), "bad"); }
  renderLexicon();
};

$("wordform").onsubmit = async (ev) => {
  ev.preventDefault();
  const word = $("wordinput").value.trim();
  if (!word) return;
  $("wordinput").value = "";
  try { await invoke("add_word", { word }); toast(`Added “${word}”`); } catch (e) { renderProblems([String(e)]); }
  renderLexicon();
};

// ---- switches ------------------------------------------------------------------------------

$("toggle").onclick = () => invoke("toggle_recording");

$("enabled").onchange = () => {
  localStorage.setItem("dictation", $("enabled").checked ? "on" : "off");
  invoke("set_enabled", { enabled: $("enabled").checked });
};

const storedPaste = localStorage.getItem("autopaste");
$("autopaste").checked = storedPaste === null ? true : storedPaste === "true";
invoke("set_auto_paste", { enabled: $("autopaste").checked });
$("autopaste").onchange = () => {
  localStorage.setItem("autopaste", String($("autopaste").checked));
  invoke("set_auto_paste", { enabled: $("autopaste").checked });
};

// Window style (solid / clear / soft / frosted) and tint. The old on/off "glass" setting maps to soft/solid.
const STYLES = ["solid", "clear", "soft", "frosted"];
let glassStyle = localStorage.getItem("glassStyle") ?? (localStorage.getItem("glass") === "false" ? "solid" : "soft");
if (!STYLES.includes(glassStyle)) glassStyle = "soft";
let tint = Number(localStorage.getItem("tint") ?? 55);

function paintTint() {
  $("tint").value = String(tint);
  $("tint").style.setProperty("--fill", `${((tint - 10) / 85) * 100}%`);
  $("tintValue").textContent = `${tint}%`;
  document.body.style.setProperty("--tint", String(tint / 100));
}
async function applyGlass() {
  document.querySelectorAll("#glassStyle button").forEach((b) => b.classList.toggle("active", b.dataset.style === glassStyle));
  let seeThrough = false;
  try { seeThrough = await invoke("set_glass", { style: glassStyle }); } catch { /* unsupported: stay solid */ }
  document.body.classList.toggle("glass", seeThrough);
  $("tint").disabled = !seeThrough;
}
document.querySelectorAll("#glassStyle button").forEach((b) => (b.onclick = () => {
  glassStyle = b.dataset.style;
  localStorage.setItem("glassStyle", glassStyle);
  applyGlass();
}));
$("tint").oninput = () => {
  tint = Number($("tint").value);
  localStorage.setItem("tint", String(tint));
  paintTint();
};
paintTint();
applyGlass();

// Voice search: hold Shift or Alt with Ctrl + Win to search for what you say.
let searchOn = localStorage.getItem("search") === "on";
let searchKey = localStorage.getItem("searchKey") === "alt" ? "alt" : "shift";
let searchEngine = localStorage.getItem("searchEngine") ?? "google";
let searchCustom = localStorage.getItem("searchCustom") ?? "";
function applySearch() {
  $("search").checked = searchOn;
  document.querySelector(".switch-group").classList.toggle("off", !searchOn);
  document.querySelectorAll("#searchKey button").forEach((b) => b.classList.toggle("active", b.dataset.key === searchKey));
  $("searchEngine").value = searchEngine;
  $("searchCustom").hidden = searchEngine !== "custom";
  $("searchCustom").value = searchCustom;
  invoke("set_search", { enabled: searchOn, key: searchKey, engine: searchEngine, custom: searchCustom });
}
$("search").onchange = () => {
  searchOn = $("search").checked;
  localStorage.setItem("search", searchOn ? "on" : "off");
  applySearch();
  if (searchOn) toast(`Hold ${searchKey === "alt" ? "Alt" : "Shift"} + Ctrl + Win to search`);
};
document.querySelectorAll("#searchKey button").forEach((b) => (b.onclick = () => {
  searchKey = b.dataset.key;
  localStorage.setItem("searchKey", searchKey);
  applySearch();
}));
$("searchEngine").onchange = () => {
  searchEngine = $("searchEngine").value;
  localStorage.setItem("searchEngine", searchEngine);
  applySearch();
};
$("searchCustom").onchange = () => {
  searchCustom = $("searchCustom").value.trim();
  localStorage.setItem("searchCustom", searchCustom);
  if (searchCustom && !(searchCustom.startsWith("https://") && searchCustom.includes("{q}"))) toast("Use an https:// address containing {q}", "bad");
  applySearch();
};
applySearch();

// Hold-to-talk and spoken commands.
let holdTalk = localStorage.getItem("holdTalk") === "on";
let spoken = localStorage.getItem("spoken") !== "off";
function applyBehaviour() {
  $("holdtalk").checked = holdTalk;
  $("spoken").checked = spoken;
  invoke("set_behaviour", { spokenCommands: spoken, holdToTalk: holdTalk });
}
$("holdtalk").onchange = () => {
  holdTalk = $("holdtalk").checked;
  localStorage.setItem("holdTalk", holdTalk ? "on" : "off");
  applyBehaviour();
  if (holdTalk) toast("Hold Ctrl + Win to talk, let go to finish");
};
$("spoken").onchange = () => {
  spoken = $("spoken").checked;
  localStorage.setItem("spoken", spoken ? "on" : "off");
  applyBehaviour();
};
applyBehaviour();

// Microphone picker. The system default is "" and is also the fallback if the chosen one is unplugged.
async function loadMics() {
  let names = [];
  try { names = await invoke("list_microphones"); } catch { /* none listed */ }
  const saved = localStorage.getItem("mic") ?? "";
  const sel = $("mic");
  sel.replaceChildren(new Option("System default", ""), ...names.map((n) => new Option(n, n)));
  sel.value = names.includes(saved) ? saved : "";
  invoke("set_microphone", { name: sel.value });
}
$("mic").onchange = () => {
  localStorage.setItem("mic", $("mic").value);
  invoke("set_microphone", { name: $("mic").value });
  toast($("mic").value ? "Microphone changed" : "Using the system default microphone");
};
$("mic").onfocus = loadMics; // pick up headsets plugged in since launch
loadMics();

// Per-app rules.
const RULE_LABELS = { enter_after: "press Enter", copy_only: "copy only" };
async function renderRules() {
  let rules = [];
  try { rules = await invoke("list_app_rules"); } catch (e) { renderProblems([String(e)]); return; }
  $("rules").replaceChildren(...rules.map((r) => chip(`${r.pattern} → ${RULE_LABELS[r.action] ?? r.action}`, async () => { await invoke("remove_app_rule", { pattern: r.pattern }); renderRules(); })));
}
$("ruleform").onsubmit = async (ev) => {
  ev.preventDefault();
  try {
    await invoke("add_app_rule", { pattern: $("rulePattern").value, action: $("ruleAction").value });
    $("rulePattern").value = "";
    toast("Rule added");
  } catch (e) { toast(String(e), "bad"); }
  renderRules();
};
renderRules();

// Wave position: eight preset spots, or "drag it anywhere". Saved here and re-applied at startup.
let overlayPos = { kind: "preset", id: "top_center" };
try { overlayPos = JSON.parse(localStorage.getItem("overlayPos")) ?? overlayPos; } catch { /* keep default */ }
function paintOverlayGrid() {
  document.querySelectorAll("#posGrid button").forEach((b) => b.classList.toggle("active", overlayPos.kind === "preset" && b.dataset.pos === overlayPos.id));
}
function applyOverlayPosition() {
  paintOverlayGrid();
  invoke("set_overlay_position", { kind: overlayPos.kind, id: overlayPos.id ?? "top_center", x: overlayPos.x ?? 0, y: overlayPos.y ?? 0 });
}
document.querySelectorAll("#posGrid button").forEach((b) => (b.onclick = async () => {
  if (b.dataset.pos === "move") {
    $("moveBar").hidden = false;
    await invoke("begin_move_overlay");
    return;
  }
  overlayPos = { kind: "preset", id: b.dataset.pos };
  localStorage.setItem("overlayPos", JSON.stringify(overlayPos));
  applyOverlayPosition();
  invoke("preview_overlay");
}));
listen("overlay-moved", ({ payload: [x, y] }) => {
  overlayPos = { kind: "custom", x, y };
  localStorage.setItem("overlayPos", JSON.stringify(overlayPos));
  paintOverlayGrid();
});
$("moveDone").onclick = async () => {
  const [x, y] = await invoke("end_move_overlay");
  overlayPos = { kind: "custom", x, y };
  localStorage.setItem("overlayPos", JSON.stringify(overlayPos));
  $("moveBar").hidden = true;
  applyOverlayPosition();
  toast("Wave position saved");
};
applyOverlayPosition();

// Speech models: installed ones work offline; "Check for new models" is the only thing that goes online.
let models = [];
const downloads = {}; // file -> { done, total }
const mb = (n) => (n >= 1000 ? `${(n / 1000).toFixed(1)} GB` : `${n} MB`);
const canSwitch = () => phase === "idle" || phase === "off";

function renderModels() {
  const list = $("models");
  list.replaceChildren();
  if (!models.length) {
    const p = document.createElement("div");
    p.className = "model-note";
    p.textContent = "No models found.";
    list.append(p);
    return;
  }
  for (const m of models) {
    const row = document.createElement("div");
    row.className = `model-row${m.active ? " active" : ""}`;
    const top = document.createElement("div");
    top.className = "top";
    const title = document.createElement("span");
    title.className = "title";
    title.textContent = m.name;
    title.title = m.file;
    const actions = document.createElement("span");
    actions.className = "actions";
    const dl = downloads[m.file];
    if (dl) {
      const cancel = document.createElement("button");
      cancel.className = "btn outline small";
      cancel.textContent = "Cancel";
      cancel.onclick = () => invoke("cancel_model_download");
      actions.append(cancel);
    } else if (m.installed && !m.active) {
      const use = document.createElement("button");
      use.className = "btn primary small";
      use.textContent = "Use";
      use.disabled = !canSwitch();
      use.title = canSwitch() ? "" : "Finish dictating first";
      use.onclick = async () => {
        try { await invoke("use_model", { file: m.file }); toast("Loading the speech model…"); } catch (e) { toast(String(e), "bad"); }
      };
      actions.append(use);
      if (!m.bundled) {
        const del = document.createElement("button");
        del.className = "btn outline danger small";
        del.textContent = "Delete";
        del.onclick = async () => {
          try { await invoke("delete_model", { file: m.file }); toast("Model deleted"); } catch (e) { toast(String(e), "bad"); }
          refreshModels();
        };
        actions.append(del);
      }
    } else if (!m.installed) {
      const get = document.createElement("button");
      get.className = "btn primary small";
      get.textContent = `Download ${mb(m.size_mb)}`;
      get.onclick = async () => {
        downloads[m.file] = { done: 0, total: m.size_mb * 1_000_000 };
        renderModels();
        try { await invoke("download_model", { file: m.file }); } catch (e) { delete downloads[m.file]; toast(String(e), "bad"); renderModels(); }
      };
      actions.append(get);
    }
    top.append(title, actions);

    const tags = document.createElement("div");
    tags.className = "tags";
    const add = (text, cls = "") => { const t = document.createElement("span"); t.className = `tag ${cls}`.trim(); t.textContent = text; tags.append(t); };
    if (m.active) add("In use", "use");
    if (m.is_new) add("New", "new");
    if (m.recommended) add("Recommended", "rec");
    if (m.english_only) add("English");
    if (m.quant) add(m.quant);
    add(mb(m.size_mb));
    row.append(top, tags);

    if (dl) {
      const bar = document.createElement("div");
      bar.className = "progress";
      const fill = document.createElement("i");
      fill.style.width = `${dl.total ? Math.min(100, (dl.done / dl.total) * 100) : 0}%`;
      fill.dataset.file = m.file;
      bar.append(fill);
      row.append(bar);
    }
    list.append(row);
  }
}

async function refreshModels() {
  try {
    const installed = await invoke("list_models");
    // Keep what a previous check found, but refresh which of those are installed/in use.
    const byFile = new Map(installed.map((m) => [m.file, m]));
    const merged = models.map((m) => (byFile.has(m.file) ? { ...m, ...byFile.get(m.file), is_new: m.is_new } : { ...m, installed: false, active: false }));
    for (const m of installed) if (!merged.some((x) => x.file === m.file)) merged.push(m);
    models = merged;
  } catch (e) { renderProblems([String(e)]); }
  renderModels();
}

$("checkModels").onclick = async () => {
  const button = $("checkModels");
  button.disabled = true;
  button.textContent = "Checking…";
  try {
    models = await invoke("check_model_updates");
    const fresh = models.filter((m) => m.is_new).length;
    toast(fresh ? `${fresh} new model${fresh === 1 ? "" : "s"} available` : "You have the latest models");
  } catch (e) {
    toast("Couldn't reach the model host — are you online?", "bad");
  }
  button.disabled = false;
  button.textContent = "Check for new models";
  renderModels();
};

listen("model-progress", ({ payload }) => {
  const dl = downloads[payload.file];
  if (!dl) return;
  dl.done = payload.done;
  dl.total = payload.total || dl.total;
  const fill = document.querySelector(`.progress > i[data-file="${CSS.escape(payload.file)}"]`);
  if (fill) fill.style.width = `${Math.min(100, (dl.done / dl.total) * 100)}%`;
});
listen("model-downloaded", ({ payload: file }) => {
  delete downloads[file];
  toast("Download complete — press Use to switch to it");
  refreshModels();
});
listen("model-download-failed", ({ payload: [file, cancelled, message] }) => {
  delete downloads[file];
  toast(cancelled ? "Download cancelled" : `Download failed: ${message}`, cancelled ? "ok" : "bad");
  renderModels();
});
listen("model-ready", ({ payload: ok }) => {
  toast(ok ? "Speech model ready" : "Couldn't load that model; the previous one is still in use", ok ? "ok" : "bad");
  refreshModels();
});
invoke("list_models").then((m) => { models = m; renderModels(); }).catch(() => {});

// Launch at sign-in: the OS is the source of truth, so ask it rather than remembering locally.
invoke("get_autostart").then((on) => ($("autostart").checked = on)).catch(() => ($("autostart").disabled = true));
$("autostart").onchange = async () => {
  try {
    const on = await invoke("set_autostart", { enabled: $("autostart").checked });
    $("autostart").checked = on;
    toast(on ? "Oratio will start with Windows" : "Oratio won't start with Windows");
  } catch (e) {
    $("autostart").checked = !$("autostart").checked;
    toast("Couldn't change start-up setting", "bad");
  }
};

// ---- backend events ------------------------------------------------------------------------

let countdown = null;
listen("state", ({ payload }) => {
  clearInterval(countdown);
  renderState(payload.kind, payload.remaining_ms);
  if (payload.kind === "cancel_pending") {
    const end = Date.now() + payload.remaining_ms;
    countdown = setInterval(() => renderState("cancel_pending", Math.max(0, end - Date.now())), 200);
  }
  if (payload.kind === "recording" && phase !== "cancel_pending") liveText = "";
});
listen("level", ({ payload }) => wave.setLevel(payload));
listen("segment", ({ payload }) => {
  liveText += (liveText ? " " : "") + payload;
  setLive(liveText, true);
});
listen("finished", ({ payload }) => {
  if (!payload.text) setLive("No speech detected.");
  else {
    setLive(payload.text, true);
    toast(payload.searched ? "Searching…" : payload.copy_only ? "Copied — typing is off for this app" : payload.pasted ? "Typed into your app" : payload.copied ? "Copied to clipboard" : "Saved to history", payload.searched || payload.pasted || payload.copied ? "ok" : "bad");
  }
  if (payload.elapsed_ms != null && payload.text) {
    latencies.push(payload.elapsed_ms / 1000);
    renderStats();
  }
  liveText = "";
  if (payload.text && !$("navHistory").classList.contains("active")) $("navHistory").classList.add("badge");
});
listen("scratched", ({ payload }) => {
  setLive(payload ? "Undid your last dictation." : "Nothing recent to undo.");
  toast(payload ? "Undid last dictation" : "Nothing recent to undo", payload ? "ok" : "bad");
});
listen("problem", () => invoke("get_snapshot").then((s) => renderProblems(s.problems)));
listen("history", renderHistory);

(async () => {
  const s = await invoke("get_snapshot");
  $("hotkey").textContent = s.hotkey;
  renderState(s.state.kind, null);
  renderProblems(s.problems);
  renderStats();
  renderHistory();
  renderLexicon();
})();
