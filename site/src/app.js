// dredge.sh is one terminal. This file is the shell: the dot bar in its header, the theme
// picker, command cards, the tutorial, the commands, and the folder demo's output (the reading
// lives in demo.js, loaded on first use). No dependencies, no network. The one thing it stores
// is the theme you pick.

const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;
const REPO = "https://github.com/CinematicGenius007/dredge";
const INSTALL = `cargo install --git ${REPO} dredge-cli`;
const CLONE = `git clone ${REPO} && cd dredge && cargo install --path crates/dredge-cli`;
const MCP_ADD = "claude mcp add dredge --scope user -- dredge mcp";
const GiB = 1024 ** 3;

// ------------------------------------------------------------------------------------------
// Dither primitives, ported from dredge-render (Bayer matrix, density ramp, column widths).

const BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
// Category inks come from the theme (--c-<key> in styles.css); these are only a fallback.
const INK = {
  apps: "#78A9F0", developer: "#9BCB7A", photos: "#EE8DC0", movies: "#B39DF3", documents: "#6CCFC6",
  music: "#E6C46A", system: "#6E6F6B", caches: "#F29E5C", trash: "#F07178", other: "#6E6F6B", free: "#3A3D41",
};
function readInk() {
  const cs = getComputedStyle(document.documentElement);
  for (const k of Object.keys(INK)) { const v = cs.getPropertyValue(`--c-${k}`).trim(); if (v) INK[k] = v; }
}
readInk();
const LABEL = {
  apps: "Apps", developer: "Developer", photos: "Photos", movies: "Movies", documents: "Documents",
  music: "Music", system: "System", caches: "Caches", trash: "Trash", other: "Other", free: "Free",
};

/** Cells per segment, proportional to bytes, summing to `width` (largest remainder). */
function widths(bytes, width) {
  const total = bytes.reduce((a, b) => a + b, 0);
  if (!total || width <= 0) return bytes.map(() => 0);
  const exact = bytes.map((b) => (b / total) * width);
  const out = exact.map((e, i) => (bytes[i] > 0 ? Math.max(1, Math.floor(e)) : 0));
  let left = width - out.reduce((a, b) => a + b, 0);
  const order = exact.map((e, i) => [e - Math.floor(e), i]).sort((a, b) => b[0] - a[0]);
  for (let k = 0; left > 0 && k < order.length * 4; k++) { const i = order[k % order.length][1]; if (bytes[i] > 0) { out[i]++; left--; } }
  while (left < 0) { const i = out.indexOf(Math.max(...out)); out[i]--; left++; }
  return out;
}

