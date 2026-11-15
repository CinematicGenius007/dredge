# dredge.sh

The dredge website is one terminal. Its header is the dithered category bar from `dredge bar` (hover a color to focus it, click to pin). Below it is a shell: `tutorial` walks from install to MCP, `demo` reads a folder in the browser, and `mcp`, `install`, `rules` and `help` print GitHub-style code blocks with copy buttons. `/#mcp`, `/#demo`, `/#install` and `/#tutorial` open the shell on that command.

Static HTML, CSS and JS. No framework, no dependencies, no analytics, no cookies.

```sh
node build.mjs                      # src/ → dist/, fingerprints CSS/JS, fills in the site URL
cd dist && python3 -m http.server   # preview
```

The site URL is `https://dredge.cinematicgenius007.com`, used for the canonical URL, Open Graph tags and sitemap. Set `SITE_URL` to override it, for example for a local preview.

## Deploy (Vercel)

Create a project with **Root Directory** `site`. `vercel.json` sets the build (`node build.mjs`, output `dist`), the security headers and CSP, and long-lived caching for fonts and hashed assets.

## For machines

| Path | What |
| :- | :- |
| `/llms.txt`, `/llms-full.txt` | the site for LLMs ([llmstxt.org](https://llmstxt.org)); `Accept: text/markdown` on `/` returns the full text |
| `/index.txt` | what `curl <host>` gets: the page as terminal text |
| `/sitemap.xml`, `/robots.txt` | crawlers are welcome, AI ones included |

## Files

- `src/index.html`: the terminal frame plus a static boot screen (title, install, command list) that works without JS and is what crawlers read
- `src/app.js`: `DotField` (a port of dredge-render's Bayer ramp), click ripples, code components (field, code block, tabs, copy), the shell and its commands
- `src/demo.js`: the folder demo's reading and sorting, loaded on first use; mirrors `crates/dredge-core/src/categories.rs`
- `og/og.html`: source for `src/og.png` (1200×630), rendered with headless Chrome (its Inter font lives in `og/fonts`, since the site itself is all JetBrains Mono):
  `chrome --headless --window-size=1200,630 --screenshot=src/og.png og/og.html`

Keep tutorial output in step with `crates/dredge-render/src/human.rs` when the CLI's wording changes.
