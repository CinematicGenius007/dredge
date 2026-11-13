//! End-to-end tests: run the real binary against fixture trees in temp dirs. Nothing outside
//! the temp dirs is read or written.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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
        // A small JavaScript project and a photo folder, for categories.
        fs::create_dir_all(tree.join("proj/node_modules/pkg")).unwrap();
        fs::write(tree.join("proj/package-lock.json"), b"{}").unwrap();
        fs::write(
            tree.join("proj/node_modules/pkg/index.js"),
            vec![3u8; 16384],
        )
        .unwrap();
        fs::create_dir_all(tree.join("pics")).unwrap();
        fs::write(tree.join("pics/cat.jpg"), vec![4u8; 12288]).unwrap();
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
    // big.bin, big.link, small.txt, empty, ünï.log, file.dat, package-lock.json, index.js, cat.jpg
    assert_eq!(s["summary"]["files"], 9);
    assert_eq!(s["schema_version"], 3);

    let v = f.json(&["browse", &f.root()]);
    assert_eq!(v["kind"], "browse");
    assert_eq!(v["records"][0]["name"], "a");
    assert_eq!(v["summary"]["entries"], 4);
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
    for (command, query) in [
        (
            "categories",
            Query::Categories {
                path: f.root(),
                limit: Some(1),
            },
        ),
        (
            "insights",
            Query::Insights {
                path: f.root(),
                limit: Some(1),
            },
        ),
    ] {
        let (cli, ok) = f.run(&[command, &f.root(), "--limit", "1"]);
        assert!(ok);
        assert_eq!(cli.trim_end(), engine.run(&query).render(false));
    }
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

#[test]
fn completions_exist_for_each_supported_shell() {
    let f = Fixture::new();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let (text, ok) = f.run(&["completions", shell]);
        assert!(ok, "{shell}: {text}");
        assert!(
            text.contains("dredge") && !text.trim().is_empty(),
            "{shell}: {text}"
        );
    }
}

#[test]
fn doctor_reports_empty_store_corrupt_snapshot_and_missing_focus_root() {
    let f = Fixture::new();
    fs::create_dir_all(&f.data).unwrap();
    fs::write(f.data.join("broken.dredge"), b"not a snapshot").unwrap();
    let missing = f._tmp.path().join("missing-focus");
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["doctor"])
        .env("DREDGE_DATA_DIR", &f.data)
        .env("DREDGE_FOCUS_ROOTS", &missing)
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["kind"], "doctor");
    assert_eq!(v["schema_version"], 3);
    let rows = v["records"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| r["check"] == "snapshot_count"
            && r["message"].as_str().unwrap().contains("1 snapshot")));
    assert!(rows.iter().any(
        |r| r["check"].as_str().unwrap().starts_with("snapshot_header:") && r["status"] == "warn"
    ));
    assert!(rows
        .iter()
        .any(|r| r["check"].as_str().unwrap().starts_with("focus_root:") && r["status"] == "warn"));
}

#[test]
fn doctor_reports_the_files_own_snapshot_version() {
    let f = Fixture::new();
    fs::create_dir_all(&f.data).unwrap();
    let mut bytes = b"DREDGE\0\x01".to_vec();
    bytes.extend_from_slice(&99u32.to_le_bytes());
    fs::write(f.data.join("future.dredge"), bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["doctor"])
        .env("DREDGE_DATA_DIR", &f.data)
        .env_remove("DREDGE_FOCUS_ROOTS")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["records"].as_array().unwrap();
    let header = rows
        .iter()
        .find(|r| r["check"] == "snapshot_header:future.dredge")
        .unwrap();
    assert_eq!(header["status"], "warn");
    assert!(header["message"].as_str().unwrap().contains("version 99"));
    let fmt = rows
        .iter()
        .find(|r| r["check"] == "snapshot_format")
        .unwrap();
    assert_eq!(fmt["status"], "warn");
}