function human(bytes) {
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0, v = bytes;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${v} B` : `${v.toFixed(1)} ${u[i]}`;
}

// A canvas of dots in dredge's ramp: sparse at the top, solid at the bottom, the focused
// segment densest. It is still until you point at it; a focus change starts at the pointer's
// column and spreads outward quickly.
class DotField {
  static all = new Set();
  static redrawAll() { for (const f of DotField.all) f.c.isConnected ? f.draw() : DotField.all.delete(f); }
  constructor(canvas, { segments = [], pitch = 6, radius = 1.25, levels } = {}) {
    this.c = canvas; this.ctx = canvas.getContext("2d");
    this.pitch = pitch; this.radius = radius;
    DotField.all.add(this);
    this.levels = { focus: 16, other: 6, none: 12, free: 2, ...levels };
    this.segments = segments; this.focus = null; this.cols = 0;
    this.cur = new Float32Array(0); this.tgt = new Float32Array(0); this.start = new Float64Array(0); this.segAt = new Int16Array(0);
    this.visible = true; this.raf = 0;
    this.resize();
    new ResizeObserver(() => this.resize()).observe(canvas);
    new IntersectionObserver(([e]) => { this.visible = e.isIntersecting; if (this.visible) this.kick(); }).observe(canvas);
  }
  resize() {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    const w = this.c.clientWidth, h = this.c.clientHeight;
    if (!w || !h) return;
    this.c.width = Math.round(w * dpr); this.c.height = Math.round(h * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.cols = Math.floor(w / this.pitch); this.rows = Math.max(1, Math.floor(h / this.pitch));
    this.ox = (w - this.cols * this.pitch) / 2 + this.pitch / 2;
    this.oy = (h - this.rows * this.pitch) / 2 + this.pitch / 2;
    this.cur = new Float32Array(this.cols); this.tgt = new Float32Array(this.cols); this.start = new Float64Array(this.cols);
    this.layout(null);
    this.cur.set(this.tgt);
    this.draw();
  }
  setSegments(segments, fromCol = 0) { this.segments = segments; this.focus = null; this.layout(fromCol); }
  layout(fromCol) {
    const cells = widths(this.segments.map((s) => s.bytes), Math.floor(this.cols / 2));
    this.cells = cells;
    this.segAt = new Int16Array(this.cols).fill(-1);
    let x = 0;
    cells.forEach((n, i) => { for (let k = 0; k < n * 2 && x < this.cols; k++) this.segAt[x++] = i; });
    this.retarget(fromCol);
  }
  level(i) {
    if (i < 0 || this.segments[i]?.key === "free") return this.levels.free;
    if (this.focus == null) return this.levels.none;
    return i === this.focus ? this.levels.focus : this.levels.other;
  }
  retarget(fromCol) {
    const now = performance.now();
    for (let x = 0; x < this.cols; x++) {
      this.tgt[x] = this.level(this.segAt[x]);
      this.start[x] = fromCol == null || REDUCED ? now : now + Math.abs(x - fromCol) * 1.5;
    }
    if (REDUCED) this.cur.set(this.tgt);
    this.kick();
  }
  setFocus(i, fromCol) { if (i !== this.focus) { this.focus = i; this.retarget(fromCol ?? null); } }
  centerOf(i) { let c = 0; for (let k = 0; k < i; k++) c += this.cells[k] * 2; return c + this.cells[i]; }
  hit(clientX) {
    const r = this.c.getBoundingClientRect();
    const col = Math.max(0, Math.min(this.cols - 1, Math.floor((clientX - r.left - this.ox + this.pitch / 2) / this.pitch)));
    return { col, seg: this.segAt[col] };
  }
  kick() { if (!this.raf && this.visible) this.raf = requestAnimationFrame((t) => this.frame(t)); }
  frame(t) {
    this.raf = 0;
    let moving = false;
    for (let x = 0; x < this.cols; x++) {
      if (t < this.start[x]) { moving = true; continue; }
      const d = this.tgt[x] - this.cur[x];
      if (Math.abs(d) > 0.02) { this.cur[x] += d * 0.3; moving = true; } else this.cur[x] = this.tgt[x];
    }
    this.draw();
    if (moving && this.visible) this.raf = requestAnimationFrame((tt) => this.frame(tt));
  }
  draw() {
    const { ctx, cols, rows, pitch, radius, ox, oy } = this;
    ctx.clearRect(0, 0, this.c.width, this.c.height);
    const last = Math.max(1, rows - 1);
    let color = null;
    for (let x = 0; x < cols; x++) {
      const s = this.segAt[x];
      const c = s < 0 ? INK.free : INK[this.segments[s].key] || INK.other;
      if (c !== color) { if (color) ctx.fill(); ctx.fillStyle = color = c; ctx.beginPath(); }
      const lvl = this.cur[x];
      for (let y = 0; y < rows; y++) {
        const d = Math.min(16, (lvl * (last + 3 * y)) / (4 * last));
        if (BAYER[y & 3][x & 3] < d) { const px = ox + x * pitch, py = oy + y * pitch; ctx.moveTo(px + radius, py); ctx.arc(px, py, radius, 0, 6.2832); }
      }
    }
    if (color) ctx.fill();
  }
}

// ------------------------------------------------------------------------------------------
// Command cards. A command you can read before you copy it: point at a part (or focus the card
// and step with ← →) and a branch drops from that part to a note on what it does. Variants
// (Cargo or a clone, one MCP client or another) sit in the header. Copying selects the line the
// way a terminal does, then lets go.

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const sp = (cls, t) => `<span class="${cls}">${esc(t)}</span>`;
const ICONS = `<svg class="i-copy" aria-hidden="true"><use href="#i-copy"/></svg><svg class="i-check" aria-hidden="true"><use href="#i-check"/></svg>`;

/** Shell highlighting. `st.expect` carries "the next word is a command" across calls. */
function hlShell(src, st = { expect: true }) {
  let out = "";
  for (const m of src.matchAll(/(\s+)|('[^']*'|"[^"]*")|(&&|\|\||\||;)|(#.*$)|(\S+)/g)) {
    const [t, ws, str, op, com, word] = m;
    if (ws) out += t;
    else if (str) { out += sp("t-str", t); st.expect = false; }
    else if (op) { out += sp("t-op", t); st.expect = true; }
    else if (com) out += sp("d", t);
    else if (word) {
      if (st.expect && /^[A-Za-z_]\w*=/.test(word)) out += sp("t-key", word);
      else if (st.expect) { out += sp("t-cmd", word); st.expect = false; }
      else if (word === "--") out += sp("t-op", word);
      else if (word.startsWith("-")) out += sp("t-flag", word);
      else if (/^https?:\/\//.test(word)) out += sp("t-str", word);
      else out += esc(word);
    }
  }
  return out;
}
function hlJson(src) {
  let out = "", last = 0;
  for (const m of src.matchAll(/("(?:[^"\\]|\\.)*")(\s*:)?|([{}[\],:])/g)) {
    out += esc(src.slice(last, m.index)); last = m.index + m[0].length;
    if (m[1]) out += m[2] ? sp("t-key", m[1]) + sp("t-punc", m[2]) : sp("t-str", m[1]);
    else out += sp("t-punc", m[3]);
  }
  return out + esc(src.slice(last));
}

const HOVER = matchMedia("(hover: hover)").matches;
const cardData = new Map();
let cardSeq = 0;
const textOf = (v) => v.json ?? v.parts.map((p) => p[0]).join(" ");

/** variants: [{ label?, parts: [[text, note]], idle? } | { label?, json, idle }] */
function card({ title, variants, ink }) {
  const id = String(++cardSeq);
  cardData.set(id, variants);
  const seg = variants.length > 1
    ? `<div class="xc-seg" role="group" aria-label="${esc(title)} variants">${variants.map((v, i) =>
      `<button type="button" data-v="${i}" aria-pressed="${i === 0}">${esc(v.label)}</button>`).join("")}</div>`
    : "";
  return `<div class="xc${ink ? ` ink-${ink}` : ""}" data-card="${id}" data-v="0" tabindex="0" role="group" aria-label="${esc(title)}. Arrow keys explain each part; c copies.">` +
    `<div class="xc-head"><span class="xc-title">${esc(title)}</span>${seg}` +
    `<button type="button" class="xc-copy" aria-label="Copy">${ICONS}<span class="lbl">copy</span><kbd>c</kbd></button></div>` +
    `<div class="xc-body">${cardBody(variants[0])}</div></div>`;
}
function cardBody(v) {
  let code;
  if (v.json) code = `<pre class="xc-code"><code>${hlJson(v.json)}</code></pre>`;
  else {
    const st = { expect: true };
    code = `<div class="xc-code"><span class="ps" aria-hidden="true">$</span><code class="xc-cmd">${v.parts.map(([t], i) =>
      `<span class="tok" data-i="${i}">${hlShell(t, st)}</span>`).join(" ")}</code></div>`;
  }
  return `${code}<p class="xc-note" aria-live="polite"><span class="arm" aria-hidden="true">└</span><span class="what">${idleNote(v)}</span></p>`;
}
const idleNote = (v) => v.json ? v.idle
  : `${v.idle ? `${v.idle} · ` : ""}${sp("d", HOVER ? "point at any part to see what it does" : "tap any part to see what it does")}`;

function cardState(c) { return cardData.get(c.dataset.card)[+c.dataset.v]; }

/** Light up part i and hang its note from it. null puts the card back at rest. */
function explain(c, i) {
  const v = cardState(c);
  const cmd = c.querySelector(".xc-cmd"), note = c.querySelector(".xc-note");
  if (!cmd || c.classList.contains("copied")) return;
  const toks = [...cmd.querySelectorAll(".tok")];
  toks.forEach((t, k) => t.classList.toggle("on", k === i));
  cmd.classList.toggle("has-on", i != null);
  c._on = i;
  note.classList.toggle("on", i != null);
  note.style.paddingLeft = "";
  if (i == null) { note.querySelector(".arm").textContent = "└"; note.querySelector(".what").innerHTML = idleNote(v); return; }
  const tok = toks[i];
  tok.scrollIntoView({ block: "nearest", inline: "nearest" });
  const body = c.querySelector(".xc-body").getBoundingClientRect(), r = tok.getBoundingClientRect();
  const x = Math.max(0, Math.min(r.left - body.left - 14 + Math.min(r.width / 2, 12), body.width * 0.55));
  note.style.paddingLeft = `${x}px`;
  note.querySelector(".arm").textContent = "└─";
  note.querySelector(".what").textContent = v.parts[i][1];
}

function setVariant(c, i) {
  c.dataset.v = String(i); c._on = null;
  c.querySelectorAll(".xc-seg button").forEach((b) => b.setAttribute("aria-pressed", String(+b.dataset.v === i)));
  c.querySelector(".xc-body").innerHTML = cardBody(cardState(c));
}

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch {
    const t = Object.assign(document.createElement("textarea"), { value: text });
    t.style.position = "fixed"; t.style.opacity = "0";
    document.body.append(t); t.select();
    const ok = document.execCommand("copy"); t.remove(); return ok;
  }
}
async function copyCard(c) {
  const v = cardState(c);
  if (!(await copyText(textOf(v)))) return;
  explain(c, null);
  c.classList.remove("copied"); void c.offsetWidth; c.classList.add("copied");
  c.querySelector(".xc-copy .lbl").textContent = "copied";
  const note = c.querySelector(".xc-note");
  note.querySelector(".arm").textContent = "✓";
  note.querySelector(".what").innerHTML = v.json ? `copied · ${esc(v.file || "paste it into your config")}` : "copied · paste it into your terminal";
  clearTimeout(c._t);
  c._t = setTimeout(() => {
    c.classList.remove("copied");
    c.querySelector(".xc-copy .lbl").textContent = "copy";
    explain(c, null);
  }, 1500);
}

document.addEventListener("mouseover", (e) => {
  const t = e.target.closest?.(".xc .tok");
  if (t) explain(t.closest(".xc"), +t.dataset.i);
});
document.addEventListener("mouseout", (e) => {
  const c = e.target.closest?.(".xc");
  if (c && !c.contains(e.relatedTarget) && document.activeElement !== c) explain(c, null);
});
document.addEventListener("click", (e) => {
  const c = e.target.closest(".xc");
  if (!c) return;
  const seg = e.target.closest(".xc-seg button"), tok = e.target.closest(".tok");
  if (seg) setVariant(c, +seg.dataset.v);
  else if (e.target.closest(".xc-copy")) copyCard(c);
  else if (tok) explain(c, +tok.dataset.i);
});
document.addEventListener("keydown", (e) => {
  const c = e.target.classList?.contains("xc") ? e.target : null;
  if (!c) return;
  const n = c.querySelectorAll(".tok").length;
  if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
    if (!n) return;
    e.preventDefault();
    const d = e.key === "ArrowRight" ? 1 : -1;
    explain(c, c._on == null ? (d > 0 ? 0 : n - 1) : (c._on + d + n) % n);
  } else if ((e.key === "c" && !e.metaKey && !e.ctrlKey) || ((e.metaKey || e.ctrlKey) && e.key === "c" && !getSelection().toString())) {
    e.preventDefault(); copyCard(c);
  } else if (e.key === "Escape") explain(c, null);
});
document.addEventListener("focusout", (e) => { if (e.target.classList?.contains("xc") && !e.target.contains(e.relatedTarget)) explain(e.target, null); });

const REPO_SHORT = "github.com/CinematicGenius007/dredge";
const INSTALL_CARD = {
  title: "install", ink: "developer",
  variants: [
    { label: "cargo", idle: "needs cargo from rustup.rs", parts: [
      ["cargo install", "builds a Rust program and puts its binary in ~/.cargo/bin"],
      [`--git ${REPO}`, `builds straight from ${REPO_SHORT}; no release needed`],
      ["dredge-cli", "the package to build. It installs one binary: dredge"],
    ] },
    { label: "from a clone", idle: "read the code first, then build it", parts: [
      [`git clone ${REPO}`, "downloads the source into ./dredge"],
      ["&&", "runs the next command only if the last one worked"],
      ["cd dredge", "steps into the checkout"],
      ["&&", "runs the next command only if the last one worked"],
      ["cargo install --path crates/dredge-cli", "builds the CLI from this checkout into ~/.cargo/bin"],
    ] },
  ],
};
const MCP_CARD = {
  title: "add dredge to an agent", ink: "apps",
  variants: [
    { label: "Claude Code", parts: [
      ["claude mcp add", "registers an MCP server with Claude Code"],
      ["dredge", "the name your agent will see"],
      ["--scope user", "for every project. --scope project keeps it to this one"],
      ["--", "everything after this is the command Claude Code starts"],
      ["dredge mcp", "dredge's read-only MCP server, spoken over stdio"],
    ] },
    { label: "Claude Desktop", file: "paste it into claude_desktop_config.json, then restart Claude",
      idle: `claude_desktop_config.json · use the full path from ${sp("t-cmd", "which dredge")}; Claude Desktop doesn't read your PATH`,
      json: '{\n  "mcpServers": {\n    "dredge": { "command": "/Users/you/.cargo/bin/dredge", "args": ["mcp"] }\n  }\n}' },
    { label: "Cursor", file: "paste it into ~/.cursor/mcp.json",
      idle: "~/.cursor/mcp.json · Cursor starts it on its next launch",
      json: '{\n  "mcpServers": {\n    "dredge": { "command": "dredge", "args": ["mcp"] }\n  }\n}' },
    { label: "any client", idle: "point any MCP client at this command", parts: [
      ["DREDGE_FOCUS_ROOTS=~/work:/", "optional: the folders agents may scan, separated by colons"],
      ["dredge mcp", "a read-only MCP server on stdio"],
    ] },
  ],
};
/** The command dredge prints for a hint, explained. */
function hintCard(path, contents) {
  const q = `'${path.replace(/'/g, "'\\''")}'`;
  return {
    title: "suggested · dredge never runs this", ink: "caches",
    variants: [{ idle: "read it before you run it", parts: contents ? [
      ["find", "walks the folder"],
      [q, "the folder dredge flagged. It stays; what is inside it goes"],
      ["-xdev", "stays on this disk and never crosses into another volume"],
      ["-mindepth 1", "skips the folder itself, so only its contents match"],
      ["-delete", "deletes each match. Permanent: nothing goes to the Trash"],
    ] : [
      ["rm -rf", "removes the folder and everything in it, permanently"],
      ["--", "ends the options, so a path that starts with - is never read as a flag"],
      [q, "the folder dredge flagged"],
    ] }],
  };
}

