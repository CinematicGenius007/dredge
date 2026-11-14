//! Every command's human view follows the shared skeleton (docs/UX.md): it leads with the
//! answer, fits the terminal, ends with a Next line, and emits no escapes without color.
//! The outputs come from a real engine over a fixture snapshot, not hand-built records.

use std::path::Path;

use dredge_core::{
    Engine, EngineConfig, Error, Forget, Index, IndexBuilder, MetaInit, Output, Query, RawEntry,
    ScanOptions, ScanProgress, Scanner,
};
use dredge_render::human::{render_human, Style};
use dredge_render::{Caps, ColorTier, Glyphs};
use unicode_width::UnicodeWidthStr;

struct Fixture;

impl Scanner for Fixture {
    fn name(&self) -> &'static str {
        "fixture"
    }

    fn scan(&self, root: &Path, _: &ScanOptions, _: &ScanProgress) -> Result<Index, Error> {
        let mut b = IndexBuilder::new(&root.to_string_lossy());
        let app = b.add(0, RawEntry::dir("app"));
        b.add(app, RawEntry::file("package-lock.json", 4096));
        let nm = b.add(app, RawEntry::dir("node_modules"));
        b.add(nm, RawEntry::file("bundle.js", 300 << 20));
        let pics = b.add(0, RawEntry::dir("Pictures"));
        b.add(pics, RawEntry::file("trip.jpg", 50 << 20));
        b.add(0, RawEntry::file("notes.txt", 1000));
        b.finish(MetaInit {
            scanner: "fixture".into(),
            volume_id: None,
            case_insensitive: false,
            elapsed_ms: 12,
        })
    }
}

fn outputs(root: &str, data: &Path) -> Vec<(&'static str, Output)> {
    let e = Engine::new(EngineConfig {
        data_dir: data.to_path_buf(),
        focus_roots: vec![],
        verbose: false,
        max_data: None,
    });
    let scan = e
        .scan(
            &Fixture,
            root,
            &ScanOptions::default(),
            &ScanProgress::default(),
        )
        .unwrap();
    let p = || root.to_string();
    let app = format!("{root}{}app", std::path::MAIN_SEPARATOR);
    vec![
        ("scan", Output::Scan(scan)),
        ("volumes", e.run(&Query::Volumes)),
        ("snapshots", e.run(&Query::Snapshots)),
        ("doctor", e.run(&Query::Doctor)),
        (
            "browse",
            e.run(&Query::Browse {
                path: p(),
                limit: None,
            }),
        ),
        (
            "largest",
            e.run(&Query::Largest {
                path: p(),
                ext: None,
                min_bytes: None,
                limit: None,
            }),
        ),
        (
            "types",
            e.run(&Query::Types {
                path: p(),
                limit: None,
            }),
        ),
        (
            "categories",
            e.run(&Query::Categories {
                path: p(),
                limit: None,
            }),
        ),
        (
            "insights",
            e.run(&Query::Insights {
                path: p(),
                limit: None,
            }),
        ),
        ("detail", e.run(&Query::Detail { path: app })),
        (
            "forget",
            Output::Forget(e.forget(&Forget::Prune, true).unwrap()),
        ),
    ]
}

fn style(width: usize, caps: Caps) -> Style {
    Style {
        caps,
        width,
        home: None,
    }
}

/// A Next line or a `$ command` line: shown whole even past the width.
fn is_command(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("Next ") || t.starts_with("dredge ") || t.starts_with("$ ")
}

fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for d in chars.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn every_command_follows_the_skeleton() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root_s = root.path().to_string_lossy().to_string();
    for (name, out) in outputs(&root_s, data.path()) {
        assert!(!out.is_error(), "{name}: {}", out.render(true));
        for width in [100, 72, 50] {
            let lines = render_human(&out, &style(width, Caps::PLAIN));
            let text: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
            let all = text.join("\n");
            assert!(
                text.first().is_some_and(|l| !l.trim().is_empty()),
                "{name}@{width}: the first line is the answer:\n{all}"
            );
            assert!(
                !all.contains('\x1b'),
                "{name}@{width}: escapes without color"
            );
            // Commands are never cut (a cut command cannot be pasted); everything else fits.
            for l in text.iter().filter(|l| !is_command(l)) {
                assert!(
                    UnicodeWidthStr::width(*l) <= width,
                    "{name}@{width}: line wider than the terminal: {l:?}\n{all}"
                );
            }
            assert!(
                text.iter().any(|l| l.starts_with("Next")),
                "{name}@{width}: no Next line:\n{all}"
            );
        }
        // With color the same text comes out, only styled.
        let color = Caps {
            color: ColorTier::TrueColor,
            glyphs: Glyphs::HalfBlock,
        };
        for l in render_human(&out, &style(100, color))
            .into_iter()
            .filter(|l| !is_command(&strip(l.as_str())))
        {
            assert!(
                UnicodeWidthStr::width(strip(l.as_str()).as_str()) <= 100,
                "{name}: colored line too wide: {:?}",
                l.as_str()
            );
        }
    }
}

#[test]
fn commands_lead_with_their_answer() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root_s = root.path().to_string_lossy().to_string();
    let render = |out: &Output| -> String {
        render_human(out, &style(100, Caps::PLAIN))
            .iter()
            .map(|l| l.as_str().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (name, out) in outputs(&root_s, data.path()) {
        let t = render(&out);
        let want: &[&str] = match name {
            "scan" => &["350", "files"],
            "browse" => &["app/", "Pictures/", "notes.txt"],
            "largest" => &["bundle.js", "trip.jpg"],
            "types" => &[".js", ".jpg"],
            "categories" => &["Developer", "Photos"],
            "insights" => &["node_modules/", "$ ", "dredge never runs"],
            "detail" => &["app/"],
            "snapshots" => &["1 snapshot"],
            _ => &[],
        };
        for w in want {
            assert!(t.contains(w), "{name}: missing {w:?} in:\n{t}");
        }
    }
}

#[test]
fn errors_say_what_failed_and_how_to_fix_it() {
    let data = tempfile::tempdir().unwrap();
    let e = Engine::new(EngineConfig {
        data_dir: data.path().to_path_buf(),
        focus_roots: vec![],
        verbose: false,
        max_data: None,
    });
    let out = e.run(&Query::Browse {
        path: data.path().join("never-scanned").to_string_lossy().into(),
        limit: None,
    });
    assert!(out.is_error());
    let lines = render_human(&out, &style(80, Caps::PLAIN));
    assert!(
        lines[0].as_str().starts_with("\u{2717} "),
        "{:?}",
        lines[0].as_str()
    );
    assert!(lines.len() >= 2, "a fix line follows: {lines:?}");
}
