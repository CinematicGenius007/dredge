//! End-to-end tests: run the real binary against fixture trees in temp dirs. Nothing outside
//! the temp dirs is read or written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dredge_core::{Engine, EngineConfig, Query};
use serde_json::Value;
use tempfile::TempDir;

const MIB: usize = 1024 * 1024;

struct Fixture {
    _tmp: TempDir,
    tree: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        let data = tmp.path().join("data");
        for d in ["a", "b/deep/x/y/z"] {
            fs::create_dir_all(tree.join(d)).unwrap();
        }
        fs::write(tree.join("a/big.bin"), vec![7u8; MIB]).unwrap();
        fs::write(tree.join("a/small.txt"), b"hello").unwrap();
        fs::write(tree.join("b/empty"), b"").unwrap();
        fs::write(tree.join("b/\u{fc}n\u{ef}.log"), vec![1u8; 4096]).unwrap();
        fs::write(tree.join("b/deep/x/y/z/file.dat"), vec![2u8; 8192]).unwrap();
        // A second name for the same data: must not be counted twice.
        fs::hard_link(tree.join("a/big.bin"), tree.join("a/big.link")).unwrap();
        Fixture {
            _tmp: tmp,
            tree,
            data,
        }
    }

    fn root(&self) -> String {
        self.tree.to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> (String, bool) {
        let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
            .args(args)
            .env("DREDGE_DATA_DIR", &self.data)
            .env_remove("DREDGE_FOCUS_ROOTS")
            .output()
            .unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.success())
    }

    fn json(&self, args: &[&str]) -> Value {
        let (text, _) = self.run(args);
        serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("bad JSON ({e}): {text}"))
    }

    fn scan(&self) -> Value {
        let v = self.json(&["scan", &self.root()]);
        assert_eq!(v["kind"], "scan", "{v}");
        v
    }
}

fn sub(root: &str, rest: &str) -> String {
    Path::new(root).join(rest).to_string_lossy().into_owned()
}

#[test]
fn scan_then_browse_is_sorted_with_envelope() {
    let f = Fixture::new();
    let s = f.scan();
    // big.bin, big.link, small.txt, empty, ünï.log, file.dat
    assert_eq!(s["summary"]["files"], 6);
    assert_eq!(s["schema_version"], 1);

    let v = f.json(&["browse", &f.root()]);
    assert_eq!(v["kind"], "browse");
    assert_eq!(v["records"][0]["name"], "a");
    assert_eq!(v["records"][1]["name"], "b");
    assert_eq!(v["summary"]["entries"], 2);
    assert_eq!(v["summary"]["truncated"], false);
    assert!(v["snapshot"]["age_secs"].is_u64());
}

#[test]
fn hard_links_are_counted_once() {
    let f = Fixture::new();
    f.scan();
    let a = f.json(&["detail", &sub(&f.root(), "a")]);
    let bytes = a["records"][0]["allocated_bytes"].as_u64().unwrap();
    assert!(bytes >= MIB as u64, "data must be counted: {bytes}");
    assert!(bytes < 2 * MIB as u64, "hard link counted twice: {bytes}");
}

#[test]
fn largest_and_types_see_the_deep_and_unicode_files() {
    let f = Fixture::new();
    f.scan();
    let l = f.json(&["largest", &f.root(), "--ext", "dat"]);
    assert_eq!(l["records"][0]["name"], "file.dat");
    assert_eq!(l["summary"]["matched_files"], 1);

    let t = f.json(&["types", &f.root()]);
    let exts: Vec<&str> = t["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ext"].as_str().unwrap())
        .collect();
    assert!(exts.contains(&"bin") && exts.contains(&"log") && exts.contains(&"dat"));

    let d = f.json(&["detail", &sub(&f.root(), "b/\u{fc}n\u{ef}.log")]);
    assert_eq!(d["records"][0]["kind"], "file");
}

#[test]
fn queries_without_a_snapshot_fail_with_a_stable_code() {
    let f = Fixture::new();
    let (text, ok) = f.run(&["browse", &f.root()]);
    assert!(!ok);
    let v: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["kind"], "error");
    assert_eq!(v["error"]["code"], "no_snapshot");
    assert!(v["error"]["hint"].as_str().unwrap().contains("scan"));
}

#[test]
fn missing_path_inside_a_snapshot_is_reported() {
    let f = Fixture::new();
    f.scan();
    let (text, ok) = f.run(&["detail", &sub(&f.root(), "nope")]);
    assert!(!ok);
    let v: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["error"]["code"], "path_not_in_snapshot");
}

#[test]
fn zero_limit_is_rejected() {
    let f = Fixture::new();
    f.scan();
    let (text, ok) = f.run(&["browse", &f.root(), "--limit", "0"]);
    assert!(!ok);
    let v: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["error"]["code"], "invalid_argument");
}

/// The MCP server prints `Engine::run(..).render(false)`; so does the CLI. This pins that the
/// CLI's bytes are exactly the engine's, which is what keeps the two frontends identical.
#[test]
fn cli_output_is_byte_identical_to_the_engine() {
    let f = Fixture::new();
    f.scan();
    let (cli, ok) = f.run(&["browse", &f.root(), "--limit", "1"]);
    assert!(ok);
    let engine = Engine::new(EngineConfig {
        data_dir: f.data.clone(),
        focus_roots: vec![],
        verbose: false,
    });
    let direct = engine
        .run(&Query::Browse {
            path: f.root(),
            limit: Some(1),
        })
        .render(false);
    assert_eq!(cli.trim_end(), direct);
}

#[test]
fn scan_without_a_path_uses_focus_roots() {
    let f = Fixture::new();
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .arg("scan")
        .env("DREDGE_DATA_DIR", &f.data)
        .env("DREDGE_FOCUS_ROOTS", &f.tree)
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .arg("volumes")
        .env("DREDGE_DATA_DIR", &f.data)
        .env("DREDGE_FOCUS_ROOTS", &f.tree)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["summary"]["focus"][0]["scanned"], true);
}

#[test]
fn scan_without_a_path_or_focus_roots_is_an_error() {
    let f = Fixture::new();
    let (text, ok) = f.run(&["scan"]);
    assert!(!ok);
    let v: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["error"]["code"], "invalid_argument");
}

#[test]
fn schema_command_prints_json() {
    let f = Fixture::new();
    let v = f.json(&["schema"]);
    assert!(v.to_string().contains("schema_version"));
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.tree.join("a"), f.tree.join("link")).unwrap();
    f.scan();
    let v = f.json(&["detail", &sub(&f.root(), "link")]);
    assert_eq!(v["records"][0]["kind"], "symlink");
    assert!(v["records"][0]["allocated_bytes"].as_u64().unwrap() < MIB as u64);
}

#[cfg(unix)]
#[test]
fn unreadable_directories_are_flagged_not_fatal() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let locked = f.tree.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("x"), b"x").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let s = f.scan();
    // Restore so the temp dir can be cleaned up.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    // Running as root can read anything; only assert when the denial actually happened.
    if s["summary"]["errors"]["denied"].as_u64().unwrap() > 0 {
        let v = f.json(&["detail", &sub(&f.root(), "locked")]);
        assert!(v["records"][0]["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "denied"));
    }
}