// ------------------------------------------------------------------------------------------
// Themes. Pick the colors your own terminal uses. Point at one to try it, click to keep it.
// The choice is the only thing this site stores (localStorage, this browser only).

const THEMES = [
  ["auto", "Auto", "follows your system"], ["graphite", "Graphite", "dark"], ["paper", "Paper", "light"],
  ["dracula", "Dracula"], ["solarized-dark", "Solarized Dark"], ["solarized-light", "Solarized Light"],
  ["nord", "Nord"], ["gruvbox", "Gruvbox"], ["tokyo-night", "Tokyo Night"], ["catppuccin", "Catppuccin Mocha"],
  ["one-dark", "One Dark"], ["homebrew", "Homebrew"], ["pro", "Pro"], ["basic", "Basic", "light"],
];

const theme = (() => {
  const KEY = "dredge-theme";
  const root = document.documentElement;
  const btn = document.getElementById("theme-btn"), menu = document.getElementById("theme-menu");
  const label = document.getElementById("theme-name"), meta = document.querySelector('meta[name="theme-color"]');
  const light = matchMedia("(prefers-color-scheme: light)");
  const ids = THEMES.map(([id]) => id);
  let pref = "auto";
  try { pref = localStorage.getItem(KEY) || "auto"; } catch {}
  if (!ids.includes(pref)) pref = "auto";
  let act = 0;

  const resolve = (id) => (id === "auto" ? (light.matches ? "paper" : "graphite") : id);
  const nameOf = (id) => THEMES.find((t) => t[0] === id)?.[1] ?? id;
  function apply(id) {
    const t = resolve(id);
    if (root.dataset.theme !== t) root.dataset.theme = t;
    readInk();
    DotField.redrawAll();
    meta?.setAttribute("content", getComputedStyle(root).getPropertyValue("--bg").trim());
  }
  function set(id) {
    pref = id;
    try { id === "auto" ? localStorage.removeItem(KEY) : localStorage.setItem(KEY, id); } catch {}
    apply(id);
    label.textContent = nameOf(id).toLowerCase();
    menu.querySelectorAll("li").forEach((li) => li.setAttribute("aria-selected", String(li.dataset.id === id)));
  }
  menu.innerHTML = THEMES.map(([id, name, sub]) =>
    `<li role="option" id="th-${id}" data-id="${id}" aria-selected="${id === pref}"><span class="pv" data-theme="${resolve(id)}" aria-hidden="true"><b>$</b><i></i><i></i><i></i><i></i><i></i><i></i></span><span>${esc(name)}</span>${sub ? `<span class="d">${esc(sub)}</span>` : `<span class="d"></span>`}</li>`).join("");
  const items = [...menu.children];

  function activate(k, preview = true) {
    act = (k + items.length) % items.length;
    items.forEach((li, j) => li.classList.toggle("act", j === act));
    menu.setAttribute("aria-activedescendant", items[act].id);
    items[act].scrollIntoView({ block: "nearest" });
    if (preview) apply(items[act].dataset.id);
  }
  function open() {
    menu.hidden = false; btn.setAttribute("aria-expanded", "true");
    activate(ids.indexOf(pref), false);
    menu.focus({ preventScroll: true });
  }
  function close(commit) {
    if (menu.hidden) return;
    if (commit != null) set(commit); else apply(pref);
    menu.hidden = true; btn.setAttribute("aria-expanded", "false");
  }
  btn.addEventListener("click", () => (menu.hidden ? open() : close()));
  menu.addEventListener("mouseover", (e) => { const li = e.target.closest("li"); if (li) activate(items.indexOf(li)); });
  menu.addEventListener("mouseleave", () => apply(pref));
  menu.addEventListener("click", (e) => { const li = e.target.closest("li"); if (li) { close(li.dataset.id); btn.focus({ preventScroll: true }); } });
  menu.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); activate(act + (e.key === "ArrowDown" ? 1 : -1)); }
    else if (e.key === "Home" || e.key === "End") { e.preventDefault(); activate(e.key === "Home" ? 0 : items.length - 1); }
    else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); close(items[act].dataset.id); btn.focus({ preventScroll: true }); }
    else if (e.key === "Escape") { e.preventDefault(); close(); btn.focus({ preventScroll: true }); }
    else if (e.key === "Tab") close();
  });
  document.addEventListener("pointerdown", (e) => { if (!e.target.closest(".themes")) close(); });
  light.addEventListener("change", () => { if (pref === "auto") apply("auto"); });
  label.textContent = nameOf(pref).toLowerCase();
  apply(pref);

  return {
    command(arg) {
      const want = arg?.toLowerCase();
      if (want) {
        const id = ids.find((x) => x === want || nameOf(x).toLowerCase().replace(/\s+/g, "-") === want);
        if (!id) return o.lines([`${sp("e", "✗")} no theme called ${esc(arg)}`, `  ${sp("f", "type")} ${runLink("theme")} ${sp("f", "for the list")}`]);
        set(id);
        return o.line(`${sp("ok", "✓")} theme ${sp("b", nameOf(id))} ${sp("f", "· saved in this browser")}`);
      }
      o.html(`<dl class="kv">${THEMES.map(([id, name, sub]) =>
        `<dt>${runLink(`theme ${id}`)}</dt><dd><span class="pv" data-theme="${resolve(id)}" aria-hidden="true"><b>$</b><i></i><i></i><i></i><i></i><i></i><i></i></span> ${esc(name)}${sub ? ` ${sp("d", "· " + sub)}` : ""}${id === pref ? ` ${sp("ok", "✓")}` : ""}</dd>`).join("")}</dl>`);
      o.line(sp("f", "Or use the picker at the top right: point at a theme to try it, click to keep it."));
    },
  };
})();

