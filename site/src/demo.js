// The folder demo's reading and sorting. Loaded on first use. Everything stays in this tab:
// names and sizes come from the File API and nothing is sent anywhere. Categories and hints
// mirror dredge-core (crates/dredge-core/src/categories.rs), reduced to what a browser sees.

const EXT = {
  photos: "jpg jpeg png heic heif gif webp tif tiff raw dng cr2 cr3 nef arw",
  movies: "mp4 m4v mov mkv avi webm",
  music: "mp3 m4a aac flac wav ogg",
  documents: "pdf doc docx xls xlsx ppt pptx odt ods odp rtf txt md epub pages numbers",
  apps: "dmg pkg exe msi apk",
};
const BY_EXT = new Map();
for (const [cat, list] of Object.entries(EXT)) for (const e of list.split(" ")) BY_EXT.set(e, cat);

const DEV_DIRS = new Set(["node_modules", ".gradle", ".npm", "__pycache__", ".venv", "DerivedData", "CoreSimulator"]);
const LOCKFILES = ["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lock", "bun.lockb"];
const PREBUILT = ["node_modules", "Applications", "Library", "AppData", "Program Files", "Program Files (x86)",
  "extensions", ".vscode", ".vscode-insiders", ".vscode-server", ".cursor", ".cursor-server"];
export const MIN_HINT = 100 * 1024 * 1024;

const HINTS = [
  { suffix: ["node_modules"], sibling: LOCKFILES, deny: PREBUILT,
    msg: "Installed packages for the project beside it; the package manager reinstalls them from the lockfile." },
  { suffix: [".npm", "_cacache"],
    msg: "npm's download cache; npm fetches packages again when needed (same effect as `npm cache clean --force`)." },
  { suffix: [".cargo", "registry"],
    msg: "Cargo's downloaded crate index, archives and sources; Cargo downloads them again on the next build (offline builds need them)." },
  { suffix: [".gradle", "caches"],
    msg: "Gradle's dependency and build caches; Gradle downloads or rebuilds them. Stop running daemons first (`gradle --stop`)." },
  { suffix: ["Library", "Developer", "Xcode", "DerivedData"], contents: true,
    msg: "Xcode build products and indexes; Xcode rebuilds them. Quit Xcode first." },
  { suffix: ["Library", "Caches"], contents: true,
    msg: "Per-user app caches; apps re-create them. Quit apps first; some will download content again." },
  { suffix: [".cache"], contents: true,
    msg: "Per-user non-essential cached data; programs re-create it. Close programs first." },
  { suffix: [".Trash"], contents: true, msg: "Items in the Trash. Emptying it deletes them permanently." },
];

class Dir {
  constructor(name, parent) { this.name = name; this.parent = parent; this.dirs = new Map(); this.files = new Map(); this.bytes = 0; }
  child(name) { let d = this.dirs.get(name); if (!d) this.dirs.set(name, (d = new Dir(name, this))); return d; }
  path() { const p = []; for (let d = this; d; d = d.parent) p.unshift(d.name); return p.join("/"); }
}

const tick = () => new Promise((r) => setTimeout(r));

/** Files from <input webkitdirectory>: [[path parts], size][] */
export async function readFiles(list, progress) {
  const out = [];
  for (const f of list) {
    out.push([(f.webkitRelativePath || f.name).split("/"), f.size]);
    if (out.length % 2000 === 0) { progress(out.length); await tick(); }
  }
  return out;
}

/** Entries from a drop. Call `entriesOf` synchronously in the drop handler, then this. */
export function entriesOf(dt) { return [...(dt.items || [])].map((i) => i.webkitGetAsEntry?.()).filter(Boolean); }
export async function readEntries(entries, progress) {
  const out = [];
  const wrap = entries.length === 1 && entries[0].isDirectory ? null : "dropped";
  const queue = entries.map((e) => [e, wrap ? [wrap, e.name] : [e.name]]);
  while (queue.length) {
    await Promise.all(queue.splice(0, 64).map(async ([e, parts]) => {
      if (e.isFile) out.push([parts, await new Promise((res) => e.file((f) => res(f.size), () => res(0)))]);
      else if (e.isDirectory) {
        const reader = e.createReader();
        for (;;) {
          const chunk = await new Promise((res) => reader.readEntries(res, () => res([])));
          if (!chunk.length) break;
          for (const k of chunk) queue.push([k, [...parts, k.name]]);
        }
      }
    }));
    progress(out.length);
  }
  return out;
}

export function analyze(files) {
  const root = new Dir(files[0][0][0], null);
  let folders = 1;
  for (const [parts, size] of files) {
    let d = root;
    for (let i = 1; i < parts.length - 1; i++) { if (!d.dirs.has(parts[i])) folders++; d = d.child(parts[i]); }
    const leaf = parts[parts.length - 1];
    if (leaf) d.files.set(leaf, (d.files.get(leaf) || 0) + size);
    for (let x = d; x; x = x.parent) x.bytes += size;
  }

  // Categories: a directory rule claims its whole subtree; then the file extension decides.
  const cats = {};
  const add = (k, b) => { if (b) cats[k] = (cats[k] || 0) + b; };
  (function walk(d, claimed) {
    for (const [name, sub] of d.dirs) walk(sub, claimed || dirRule(name, d));
    for (const [f, b] of d.files) add(claimed || categorize(f), b);
  })(root, dirRule(root.name, null));

  // Hints: the first matching rule wins; nothing inside a match gets a second hint.
  const hints = [];
  (function walk(d) {
    for (const sub of d.dirs.values()) {
      const h = HINTS.find((r) => matches(r, sub));
      if (h) { if (sub.bytes >= MIN_HINT) hints.push({ path: sub.path(), bytes: sub.bytes, msg: h.msg, contents: !!h.contents }); continue; }
      walk(sub);
    }
  })(root);
  hints.sort((a, b) => b.bytes - a.bytes);

  let own = 0, nOwn = 0;
  for (const b of root.files.values()) { own += b; nOwn++; }
  const kids = [...root.dirs.values()].map((d) => ({ name: d.name + "/", bytes: d.bytes }));
  if (own) kids.push({ name: `(${nOwn} file${nOwn > 1 ? "s" : ""} directly inside)`, bytes: own, loose: true });
  kids.sort((a, b) => b.bytes - a.bytes);

  return {
    name: root.name, bytes: root.bytes, files: files.length, folders, kids, hints,
    cats: Object.entries(cats).sort((a, b) => b[1] - a[1]).map(([key, bytes]) => ({ key, bytes })),
  };
}

function categorize(name) {
  const dot = name.lastIndexOf(".");
  return (dot > 0 && BY_EXT.get(name.slice(dot + 1).toLowerCase())) || "other";
}
function dirRule(name, parent) {
  if (DEV_DIRS.has(name)) return "developer";
  if (name === "target" && parent?.files.has("Cargo.toml")) return "developer";
  if (name === "registry" && parent?.name === ".cargo") return "developer";
  if (name === "Caches" && parent?.name === "Library") return "caches";
  if (name === ".cache") return "caches";
  if (name === ".Trash") return "trash";
  if (name.endsWith(".app")) return "apps";
  return null;
}
function matches(rule, d) {
  let x = d;
  for (let i = rule.suffix.length - 1; i >= 0; i--, x = x?.parent) if (!x || x.name !== rule.suffix[i]) return false;
  if (rule.sibling && !rule.sibling.some((f) => d.parent?.files.has(f))) return false;
  if (rule.deny) for (let a = d.parent; a; a = a.parent) if (rule.deny.includes(a.name) || a.name.endsWith(".app")) return false;
  return true;
}
