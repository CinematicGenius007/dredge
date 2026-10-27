//! The query functions. Both frontends reach these through `Engine`; nothing here knows about
//! CLI flags, MCP, snapshots on disk, or JSON text.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::FxHashMap;

use crate::error::Error;
use crate::model::{Index, Kind, FLAG_ALIAS, FLAG_NAMES};
use crate::output::{
    BrowseSummary, DetailSummary, EntryRecord, Filters, LargestSummary, TypeRecord, TypesSummary,
};
use crate::util::{human_bytes, pct, rfc3339};

pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 1000;
const LOGICAL_NOTABLE_BYTES: u64 = 1 << 20;

#[derive(Clone, Copy)]
enum Share {
    OfParent,
    OfScope(u64),
}

fn resolve(idx: &Index, path: &str) -> Result<u32, Error> {
    idx.resolve(path).ok_or_else(|| Error::PathNotInSnapshot {
        path: path.to_string(),
        root: idx.meta.root.clone(),
    })
}

/// Build one output row. `full` adds every extra field (verbose output and `detail`).
fn entry(idx: &Index, i: u32, share: Share, full: bool) -> EntryRecord {
    let flags = idx.flags(i);
    let kind = idx.kind(i);
    let alloc = idx.allocated(i);
    let logical = idx.logical(i);
    let (mut pct_of_parent, mut pct_of_scope) = (None, None);
    match share {
        Share::OfParent if i != 0 => pct_of_parent = Some(pct(alloc, idx.allocated(idx.parent(i)))),
        Share::OfParent => {}
        Share::OfScope(total) => pct_of_scope = Some(pct(alloc, total)),
    }
    let notable_logical = alloc.saturating_mul(2) < logical && logical >= LOGICAL_NOTABLE_BYTES;
    let is_dir = kind == Kind::Dir;
    let mtime = idx.mtime(i);
    EntryRecord {
        name: idx.name(i).to_string(),
        path: idx.path(i),
        kind: kind.as_str(),
        allocated_bytes: alloc,
        human: human_bytes(alloc),
        pct_of_parent,
        pct_of_scope,
        flags: FLAG_NAMES
            .iter()
            .filter(|(bit, _)| flags & bit != 0)
            .map(|(_, n)| *n)
            .collect(),
        hard_links: idx.hard_links(i),
        logical_bytes: (full || notable_logical).then_some(logical),
        files: (full && is_dir).then(|| u64::from(idx.files(i))),
        dirs: (full && is_dir).then(|| idx.dir_count_below(i)),
        modified: (full && mtime > 0).then(|| rfc3339(i64::from(mtime))),
    }
}

pub fn browse(
    idx: &Index,
    path: &str,
    limit: usize,
    verbose: bool,
) -> Result<(Vec<EntryRecord>, BrowseSummary), Error> {
    let node = resolve(idx, path)?;
    let total = idx.allocated(node);
    let kids: Vec<u32> = if idx.is_dir(node) {
        idx.children(node).collect()
    } else {
        vec![node]
    };
    let shown = kids.len().min(limit);
    let records = kids[..shown]
        .iter()
        .map(|&c| entry(idx, c, Share::OfParent, verbose))
        .collect();
    let remaining_bytes = kids[shown..].iter().map(|&c| idx.allocated(c)).sum::<u64>();
    Ok((
        records,
        BrowseSummary {
            path: idx.path(node),
            allocated_bytes: total,
            human: human_bytes(total),
            entries: kids.len(),
            shown,
            truncated: shown < kids.len(),
            remaining_entries: kids.len() - shown,
            remaining_bytes,
        },
    ))
}

/// Extension without the dot; `None` for no extension or a leading-dot name like `.gitignore`.
fn file_ext(name: &str) -> Option<&str> {
    match name.rfind('.') {
        Some(p) if p > 0 && p + 1 < name.len() => Some(&name[p + 1..]),
        _ => None,
    }
}