// ------------------------------------------------------------------------------------------
// HUD: the bar in the terminal's header. Hover a color to focus it, click to pin it.

const SAMPLE = [
  { key: "developer", gib: 152.8, note: "node_modules, target/, .venv, DerivedData" },
  { key: "photos", gib: 88.4, note: "a Photos library, RAW and HEIC" },
  { key: "movies", gib: 61.2, note: "screen recordings and exports" },
  { key: "other", gib: 54.6, note: "everything without a clearer home" },
  { key: "apps", gib: 41.7, note: "~/Applications and .app bundles" },
  { key: "documents", gib: 23.9, note: "PDFs, docs, notes" },
  { key: "caches", gib: 19.6, note: "~/Library/Caches · ◆ apps re-create it" },
  { key: "music", gib: 9.3, note: "a music library" },
  { key: "trash", gib: 4.1, note: "◆ already thrown away" },
].map((s) => ({ ...s, bytes: s.gib * GiB }));

const hud = (() => {
  const canvas = document.getElementById("hud-bar");
  const legend = document.getElementById("legend");
  const pointer = document.getElementById("pointer");
  const name = pointer.querySelector(".seg-name"), size = pointer.querySelector(".seg-size"), note = pointer.querySelector(".seg-note");
  const field = new DotField(canvas, { segments: SAMPLE, pitch: 6, radius: 1.3 });
  let segs = SAMPLE, root = "~/", idle = "a sample home folder · hover a color", pinned = null, chips = [];

  function load(list, rootName, idleNote) {
    segs = list; root = rootName; idle = idleNote; pinned = null;
    field.setSegments(list, 0);
    legend.innerHTML = "";
    chips = list.map((s, i) => {
      const li = document.createElement("li"), b = document.createElement("button");
      b.type = "button"; b.className = `ink-${s.key}`;
      b.setAttribute("aria-pressed", "false");
      b.innerHTML = `<i></i>${esc(LABEL[s.key])} <span class="d">${esc(human(s.bytes).replace(" GiB", ""))}</span>`;
      b.addEventListener("mouseenter", () => show(i));
      b.addEventListener("focus", () => show(i));
      b.addEventListener("mouseleave", () => show(pinned));
      b.addEventListener("blur", () => show(pinned));
      b.addEventListener("click", () => pin(i));
      li.append(b); legend.append(li);
      return b;
    });
    show(null);
  }
  function show(i, col) {
    field.setFocus(i, col ?? (i == null ? null : field.centerOf(i)));
    chips.forEach((c, k) => c.classList.toggle("focus", k === i));
    const total = segs.reduce((a, s) => a + s.bytes, 0);
    if (i == null) { name.textContent = root; name.style.color = ""; size.textContent = human(total); note.textContent = idle; return; }
    const s = segs[i];
    name.textContent = LABEL[s.key]; name.style.color = `var(--c-${s.key})`;
    size.textContent = `${human(s.bytes)} · ${Math.round((s.bytes / total) * 100)}%`;
    note.textContent = (s.note ? s.note + " · " : "") + (pinned === i ? `pinned · dredge bar ${root} --focus ${s.key}` : "click to pin");
  }
  function pin(i, col) {
    pinned = pinned === i ? null : i;
    chips.forEach((c, k) => c.setAttribute("aria-pressed", String(k === pinned)));
    show(pinned ?? i, col);
  }
  canvas.addEventListener("pointermove", (e) => { const { seg, col } = field.hit(e.clientX); if (seg >= 0) show(seg, col); });
  canvas.addEventListener("pointerleave", () => show(pinned));
  canvas.addEventListener("click", (e) => {
    const { seg, col } = field.hit(e.clientX);
    if (seg >= 0) pin(seg, col);
  });
  load(SAMPLE, "~/", "a sample home folder · hover a color");
  return { load };
})();

