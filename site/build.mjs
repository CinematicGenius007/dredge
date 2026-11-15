// Builds the static site: copies src/ to dist/, fills in the site URL, fingerprints CSS and JS
// so they can be cached forever, and writes sitemap.xml with today's date. No dependencies.
import { createHash } from "node:crypto";
import { cpSync, existsSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, extname } from "node:path";

const root = new URL(".", import.meta.url).pathname;
const src = join(root, "src"), dist = join(root, "dist");
const url = (process.env.SITE_URL ||
  "https://dredge.cinematicgenius007.com")
  .replace(/\/$/, "");
const host = url.replace(/^https?:\/\//, "");

rmSync(dist, { recursive: true, force: true });
cpSync(src, dist, { recursive: true, filter: (p) => !p.includes("/_") });

// Fingerprint assets; app.js imports ./demo.js, so hash demo first and rewrite the import.
const rename = {};
for (const name of ["demo.js", "styles.css", "app.js"]) {
  let text = readFileSync(join(dist, name), "utf8");
  for (const [a, b] of Object.entries(rename)) text = text.replaceAll(`./${a}`, `./${b}`);
  const hash = createHash("sha256").update(text).digest("hex").slice(0, 10);
  const out = name.replace(/(\.\w+)$/, `.${hash}$1`);
  writeFileSync(join(dist, out), text); rmSync(join(dist, name));
  rename[name] = out;
}

const text = new Set([".html", ".txt", ".xml", ".json", ".webmanifest", ".css", ".js", ".svg"]);
(function walk(dir) {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) { walk(p); continue; }
    if (!text.has(extname(f))) continue;
    let s = readFileSync(p, "utf8");
    const before = s;
    s = s.replaceAll("%SITE_URL%", url).replaceAll("%SITE_HOST%", host).replaceAll("%DATE%", new Date().toISOString().slice(0, 10));
    for (const [a, b] of Object.entries(rename)) s = s.replaceAll(`/${a}"`, `/${b}"`);
    if (s !== before) writeFileSync(p, s);
  }
})(dist);

for (const f of ["og.html"]) if (existsSync(join(dist, f))) rmSync(join(dist, f));
console.log(`built dist/ for ${url} · ${Object.values(rename).join(", ")}`);