pub fn largest(
    idx: &Index,
    path: &str,
    ext: Option<&str>,
    min_bytes: Option<u64>,
    limit: usize,
    verbose: bool,
) -> Result<(Vec<EntryRecord>, LargestSummary), Error> {
    let node = resolve(idx, path)?;
    let total = idx.allocated(node);
    let want_ext = ext
        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty());
    let min = min_bytes.unwrap_or(0);

    // Min-heap of the best `limit` candidates: size descending, then pre-order position ascending.
    let mut heap: BinaryHeap<Reverse<(u64, Reverse<u32>)>> = BinaryHeap::new();
    let (mut matched_files, mut matched_bytes) = (0u64, 0u64);
    for i in node..idx.subtree_end(node) {
        if idx.kind(i) != Kind::File {
            continue;
        }
        let alloc = idx.allocated(i);
        // Zero-size files (and hard-link aliases) use no space, so they are not "largest" material.
        if alloc == 0 || alloc < min {
            continue;
        }
        if let Some(w) = &want_ext {
            match file_ext(idx.name(i)) {
                Some(e) if e.to_lowercase() == *w => {}
                _ => continue,
            }
        }
        matched_files += 1;
        matched_bytes += alloc;
        heap.push(Reverse((alloc, Reverse(i))));
        if heap.len() > limit {
            heap.pop();
        }
    }
    let records: Vec<EntryRecord> = heap
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse((_, Reverse(i)))| entry(idx, i, Share::OfScope(total), verbose))
        .collect();
    let shown = records.len();
    Ok((
        records,
        LargestSummary {
            path: idx.path(node),
            filters: Filters {
                ext: want_ext,
                min_bytes,
            },
            matched_files,
            matched_bytes,
            shown,
            truncated: (shown as u64) < matched_files,
        },
    ))
}

pub fn types(idx: &Index, path: &str, limit: usize) -> Result<(Vec<TypeRecord>, TypesSummary), Error> {
    let node = resolve(idx, path)?;
    let total = idx.allocated(node);
    let mut map: FxHashMap<Box<str>, (u64, u64)> = FxHashMap::default();
    for i in node..idx.subtree_end(node) {
        if idx.kind(i) != Kind::File || idx.flags(i) & FLAG_ALIAS != 0 {
            continue;
        }
        let ext = file_ext(idx.name(i)).unwrap_or("(none)");
        let key: Cow<str> = if ext.chars().any(char::is_uppercase) {
            Cow::Owned(ext.to_lowercase())
        } else {
            Cow::Borrowed(ext)
        };
        let alloc = idx.allocated(i);
        if let Some(e) = map.get_mut(&*key) {
            e.0 += 1;
            e.1 += alloc;
        } else {
            map.insert(key.into_owned().into_boxed_str(), (1, alloc));
        }
    }
    let mut rows: Vec<(Box<str>, (u64, u64))> = map.into_iter().collect();
    rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then(b.1 .0.cmp(&a.1 .0)).then(a.0.cmp(&b.0)));
    let shown = rows.len().min(limit);
    let remaining_bytes = rows[shown..].iter().map(|r| r.1 .1).sum::<u64>();
    let types = rows.len();
    let records = rows
        .into_iter()
        .take(shown)
        .map(|(ext, (files, bytes))| TypeRecord {
            ext: ext.into_string(),
            files,
            allocated_bytes: bytes,
            human: human_bytes(bytes),
            pct_of_scope: pct(bytes, total),
        })
        .collect();
    Ok((
        records,
        TypesSummary {
            path: idx.path(node),
            types,
            shown,
            truncated: shown < types,
            remaining_bytes,
        },
    ))
}