// ------------------------------------------------------------------------------------------
// The shell

const out = document.getElementById("out");
const log = document.getElementById("log");
const form = document.getElementById("prompt");
const input = document.getElementById("cmd");
const chipsEl = document.getElementById("chips");
const progress = document.getElementById("progress");
const term = document.getElementById("term");
const pick = document.getElementById("pick");

// Each command gets its own screen: a section at least as tall as the scrollback, scrolled to
// the top. Earlier commands stay above it (scroll up to see them), dimmed.
new ResizeObserver(() => out.style.setProperty("--screen", `${out.clientHeight}px`)).observe(out);
const o = {
  run: null,
  wait: (ms) => new Promise((r) => setTimeout(r, REDUCED ? 0 : ms)),
  screen(cmdHtml) {
    const sec = document.createElement("section");
    sec.className = "run";
    log.append(sec);
    this.run = sec;
    this.line(cmdHtml, "cmdline");
  },
  scroll() { if (this.run) out.scrollTop = this.run.offsetTop - parseFloat(getComputedStyle(out).paddingTop); },
  line(html = "", cls = "") {
    const p = document.createElement("p");
    p.className = `line in ${cls}`.trim(); p.innerHTML = html || "&nbsp;";
    (this.run || log).append(p); this.scroll(); return p;
  },
  html(html, cls = "") {
    const d = document.createElement("div");
    d.className = `blk in ${cls}`.trim(); d.innerHTML = html;
    (this.run || log).append(d); this.scroll(); return d;
  },
  async lines(list, gap = 25) { for (const l of list) { this.line(l); await this.wait(gap); } },
};
const runLink = (cmd, text = cmd) => `<button type="button" class="run-link" data-run="${esc(cmd)}">${esc(text)}</button>`;
const next = (cmd, why) => `${sp("f", "Next")}  ${runLink(cmd)}  ${sp("f", why)}`;
const comment = (t) => o.line(esc(`# ${t}`), "d");

// ---- tutorial ------------------------------------------------------------------------------

const pad = (s, n) => String(s).padStart(n);
const bar10 = (f) => { const on = Math.max(f > 0 ? 1 : 0, Math.round(f * 10)); return `<span class="on">${"━".repeat(on)}</span><span class="off">${"─".repeat(10 - on)}</span>`; };

const STEPS = [
  {
    cmd: INSTALL, title: "Install",
    about: "dredge is one Rust binary. Cargo builds it; prebuilt releases are coming.",
    async run() {
      await o.lines([
        `    ${sp("ok b", "Updating")} git repository \`${esc(REPO)}\``,
        `   ${sp("ok b", "Compiling")} dredge-core v0.2.0`,
        `   ${sp("ok b", "Compiling")} dredge-render v0.2.0`,
        `   ${sp("ok b", "Compiling")} dredge-cli v0.2.0`,
        `    ${sp("ok b", "Finished")} \`release\` profile [optimized] target(s) in 48.20s`,
        `   ${sp("ok b", "Installed")} package \`dredge-cli v0.2.0\` (executable \`dredge\`)`,
      ], 45);
      o.html(card(INSTALL_CARD));
    },
  },
  {
    cmd: "dredge scan ~", title: "Scan once",
    about: "scan walks a folder at low priority and saves a snapshot, about 6 bytes per entry. Nothing else is written. Every question after this is instant.",
    async run() {
      const frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
      const live = o.line(`${sp("f", "⠋")} scanning ~/`);
      const n = REDUCED ? 1 : 8;
      for (let i = 1; i <= n; i++) {
        const f = i / n;
        live.innerHTML = `${sp("k", frames[i % 10])} scanning ~/ · ${(f * 6.71).toFixed(2)}M files · ${(f * 455.6).toFixed(1)} GiB · ${(f * 41.2).toFixed(1)}s`;
        await o.wait(55);
      }
      live.innerHTML = `${sp("ok", "✓")} ${sp("b", "455.6 GiB")} in ${sp("f", "~/")} · 6,712,408 files · 812,330 folders · 41.2s`;
      await o.lines([sp("f", "  162,926 files/s · snapshot 41.7 MiB"), "", next("dredge categories ~", "space by kind")]);
    },
  },
  {
    cmd: "dredge browse ~", title: "Browse",
    about: "browse lists a folder largest first: size, share, a bar, the path. The last line is what to run next, with the real path filled in.",
    async run() {
      const rows = [[148.2, "Projects/"], [96.4, "Pictures/"], [71.9, "Library/"], [52.3, "Movies/"], [31.0, "Downloads/"], [22.7, "Documents/"], [14.8, "Music/"]];
      o.line(`${sp("b", "455.6 GiB")} in ${sp("f", "~/")} · 14 items, largest first`);
      await o.wait(30);
      o.html(`<pre class="grid-out">${[
        sp("d", "      SIZE   SHARE              NAME"),
        ...rows.map(([g, n]) => `${pad(g.toFixed(1), 6)} GiB  ${pad(((g / 455.6) * 100).toFixed(1), 5)}%  ${bar10(g / 455.6)}  ${sp("f", "~/")}${esc(n)}`),
        sp("f", "+ 7 more · 18.3 GiB"),
      ].join("\n")}</pre>`);
      await o.lines([sp("f", "snapshot 2 min old"), next("dredge browse ~/Projects", "go into the largest folder")]);
    },
  },
  {
    cmd: "dredge insights ~", title: "Find what rebuilds itself",
    about: "insights points at space the owning tool re-creates. Each hint is a command printed as text. You run it, or you don't.",
    async run() {
      const hints = [
        ["18.4 GiB", "~/Library/Developer/Xcode/DerivedData/", "Xcode build products and indexes; Xcode rebuilds them. Quit Xcode first.", "/Users/you/Library/Developer/Xcode/DerivedData", true],
        ["12.6 GiB", "~/Library/Caches/", "Per-user app caches; apps re-create them. Quit apps first; some will download content again.", "/Users/you/Library/Caches", true],
        ["3.9 GiB", "~/Projects/web-app/node_modules/", "Installed packages for the project beside it; the package manager reinstalls them from the lockfile.", "/Users/you/Projects/web-app/node_modules", false],
      ];
      o.line(`${sp("b", "48.3 GiB")} could be rebuilt or cleared, in 27 hints under ${sp("f", "~/")}`);
      for (const [s, p, msg, path, contents] of hints) {
        await o.wait(40);
        o.html(`<p class="line">${sp("b", s)}  ${sp("h", "◆")} ${esc(p)}</p><p class="line msg">${esc(msg)}</p>${card(hintCard(path, contents))}`, "hint-item");
      }
      await o.lines([sp("f", "+ 24 more · 13.4 GiB"), "", sp("f", "snapshot 3 min old · dredge never runs these. Read each one before you run it.")]);
    },
  },
  {
    cmd: "dredge bar ~ --focus developer", title: "See it at a glance",
    about: "bar draws space by category in braille dots, on your terminal's own background. It is the bar at the top of this page; dredge tui is the interactive version.",
    async run() {
      const c = document.createElement("canvas"); c.className = "bar";
      o.html("").append(c);
      const f = new DotField(c, { segments: SAMPLE, pitch: 5, radius: 1.05 });
      f.setFocus(0, 0);
      await o.wait(120);
      await o.lines([
        `${sp("d", "└")} ${sp("ok b", "Developer")} 152.8 GiB · 34% of ~/`,
        SAMPLE.map((s) => `<span class="dot ink-${s.key}">●</span> ${esc(LABEL[s.key])}`).join("  "),
        "", next("dredge tui ~", "the same bar, interactive"),
      ]);
    },
  },
  {
    cmd: MCP_ADD, title: "Hand it to your agent",
    about: "dredge mcp is a read-only MCP server. Agents get the same small, sorted answers, and the same promise: suggestions only.",
    async run() {
      await o.lines([
        "Added stdio MCP server dredge with command: dredge mcp to user config",
        "", sp("f", "Ask your agent: “what is using space in my home folder?”"),
        `${sp("f", "It calls")} disk_volumes ${sp("f", "→")} disk_browse ${sp("f", "→")} disk_insights${sp("f", ", and shows you each command instead of running it.")}`,
        "", `${sp("ok", "✓")} tutorial done · ${runLink("demo")} ${sp("f", "reads a folder of yours, right here")} · ${runLink("mcp")} ${sp("f", "for other clients")}`,
      ], 30);
    },
  },
];

