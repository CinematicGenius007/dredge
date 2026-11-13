//! Golden snapshots of `render_report`. Escape bytes are shown as `\e` in the files.
//! Regenerate with `DREDGE_UPDATE_GOLDEN=1 cargo test -p dredge-render --test golden`.

use std::path::PathBuf;

use dredge_render::{render_report, Caps, ColorTier, Glyphs, Ink, Report, Segment};

const GB: u64 = 1_000_000_000;

fn report() -> Report {
    let s = |l: &str, g: u64, i| Segment::new(l, g, i);
    Report {
        title: "/data".into(),
        used_bytes: 177 * GB,
        total_bytes: Some(500 * GB),
        segments: vec![
            s("Apps", 52 * GB, Ink::Apps),
            s("System", 40 * GB, Ink::System),
            s("Developer", 31 * GB, Ink::Developer),
            s("Photos", 18 * GB, Ink::Photos),
            s("Movies", 12 * GB, Ink::Movies),
            s("Caches", 9 * GB, Ink::Caches),
            s("Documents", 6 * GB, Ink::Documents),
            s("Other", 5 * GB, Ink::Other),
            s("Music", 2 * GB, Ink::Music),
            s("Trash", 1_000_000, Ink::Trash),
            s("Free", 323 * GB, Ink::Free),
        ],
        insight: Some(
            "node_modules under /data/proj is 4.2 GiB  ->  rm -rf /data/proj/node_modules".into(),
        ),
    }
}

fn check(name: &str, width: usize, focus: Option<usize>, caps: Caps) {
    let lines = render_report(&report(), width, focus, caps);
    let again = render_report(&report(), width, focus, caps);
    assert_eq!(lines, again, "{name}: not deterministic");
    if caps.color == ColorTier::None {
        assert!(
            lines.iter().all(|l| !l.0.contains('\x1b')),
            "{name}: escape in plain output"
        );
    }
    let got: String = lines
        .iter()
        .map(|l| format!("{}\n", l.0.replace('\x1b', "\\e")))
        .collect();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/{name}.txt"));
    if std::env::var_os("DREDGE_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with DREDGE_UPDATE_GOLDEN=1"));
    assert_eq!(got, want, "{name} differs from golden");
}

#[test]
fn golden_matrix() {
    let tiers = [
        ("truecolor", ColorTier::TrueColor),
        ("ansi256", ColorTier::Ansi256),
        ("ansi16", ColorTier::Ansi16),
        ("none", ColorTier::None),
    ];
    for (tn, color) in tiers {
        for width in [40, 100] {
            for (fname, focus) in [("nofocus", None), ("focus", Some(2))] {
                let caps = Caps {
                    color,
                    glyphs: Glyphs::HalfBlock,
                };
                check(&format!("{tn}_{width}_{fname}"), width, focus, caps);
            }
        }
    }
}

#[test]
fn golden_other_glyphs() {
    for (n, g) in [("braille", Glyphs::Braille), ("ascii", Glyphs::Ascii)] {
        let caps = Caps {
            color: ColorTier::TrueColor,
            glyphs: g,
        };
        check(&format!("truecolor_40_focus_{n}"), 40, Some(2), caps);
    }
}

#[test]
fn bar_widths_are_exact_on_every_tier() {
    let r = report();
    for color in [
        ColorTier::TrueColor,
        ColorTier::Ansi256,
        ColorTier::Ansi16,
        ColorTier::None,
    ] {
        for glyphs in [Glyphs::HalfBlock, Glyphs::Braille, Glyphs::Ascii] {
            for width in [1, 7, 40, 99, 100] {
                let bar =
                    dredge_render::render_bar(&r.segments, width, Some(0), Caps { color, glyphs });
                for line in bar {
                    // Visible cells: strip `ESC[...m` sequences.
                    let mut n = 0;
                    let mut esc = false;
                    for c in line.0.chars() {
                        match (esc, c) {
                            (false, '\x1b') => esc = true,
                            (true, 'm') => esc = false,
                            (false, _) => n += 1,
                            _ => {}
                        }
                    }
                    assert_eq!(n, width, "{color:?} {glyphs:?} {width}");
                }
            }
        }
    }
}