pub fn detail(idx: &Index, path: &str) -> Result<(Vec<EntryRecord>, DetailSummary), Error> {
    let node = resolve(idx, path)?;
    Ok((
        vec![entry(idx, node, Share::OfParent, true)],
        DetailSummary {
            path: idx.path(node),
            parent: (node != 0).then(|| idx.path(idx.parent(node))),
            depth: idx.depth(node),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{IndexBuilder, MetaInit};
    use crate::model::RawEntry;

    /// Uses the platform's own root so `resolve` works on every OS.
    fn root() -> String {
        crate::util::lexical_clean(if cfg!(windows) { r"C:\data" } else { "/data" })
    }

    fn sample() -> (Index, String) {
        let r = root();
        let mut b = IndexBuilder::new(&r);
        let logs = b.add(0, RawEntry::dir("logs"));
        b.add(logs, RawEntry::file("a.LOG", 300));
        b.add(logs, RawEntry::file("b.log", 200));
        b.add(logs, RawEntry::file("c.txt", 100));
        b.add(0, RawEntry::file("big.bin", 5000));
        b.add(0, RawEntry::file(".hidden", 10));
        b.add(0, RawEntry::file("empty", 0));
        let idx = b
            .finish(MetaInit {
                scanner: "test".into(),
                volume_id: None,
                case_insensitive: false,
                elapsed_ms: 0,
            })
            .unwrap();
        (idx, r)
    }

    fn join(root: &str, rest: &str) -> String {
        let mut s = root.to_string();
        crate::util::push_join(&mut s, rest);
        s
    }

    #[test]
    fn browse_is_sorted_and_truncates() {
        let (idx, r) = sample();
        let (recs, sum) = browse(&idx, &r, 2, false).unwrap();
        assert_eq!(recs[0].name, "big.bin");
        assert_eq!(recs[1].name, "logs");
        assert_eq!(sum.entries, 4);
        assert_eq!(sum.shown, 2);
        assert!(sum.truncated);
        assert_eq!(sum.remaining_entries, 2);
        assert_eq!(sum.remaining_bytes, 10);
        assert_eq!(sum.allocated_bytes, 5610);
        assert_eq!(recs[0].pct_of_parent, Some(89.1));
    }

    #[test]
    fn largest_filters_by_ext_case_insensitively() {
        let (idx, r) = sample();
        let (recs, sum) = largest(&idx, &r, Some(".log"), None, 10, false).unwrap();
        let names: Vec<&str> = recs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a.LOG", "b.log"]);
        assert_eq!(sum.matched_files, 2);
        assert_eq!(sum.matched_bytes, 500);
        assert!(!sum.truncated);
    }

    #[test]
    fn largest_min_bytes_and_limit() {
        let (idx, r) = sample();
        let (recs, sum) = largest(&idx, &r, None, Some(150), 1, false).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "big.bin");
        assert_eq!(sum.matched_files, 3);
        assert!(sum.truncated);
    }

    #[test]
    fn types_group_by_lowercased_extension() {
        let (idx, r) = sample();
        let (recs, _) = types(&idx, &r, 10).unwrap();
        let get = |e: &str| recs.iter().find(|t| t.ext == e).map(|t| (t.files, t.allocated_bytes));
        assert_eq!(get("log"), Some((2, 500)));
        assert_eq!(get("bin"), Some((1, 5000)));
        assert_eq!(get("(none)"), Some((2, 10)));
    }

    #[test]
    fn detail_and_missing_paths() {
        let (idx, r) = sample();
        let p = join(&r, "logs");
        let (recs, sum) = detail(&idx, &p).unwrap();
        assert_eq!(recs[0].files, Some(3));
        assert_eq!(recs[0].dirs, Some(0));
        assert_eq!(sum.depth, 1);
        assert_eq!(sum.parent.as_deref(), Some(r.as_str()));
        let missing = join(&r, "nope");
        assert!(matches!(detail(&idx, &missing), Err(Error::PathNotInSnapshot { .. })));
    }

    #[test]
    fn extension_rules() {
        assert_eq!(file_ext("a.tar.gz"), Some("gz"));
        assert_eq!(file_ext(".gitignore"), None);
        assert_eq!(file_ext("trailing."), None);
        assert_eq!(file_ext("noext"), None);
    }
}