let step = -1;
const done = new Set();
function setStep(i) {
  step = i;
  progress.hidden = i < 0;
  progress.innerHTML = `<span>tutorial ${Math.min(i + 1, STEPS.length)}/${STEPS.length}</span>` +
    STEPS.map((_, k) => `<i class="${done.has(k) ? "on" : k === i ? "cur" : ""}"></i>`).join("");
  chips();
}

// ---- commands ------------------------------------------------------------------------------

const MCP_TOOLS = [
  ["disk_volumes", "free space, snapshot age"], ["disk_scan", "read a folder into a snapshot"],
  ["disk_browse", "children, largest first"], ["disk_largest", "biggest files"], ["disk_types", "space by extension"],
  ["disk_categories", "space by kind"], ["disk_insights", "what could be cleared, as suggestions"],
  ["disk_detail", "one path"], ["disk_doctor", "read-only health checks"],
];
const kv = (rows, link = false) => `<dl class="kv">${rows.map(([k, v]) => `<dt>${link ? runLink(k) : esc(k)}</dt><dd>${esc(v)}</dd>`).join("")}</dl>`;

const HELP = [
  ["tutorial", "six steps from install to MCP"], ["next", "the next tutorial step"],
  ["demo", "read a folder of yours in this tab"], ["install", "install commands"],
  ["mcp", "add dredge to an MCP client"], ["tools", "the MCP tools agents get"],
  ["rules", "what dredge will and will never do"], ["theme", "pick a terminal theme"], ["download", "prebuilt binaries"],
  ["source", "the code on GitHub"], ["history", "commands you ran"], ["clear", "clear the screen (ctrl-l)"],
];

const COMMANDS = {
  help() { o.html(kv(HELP, true)); o.line(sp("f", "Tab completes, ↑ recalls, and any tutorial command works on its own, such as dredge browse ~.")); },
  async tutorial() { done.clear(); setStep(0); await o.lines([`${sp("ok", "✓")} six steps · press ↵ to run each one, or type it yourself`]); },
  next() { return exec(STEPS[Math.max(0, step)].cmd); },
  install() {
    o.html(card(INSTALL_CARD));
    o.line(next("dredge scan ~", "then read your home folder once"));
  },
  download() {
    o.line(`${sp("w", "!")} no prebuilt release yet · releases will appear at <a href="${REPO}/releases" rel="noopener">github.com/CinematicGenius007/dredge/releases</a>`);
    o.line(next("install", "build it with Cargo for now"));
  },
  mcp() {
    o.line(sp("f", "dredge mcp is a read-only stdio MCP server. Pick your client:"));
    o.html(card(MCP_CARD));
    o.line(next("tools", "what agents can ask"));
  },
  tools() { o.html(kv(MCP_TOOLS)); o.line(sp("f", "Small answers, sorted by size, from the same snapshot the CLI reads.")); },
  rules() {
    o.lines([
      `${sp("ok", "✓")} ${sp("b", "read-only")}       ${sp("f", "writes, and on request removes, only its own snapshot files")}`,
      `${sp("ok", "✓")} ${sp("b", "suggests only")}   ${sp("f", "cleanup commands are printed as text; never run")}`,
      `${sp("ok", "✓")} ${sp("b", "local only")}      ${sp("f", "no network, no telemetry, no symlinks followed, no cloud downloads")}`,
      `${sp("ok", "✓")} ${sp("b", "says its age")}    ${sp("f", "every answer names how old its snapshot is")}`,
      `${sp("ok", "✓")} ${sp("b", "pipes get JSON")}  ${sp("f", "terminals get this view; scripts get one stable document")}`,
    ], 15);
  },
  source() { open(REPO, "_blank", "noopener"); o.line(`${sp("f", "opened")} <a href="${REPO}" rel="noopener">${esc(REPO)}</a>`); },
  history() { o.html(`<pre class="grid-out">${hist.map((h, i) => `${sp("d", pad(i + 1, 4))}  ${esc(h)}`).join("\n") || sp("f", "nothing yet")}</pre>`); },
  clear() { log.innerHTML = ""; o.run = null; out.querySelector(".boot")?.remove(); },
  whoami() { o.line(`visitor ${sp("f", "· no analytics or cookies here; the one thing this page keeps is your theme, in this browser")}`); },
  theme(args) { return theme.command(args[0]); },
  ls() { o.line(["tutorial/", "demo/", "mcp/", "install", "rules", "llms.txt"].map((n) => n.endsWith("/") ? sp("k b", n) : n === "llms.txt" ? `<a href="/llms.txt">llms.txt</a>` : n).join("   ")); },
  demo() { return demo.prompt(); },
};
const ALIAS = { "?": "help", man: "help", start: "tutorial", try: "demo", github: "source", repo: "source", cls: "clear", agents: "mcp", releases: "download" };
const WINDOWS = ["tutorial", "demo", "mcp", "install"];