#[test]
fn doctor_on_empty_data_directory_reports_zero_snapshots_without_creating_it() {
    let f = Fixture::new();
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["doctor"])
        .env("DREDGE_DATA_DIR", &f.data)
        .env_remove("DREDGE_FOCUS_ROOTS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["kind"], "doctor");
    assert!(v["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["check"] == "snapshot_count" && r["message"] == "0 snapshot file(s) found"));
    assert!(!f.data.exists());
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

#[test]
fn categories_reconcile_and_insights_are_text_only() {
    let f = Fixture::new();
    f.scan();
    let root = f.json(&["detail", &f.root()]);
    let total = root["records"][0]["allocated_bytes"].as_u64().unwrap();

    let c = f.json(&["categories", &f.root(), "--limit", "100"]);
    assert_eq!(c["kind"], "categories");
    assert_eq!(c["schema_version"], 3);
    assert_eq!(c["summary"]["allocated_bytes"], total);
    let rows = c["records"].as_array().unwrap();
    let sum: u64 = rows
        .iter()
        .map(|r| r["allocated_bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(sum, total);
    let dev = rows.iter().find(|r| r["category"] == "developer").unwrap();
    assert_eq!(dev["files"], 1);
    assert_eq!(dev["top"][0], sub(&sub(&f.root(), "proj"), "node_modules"));
    let photos = rows.iter().find(|r| r["category"] == "photos").unwrap();
    assert_eq!(photos["top"][0], sub(&f.root(), "pics"));

    // The fixture's node_modules is far below the hint threshold.
    let i = f.json(&["insights", &f.root()]);
    assert_eq!(i["kind"], "insights");
    assert_eq!(i["records"].as_array().unwrap().len(), 0);
    assert_eq!(i["summary"]["insights"], 0);

    let v = f.json(&["--verbose", "browse", &sub(&f.root(), "proj")]);
    let nm = v["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "node_modules")
        .unwrap();
    assert_eq!(nm["category"], "developer");
    let plain = f.json(&["browse", &sub(&f.root(), "proj")]);
    assert!(plain["records"][0].get("category").is_none());
}

#[test]
fn schema_lists_categories_and_insights() {
    let f = Fixture::new();
    let v = f.json(&["schema"]);
    let text = v.to_string();
    for name in [
        "CategoryRecord",
        "InsightRecord",
        "suggested_command",
        "top",
        "DoctorRecord",
        "DoctorSummary",
        "doctor",
    ] {
        assert!(text.contains(name), "schema lacks {name}");
    }
}

/// Snapshot age is wall-clock seconds and the doctor's `disk_free` message reports live free space;
/// both may change between two calls while other tests write to the disk. Mask them; every other
/// byte must match.
fn without_age(s: &str) -> String {
    let mut out = s.to_string();
    let key = "\"age_secs\":";
    if let Some(p) = out.find(key) {
        let rest = &out[p + key.len()..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        out = format!("{}{key}0{}", &out[..p], &out[p + key.len() + digits..]);
    }
    let key = "\"check\":\"disk_free\",\"status\":\"ok\",\"message\":\"";
    if let Some(p) = out.find(key) {
        let start = p + key.len();
        if let Some(end) = out[start..].find('"') {
            out = format!("{}<free>{}", &out[..start], &out[start + end..]);
        }
    }
    out
}

/// Drive `dredge mcp` over stdio with JSON-RPC and return the text of each tool result.
fn mcp_tool_texts(f: &Fixture, calls: &[(&str, Value)]) -> Vec<String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .arg("mcp")
        .env("DREDGE_DATA_DIR", &f.data)
        .env_remove("DREDGE_FOCUS_ROOTS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            // MCP stdout carries the protocol only: every line must be JSON.
            let v: Value = serde_json::from_str(&line).expect("non-JSON on MCP stdout");
            if tx.send(v).is_err() {
                break;
            }
        }
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let wait_for = |id: u64| -> Value {
        loop {
            let v = rx
                .recv_timeout(Duration::from_secs(30))
                .expect("MCP server did not answer");
            if v["id"] == id {
                return v;
            }
        }
    };
    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "dredge-test", "version": "0" }
        }
    }));
    assert!(wait_for(1)["result"].is_object());
    send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let mut texts = Vec::new();
    for (k, (name, args)) in calls.iter().enumerate() {
        let id = 2 + k as u64;
        send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }));
        let r = wait_for(id);
        let text = r["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {r}"))
            .to_string();
        texts.push(text);
    }
    drop(send);
    let _ = child.kill();
    let _ = child.wait();
    texts
}

