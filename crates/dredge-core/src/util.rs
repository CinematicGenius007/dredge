use std::path::MAIN_SEPARATOR;

/// Windows and macOS filesystems are case-insensitive by default; Linux is not.
pub const CASE_INSENSITIVE_DEFAULT: bool = cfg!(any(windows, target_os = "macos"));

pub fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Binary units, one decimal: `1.2 GiB`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Percentage rounded to one decimal; 0.0 when `total` is zero.
pub fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let p = part as f64 * 100.0 / total as f64;
    (p * 10.0).round() / 10.0
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn now_secs() -> i64 {
    jiff::Timestamp::now().as_second()
}

pub fn rfc3339(secs: i64) -> String {
    jiff::Timestamp::from_second(secs)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Path components split on the platform's separators, empty pieces dropped.
pub fn components(s: &str) -> Vec<&str> {
    s.split(is_sep).filter(|c| !c.is_empty()).collect()
}

pub fn name_eq(a: &str, b: &str, case_insensitive: bool) -> bool {
    a == b || (case_insensitive && a.to_lowercase() == b.to_lowercase())
}

/// True when `root`'s components are a prefix of `path`'s components.
pub fn covers(root: &str, path: &str, case_insensitive: bool) -> bool {
    let r = components(root);
    let p = components(path);
    p.len() >= r.len() && r.iter().zip(p.iter()).all(|(a, b)| name_eq(a, b, case_insensitive))
}

/// True when both paths have identical components.
pub fn same_path(a: &str, b: &str, case_insensitive: bool) -> bool {
    covers(a, b, case_insensitive) && covers(b, a, case_insensitive)
}

/// Append `name` to `base` with exactly one separator between them.
pub fn push_join(base: &mut String, name: &str) {
    if !base.ends_with(is_sep) {
        base.push(MAIN_SEPARATOR);
    }
    base.push_str(name);
}

/// `\\?\C:\x` -> `C:\x`, `\\?\UNC\srv\share` -> `\\srv\share`.
pub fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    s.to_string()
}

/// Drive or UNC prefix (Windows only) and the remainder.
fn split_prefix(s: &str) -> (String, &str) {
    if cfg!(windows) {
        let b = s.as_bytes();
        if b.len() >= 2 && is_sep(b[0] as char) && is_sep(b[1] as char) {
            let mut it = s[2..].splitn(3, |c: char| is_sep(c));
            let server = it.next().unwrap_or("");
            let share = it.next().unwrap_or("");
            let rest = it.next().unwrap_or("");
            return (format!(r"\\{server}\{share}"), rest);
        }
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            return (s[..2].to_uppercase(), &s[2..]);
        }
    }
    (String::new(), s)
}

fn is_absolute(s: &str) -> bool {
    if cfg!(windows) {
        let b = s.as_bytes();
        let unc = b.len() >= 2 && is_sep(b[0] as char) && is_sep(b[1] as char);
        let drive = b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() && (b.len() == 2 || is_sep(b[2] as char));
        unc || drive
    } else {
        s.starts_with('/')
    }
}

/// Resolve `.` and `..` lexically and normalize separators. Roots come out as `C:\` or `/`.
pub fn lexical_clean(s: &str) -> String {
    let (prefix, rest) = split_prefix(s);
    let mut stack: Vec<&str> = Vec::new();
    for c in rest.split(is_sep) {
        match c {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            c => stack.push(c),
        }
    }
    let mut out = prefix;
    if stack.is_empty() {
        out.push(MAIN_SEPARATOR);
        return out;
    }
    for c in stack {
        out.push(MAIN_SEPARATOR);
        out.push_str(c);
    }
    out
}

/// Turn a user-supplied path into the canonical form used inside snapshots. Relative paths are
/// resolved against `cwd`, so agent sessions can pass `.` from their working directory.
pub fn absolutize(input: &str, cwd: &str) -> String {
    let s = strip_verbatim(input.trim());
    let joined = if is_absolute(&s) {
        s
    } else if cfg!(windows) && s.starts_with(is_sep) {
        let (p, _) = split_prefix(cwd);
        format!("{p}{s}")
    } else {
        format!("{cwd}{MAIN_SEPARATOR}{s}")
    };
    lexical_clean(&joined)
}

pub fn current_dir_string() -> String {
    std::env::current_dir()
        .map(|p| strip_verbatim(&p.to_string_lossy()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536 * 1024 * 1024), "1.5 GiB");
    }

    #[test]
    fn pct_rounds_and_handles_zero() {
        assert_eq!(pct(1, 3), 33.3);
        assert_eq!(pct(5, 0), 0.0);
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn components_drop_empties() {
        assert_eq!(components("/a//b/"), vec!["a", "b"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths() {
        assert_eq!(lexical_clean("c:/server/logs/../code/"), r"C:\server\code");
        assert_eq!(lexical_clean("C:\\"), r"C:\");
        assert_eq!(absolutize(".", r"C:\server\logs"), r"C:\server\logs");
        assert_eq!(absolutize("..\\code", r"C:\server\logs"), r"C:\server\code");
        assert_eq!(absolutize(r"\temp", r"D:\x"), r"D:\temp");
        assert_eq!(strip_verbatim(r"\\?\C:\a"), r"C:\a");
        assert!(covers(r"C:\server", r"c:\SERVER\logs", true));
        assert!(!covers(r"C:\server", r"C:\serverx", true));
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths() {
        assert_eq!(lexical_clean("/a/b/../c/./"), "/a/c");
        assert_eq!(lexical_clean("/"), "/");
        assert_eq!(absolutize("x/y", "/home/me"), "/home/me/x/y");
        assert!(covers("/home", "/home/me", false));
        assert!(!covers("/home", "/homer", false));
    }
}