// ---- exec ----------------------------------------------------------------------------------

const hist = [];
let hi = 0, busy = false;

async function type(text) {
  input.value = "";
  if (REDUCED) { input.value = text; return; }
  // A quick type-in, about a fifth of a second however long the command is.
  const step = Math.max(1, Math.ceil(text.length / 12));
  for (let i = step; i < text.length + step; i += step) { input.value = text.slice(0, i); await o.wait(14); }
  await o.wait(40);
}

async function exec(raw, { typed = false } = {}) {
  if (busy) return;
  const cmd = raw.trim().replace(/\s+/g, " ");
  busy = true; form.classList.add("busy");
  try {
    if (!typed) await type(cmd);
    input.value = "";
    if (!cmd) return;
    hist.push(cmd); hi = hist.length;
    o.screen(`<span class="u">visitor@dredge</span> <span class="p">~</span> $ ${esc(cmd)}`);
    await dispatch(cmd);
  } finally {
    busy = false; form.classList.remove("busy");
    chips(); o.scroll();
  }
}

async function dispatch(cmd) {
  const k = STEPS.findIndex((s) => s.cmd.toLowerCase() === cmd.toLowerCase());
  if (k >= 0) {
    if (step < 0) setStep(k);
    comment(`${k + 1}/${STEPS.length} · ${STEPS[k].title} · ${STEPS[k].about}`);
    await STEPS[k].run();
    done.add(k);
    setStep(k + 1 < STEPS.length ? k + 1 : k);
    if (done.size === STEPS.length) setStep(-1);
    return;
  }
  const [head0, ...rest] = cmd.split(" ");
  const head = ALIAS[head0.toLowerCase()] || head0.toLowerCase();
  if (WINDOWS.includes(head)) setWindow(head);
  if (COMMANDS[head]) return COMMANDS[head](rest);
  if (head === "sudo") return o.line(`${sp("e", "✗")} dredge needs no root, and this page has none to give`);
  if (head === "rm" || head === "find") return o.line(`${sp("e", "✗")} not here. dredge prints commands like this; it never runs them, and neither does this page`);
  if (head === "echo") return o.line(esc(rest.join(" ")));
  if (head === "cd") return o.line(sp("f", "this shell stays in ~ · try dredge browse ~"));
  if (head === "exit") return o.line(sp("f", "there is no leaving · close the tab, or run demo"));
  if (head === "dredge") {
    const sub = rest[0];
    if (sub === "mcp") return COMMANDS.mcp();
    if (sub === "--help" || sub === "help" || !sub) return COMMANDS.help();
    return o.lines([`${sp("w", "!")} this page knows the tutorial commands; your terminal knows the rest`, `  ${sp("f", "try")} ${runLink(STEPS[Math.max(0, step)].cmd)}`]);
  }
  return o.lines([`${sp("e", "✗")} unknown command: ${esc(head0)}`, `  ${sp("f", "type")} ${runLink("help")} ${sp("f", "for the list")}`]);
}

function setWindow(name) {
  document.querySelectorAll(".wins a").forEach((a) => a.classList.toggle("on", a.dataset.run === name));
  if (location.hash !== `#${name}`) window.history.replaceState(null, "", `#${name}`);
}

function chips() {
  const list = [];
  if (step >= 0 && step < STEPS.length && done.size < STEPS.length) {
    list.push({ cmd: STEPS[step].cmd, primary: true });
    list.push({ cmd: "demo" }, { cmd: "mcp" }, { cmd: "help" });
  } else {
    list.push({ cmd: "tutorial", primary: true }, { cmd: "demo" }, { cmd: "mcp" }, { cmd: "install" }, { cmd: "help" });
  }
  chipsEl.innerHTML = list.map((c) => `<button type="button" class="${c.primary ? "primary" : ""}" data-run="${esc(c.cmd)}">${c.primary ? "<kbd>↵</kbd>" : ""}${esc(c.cmd)}</button>`).join("");
  input.placeholder = list[0].primary ? `press ↵ to run ${list[0].cmd.length > 34 ? list[0].cmd.slice(0, 32) + "…" : list[0].cmd}` : "";
}

// ---- input ---------------------------------------------------------------------------------

form.addEventListener("submit", (e) => {
  e.preventDefault();
  const v = input.value.trim();
  if (v) return exec(v, { typed: true });
  const first = chipsEl.querySelector(".primary");
  if (first) exec(first.dataset.run);
});
document.addEventListener("click", (e) => {
  const r = e.target.closest("[data-run]");
  if (!r) return;
  e.preventDefault();
  exec(r.dataset.run);
  if (matchMedia("(hover: hover)").matches) input.focus({ preventScroll: true });
});

const WORDS = [...new Set([...HELP.map(([c]) => c), ...STEPS.map((s) => s.cmd), "whoami", "ls", ...THEMES.map(([id]) => `theme ${id}`)])];
input.addEventListener("keydown", (e) => {
  if (e.key === "Tab" && !e.shiftKey) {
    const v = input.value.trim().toLowerCase();
    const m = v ? WORDS.filter((w) => w.toLowerCase().startsWith(v) && w.toLowerCase() !== v) : [chipsEl.querySelector(".primary")?.dataset.run];
    if (m[0]) { e.preventDefault(); input.value = m[0]; if (m.length > 1) o.line(m.map((w) => runLink(w)).join("   ")); }
  } else if (e.key === "ArrowUp" && hist.length) {
    e.preventDefault(); hi = Math.max(0, hi - 1); input.value = hist[hi];
  } else if (e.key === "ArrowDown" && hist.length) {
    e.preventDefault(); hi = Math.min(hist.length, hi + 1); input.value = hist[hi] ?? "";
  } else if (e.key === "l" && e.ctrlKey) { e.preventDefault(); COMMANDS.clear(); }
  else if (e.key === "Escape") input.value = "";
});
// Typing anywhere goes to the prompt, as in a terminal.
document.addEventListener("keydown", (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey || e.target === input) return;
  if (e.target.closest?.("input, textarea, .xc, .themes")) return;
  if (e.key.length === 1 && e.key !== " ") input.focus({ preventScroll: true });
});
out.addEventListener("mouseup", (e) => {
  if (e.button === 0 && !getSelection().toString() && !e.target.closest("a, button, .drop, input, pre, code, .xc")) input.focus({ preventScroll: true });
});