/// CLI, MCP and the engine print the same bytes for categories and insights.
#[test]
fn mcp_categories_insights_and_doctor_are_byte_identical_to_the_cli() {
    let f = Fixture::new();
    f.scan();
    let root = f.root();
    let calls = [
        (
            "disk_categories",
            serde_json::json!({ "path": root, "limit": 3 }),
        ),
        ("disk_insights", serde_json::json!({ "path": root })),
        ("disk_doctor", serde_json::json!({})),
        (
            "disk_categories",
            serde_json::json!({ "path": sub(&root, "proj/node_modules") }),
        ),
    ];
    let mcp = mcp_tool_texts(&f, &calls);
    let cli = [
        f.run(&["categories", &root, "--limit", "3"]),
        f.run(&["insights", &root]),
        f.run(&["doctor"]),
        f.run(&["categories", &sub(&root, "proj/node_modules")]),
    ];
    let engine = Engine::new(EngineConfig {
        data_dir: f.data.clone(),
        focus_roots: vec![],
        verbose: false,
    });
    let direct = [
        engine.run(&Query::Categories {
            path: root.clone(),
            limit: Some(3),
        }),
        engine.run(&Query::Insights {
            path: root.clone(),
            limit: None,
        }),
        engine.run(&Query::Doctor),
        engine.run(&Query::Categories {
            path: sub(&root, "proj/node_modules"),
            limit: None,
        }),
    ];
    for ((m, (c, ok)), d) in mcp.iter().zip(&cli).zip(&direct) {
        assert!(ok);
        assert_eq!(without_age(m), without_age(c.trim_end()));
        assert_eq!(without_age(m), without_age(&d.render(false)));
    }
    let v: Value = serde_json::from_str(&mcp[0]).unwrap();
    assert_eq!(v["kind"], "categories");
}

#[cfg(unix)]
#[test]
fn bar_never_emits_control_bytes_from_file_names() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("evil\x1b]0;pwn\x07dir\u{202e}");
    let nm = root.join("node_modules");
    fs::create_dir_all(&nm).unwrap();
    fs::write(root.join("package-lock.json"), b"{}").unwrap();
    fs::write(nm.join("blob.bin"), vec![9u8; 101 * MIB]).unwrap();
    let data = tmp.path().join("data");
    let root_s = root.to_string_lossy().into_owned();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dredge"))
            .args(args)
            .env("DREDGE_DATA_DIR", &data)
            .env_remove("DREDGE_FOCUS_ROOTS")
            .output()
            .unwrap()
    };
    assert!(run(&["scan", &root_s]).status.success());
    let out = run(&["bar", &root_s, "--color", "never", "--width", "300"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Tip:"), "insight expected: {text}");
    assert!(text.contains("\\u{1b}"), "{text}");
    assert!(
        text.bytes().all(|b| (b >= 0x20 && b != 0x7f) || b == b'\n'),
        "control byte in {text:?}"
    );
    assert!(!text.contains('\u{202e}'));
}

#[test]
fn bar_prints_plain_text_not_json() {
    let f = Fixture::new();
    f.scan();
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["bar", &f.root(), "--color", "never", "--width", "40"])
        .env("DREDGE_DATA_DIR", &f.data)
        .env_remove("DREDGE_FOCUS_ROOTS")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains('\x1b'), "escape codes in plain output");
    assert!(serde_json::from_str::<Value>(&text).is_err());
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].chars().count() <= 40, "{text}");
    assert_eq!(lines[1].chars().count(), 40, "{text}");
    assert!(lines[1].chars().all(|c| "#=+%*@&:;!.".contains(c)));
    assert!(text.contains("Developer"), "{text}");
    assert!(text.contains("Photos"), "{text}");

    // Piped stdout in auto mode is plain too; an unknown focus fails on stderr only.
    let (auto, ok) = f.run(&["bar", &f.root(), "--width", "20", "--focus", "photos"]);
    assert!(ok && !auto.contains('\x1b'));
    let bad = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["bar", &f.root(), "--focus", "nope"])
        .env("DREDGE_DATA_DIR", &f.data)
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    let error = String::from_utf8(bad.stderr).unwrap();
    assert!(
        error.contains("valid categories:")
            && error.contains("developer")
            && error.contains("trash"),
        "{error}"
    );
    let absent = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["bar", &f.root(), "--focus", "music", "--color", "never"])
        .env("DREDGE_DATA_DIR", &f.data)
        .output()
        .unwrap();
    assert!(absent.status.success());
    assert!(String::from_utf8(absent.stderr)
        .unwrap()
        .contains("category `music` has no space"));
    let (unfocused, ok) = f.run(&["bar", &f.root(), "--color", "never"]);
    assert!(ok);
    assert_eq!(String::from_utf8(absent.stdout).unwrap(), unfocused);
}

#[test]
fn tui_refuses_without_a_terminal() {
    let f = Fixture::new();
    // The test harness pipes stdout, so there is no terminal: a message on stderr, nothing on
    // stdout, and a nonzero exit, with no scan started.
    let out = Command::new(env!("CARGO_BIN_EXE_dredge"))
        .args(["tui", &f.root()])
        .env("DREDGE_DATA_DIR", &f.data)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.starts_with("dredge: ") && err.contains("terminal"),
        "{err}"
    );
    assert!(!f.data.exists(), "tui must not scan or write on its own");
}