// ---- demo ----------------------------------------------------------------------------------

const demo = (() => {
  let mod, busyRead = false;
  const load = () => (mod ??= import("./demo.js"));

  function prompt() {
    o.line(sp("f", "Pick a folder, or drag one onto this terminal. The browser reads names and sizes here; nothing is uploaded."));
    const zone = o.html(`<div class="drop" role="button" tabindex="0" aria-label="Choose a folder"><canvas aria-hidden="true"></canvas><p><b>Drop a folder here</b><br><span class="f">or click to choose one · try ~/Downloads or a project</span></p></div>`).firstChild;
    new DotField(zone.querySelector("canvas"), { segments: [{ key: "other", bytes: 1 }], pitch: 7, radius: 1.3, levels: { none: 3 } });
    zone.addEventListener("click", () => pick.click());
    zone.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pick.click(); } });
    load();
  }

  async function read(source, name) {
    if (busyRead) return;
    busyRead = true;
    try {
      const m = await load();
      setWindow("demo");
      o.screen(`<span class="u">visitor@dredge</span> <span class="p">~</span> $ dredge scan ${esc(name)}  ${sp("d", "# in this tab")}`);
      const live = o.line(`${sp("k", "⠋")} reading…`);
      const frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
      let f = 0;
      const t0 = performance.now();
      const progressFn = (n) => { live.innerHTML = `${sp("k", frames[f++ % 10])} reading ${n.toLocaleString()} files…`; };
      const files = source.list ? await m.readFiles(source.list, progressFn) : await m.readEntries(source.entries, progressFn);
      if (!files.length) { live.innerHTML = `${sp("e", "✗")} that folder is empty`; return; }
      const r = m.analyze(files);
      live.remove();
      render(r, (performance.now() - t0) / 1000);
    } catch (err) {
      o.line(`${sp("e", "✗")} could not read that folder: ${esc(err?.message || err)}`);
    } finally { busyRead = false; chips(); }
  }

  function render(r, secs) {
    const total = r.bytes || 1;
    o.line(`${sp("ok", "✓")} ${sp("b", human(r.bytes))} in ${sp("f", r.name + "/")} · ${r.files.toLocaleString()} files · ${r.folders.toLocaleString()} folders · ${secs.toFixed(1)}s · ${sp("f", "nothing uploaded")}`);

    const blk = o.html(`<canvas class="bar" aria-hidden="true"></canvas><div class="cats"></div>`);
    const f = new DotField(blk.querySelector("canvas"), { segments: r.cats, pitch: 5, radius: 1.05 });
    const cats = blk.querySelector(".cats");
    r.cats.forEach((s, i) => {
      const b = document.createElement("button");
      b.type = "button"; b.className = `ink-${s.key}`;
      b.innerHTML = `<b>${esc(LABEL[s.key])}</b> ${esc(human(s.bytes))} <span class="d">${Math.round((s.bytes / total) * 100)}%</span>`;
      const on = () => f.setFocus(i, f.centerOf(i)), off = () => f.setFocus(null);
      b.addEventListener("mouseenter", on); b.addEventListener("focus", on);
      b.addEventListener("mouseleave", off); b.addEventListener("blur", off);
      cats.append(b);
    });
    hud.load(r.cats, r.name + "/", "your folder · hover a color");

    const shown = r.kids.slice(0, 8), more = r.kids.slice(8);
    o.line(`${sp("b", human(r.bytes))} in ${sp("f", r.name + "/")} · ${r.kids.length} items, largest first`);
    o.html(`<pre class="grid-out">${[
      sp("d", "      SIZE   SHARE              NAME"),
      ...shown.map((k) => `${pad(human(k.bytes), 10)}  ${pad(((k.bytes / total) * 100).toFixed(1), 5)}%  ${bar10(k.bytes / total)}  ${k.loose ? sp("f", k.name) : sp("f", r.name + "/") + esc(k.name)}`),
      ...(more.length ? [sp("f", `+ ${more.length} more · ${human(more.reduce((a, k) => a + k.bytes, 0))}`)] : []),
    ].join("\n")}</pre>`);

    if (r.hints.length) {
      const sum = r.hints.reduce((a, h) => a + h.bytes, 0);
      o.line(`${sp("b", human(sum))} could be rebuilt or cleared, in ${r.hints.length} hint${r.hints.length > 1 ? "s" : ""} under ${sp("f", r.name + "/")}`);
      for (const h of r.hints.slice(0, 5)) {
        o.html(`<p class="line">${sp("b", human(h.bytes))}  ${sp("h", "◆")} ${esc(h.path)}/</p><p class="line msg">${esc(h.msg)}</p>${card(hintCard(h.path, h.contents))}`, "hint-item");
      }
      if (r.hints.length > 5) o.line(sp("f", `+ ${r.hints.length - 5} more`));
      o.line(sp("f", "Paths start at the folder you picked; dredge prints full ones. It never runs these. Read each one before you run it."));
    } else {
      o.line(sp("f", "Nothing to suggest: no node_modules beside a lockfile, package caches, build output or Trash over 100 MiB here."));
    }
    o.line(sp("d", "# Sizes here are file lengths, which is what a browser can see. dredge measures space on disk, counts hard links once, and reads the whole disk."));
    o.line(next("install", "to see the whole disk"));
  }

  pick.addEventListener("change", () => {
    if (!pick.files.length) return;
    const name = (pick.files[0].webkitRelativePath || pick.files[0].name).split("/")[0];
    read({ list: [...pick.files] }, name);
    pick.value = "";
  });

  let depth = 0;
  term.addEventListener("dragenter", (e) => { if (![...e.dataTransfer.types].includes("Files")) return; e.preventDefault(); depth++; term.classList.add("over"); load(); });
  term.addEventListener("dragover", (e) => { if ([...e.dataTransfer.types].includes("Files")) e.preventDefault(); });
  term.addEventListener("dragleave", () => { if (--depth <= 0) { depth = 0; term.classList.remove("over"); } });
  term.addEventListener("drop", (e) => {
    e.preventDefault(); depth = 0; term.classList.remove("over");
    // Entries must be taken before the first await; the DataTransfer empties after the event.
    const entries = [...(e.dataTransfer.items || [])].map((i) => i.webkitGetAsEntry?.()).filter(Boolean);
    if (!entries.length) return o.line(`${sp("e", "✗")} that wasn't a folder`);
    read({ entries }, entries.length === 1 ? entries[0].name : "dropped");
  });

  return { prompt };
})();

// ---- boot ----------------------------------------------------------------------------------

document.getElementById("install").innerHTML = card(INSTALL_CARD);
chips();
const start = location.hash.slice(1);
if (WINDOWS.includes(start) || ["rules", "help", "tools", "theme"].includes(start)) setTimeout(() => exec(start), 150);
if (matchMedia("(hover: hover)").matches) input.focus({ preventScroll: true });
