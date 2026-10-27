//! Collects raw entries from a scanner (in any order, from one thread at a time) and packs
//! them into an immutable [`Index`]. Public so a future MFT scanner can feed it directly.

use std::collections::HashMap;

use rustc_hash::FxHashMap;

use crate::error::Error;
use crate::model::{
    Index, IndexMeta, Kind, RawEntry, ScanErrors, FLAG_ALIAS, FLAG_CLOUD, FLAG_DENIED, KIND_MASK,
};

#[derive(Clone, Copy, Debug)]
pub enum ErrorCategory {
    Denied,
    Locked,
    Other,
}

/// Facts about the scan that only the scanner knows.
pub struct MetaInit {
    pub scanner: String,
    pub volume_id: Option<String>,
    pub case_insensitive: bool,
    pub elapsed_ms: u64,
}

const MAX_ERROR_SAMPLES: usize = 20;

pub struct IndexBuilder {
    root: String,
    names: Vec<u8>,
    intern: FxHashMap<Box<str>, (u32, u16)>,
    parent: Vec<u32>,
    name_off: Vec<u32>,
    name_len: Vec<u16>,
    flags: Vec<u16>,
    allocated: Vec<u64>,
    logical: Vec<u64>,
    mtime: Vec<u32>,
    /// (file id, node) for files that may be hard linked.
    link_ids: Vec<(u128, u32)>,
    errors: ScanErrors,
}

impl IndexBuilder {
    /// `root` is the normalized absolute path of the scan root; node 0 is that directory.
    pub fn new(root: &str) -> Self {
        let mut b = IndexBuilder {
            root: root.to_string(),
            names: Vec::new(),
            intern: FxHashMap::default(),
            parent: Vec::new(),
            name_off: Vec::new(),
            name_len: Vec::new(),
            flags: Vec::new(),
            allocated: Vec::new(),
            logical: Vec::new(),
            mtime: Vec::new(),
            link_ids: Vec::new(),
            errors: ScanErrors::default(),
        };
        let (off, len) = b.intern(root);
        b.push_node(0, off, len, Kind::Dir as u16, 0, 0, 0);
        b
    }

    pub fn root_id(&self) -> u32 {
        0
    }

    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    fn intern(&mut self, name: &str) -> (u32, u16) {
        if let Some(v) = self.intern.get(name) {
            return *v;
        }
        let mut end = name.len().min(u16::MAX as usize);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let name = &name[..end];
        let off = self.names.len() as u32;
        self.names.extend_from_slice(name.as_bytes());
        let v = (off, end as u16);
        self.intern.insert(name.into(), v);
        v
    }

    #[allow(clippy::too_many_arguments)]
    fn push_node(&mut self, parent: u32, off: u32, len: u16, flags: u16, a: u64, l: u64, m: u32) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(parent);
        self.name_off.push(off);
        self.name_len.push(len);
        self.flags.push(flags);
        self.allocated.push(a);
        self.logical.push(l);
        self.mtime.push(m);
        id
    }

    /// Add one entry under `parent` and return its node id.
    pub fn add(&mut self, parent: u32, e: RawEntry) -> u32 {
        let (off, len) = self.intern(&e.name);
        let flags = (e.kind as u16) | (e.flags & !KIND_MASK);
        let id = self.push_node(parent, off, len, flags, e.allocated, e.logical, e.mtime);
        if e.kind == Kind::File && e.nlink != Some(1) {
            if let Some(fid) = e.file_id {
                self.link_ids.push((fid, id));
            }
        }
        id
    }

    /// Mark a directory as unreadable.
    pub fn mark_denied(&mut self, id: u32) {
        self.flags[id as usize] |= FLAG_DENIED;
    }

    pub fn record_error(&mut self, cat: ErrorCategory, path: &str) {
        match cat {
            ErrorCategory::Denied => self.errors.denied += 1,
            ErrorCategory::Locked => self.errors.locked += 1,
            ErrorCategory::Other => self.errors.other += 1,
        }
        if self.errors.samples.len() < MAX_ERROR_SAMPLES {
            self.errors.samples.push(path.to_string());
        }
    }

    fn name_bytes(&self, i: u32) -> &[u8] {
        let off = self.name_off[i as usize] as usize;
        &self.names[off..off + self.name_len[i as usize] as usize]
    }

    pub fn finish(mut self, init: MetaInit) -> Result<Index, Error> {
        let n = self.parent.len();
        if n >= u32::MAX as usize || self.names.len() >= u32::MAX as usize {
            return Err(Error::ScanFailed("tree is too large for the index format".into()));
        }

        // Children of each node as contiguous slices of `kids`.
        let mut first = vec![0u32; n + 1];
        for i in 1..n {
            first[self.parent[i] as usize + 1] += 1;
        }
        for i in 0..n {
            first[i + 1] += first[i];
        }
        let mut fill = first.clone();
        let mut kids = vec![0u32; n - 1];
        for i in 1..n {
            let p = self.parent[i] as usize;
            kids[fill[p] as usize] = i as u32;
            fill[p] += 1;
        }

        // Pass 1: name-sorted tree, so hard-link ownership does not depend on thread timing.
        for p in 0..n {
            let (a, b) = (first[p] as usize, first[p + 1] as usize);
            let slice = &mut kids[a..b];
            slice.sort_by(|&x, &y| {
                self.name_bytes(x)
                    .cmp(self.name_bytes(y))
                    .then(self.allocated[y as usize].cmp(&self.allocated[x as usize]))
                    .then(self.flags[x as usize].cmp(&self.flags[y as usize]))
            });
        }
        let order = preorder(&first, &kids, n);
        if order.len() != n {
            return Err(Error::Internal("unreachable nodes in scan tree".into()));
        }

        // Hard links: the member that comes first in name-sorted order owns the data.
        let mut pos = vec![0u32; n];
        for (i, &id) in order.iter().enumerate() {
            pos[id as usize] = i as u32;
        }
        let mut link_ids = std::mem::take(&mut self.link_ids);
        link_ids.sort_unstable_by_key(|&(fid, node)| (fid, pos[node as usize]));
        let mut hard_links: HashMap<u32, u32> = HashMap::new();
        let mut hard_link_groups = 0u64;
        let mut i = 0;
        while i < link_ids.len() {
            let mut j = i + 1;
            while j < link_ids.len() && link_ids[j].0 == link_ids[i].0 {
                j += 1;
            }
            if j - i > 1 {
                hard_link_groups += 1;
                for (k, &(_, node)) in link_ids[i..j].iter().enumerate() {
                    hard_links.insert(node, (j - i) as u32);
                    if k > 0 {
                        self.allocated[node as usize] = 0;
                        self.logical[node as usize] = 0;
                        self.flags[node as usize] |= FLAG_ALIAS;
                    }
                }
            }
            i = j;
        }
        drop(link_ids);

        // Directory totals: in pre-order every child follows its parent, so walk it backwards.
        let mut files = vec![0u32; n];
        for (i, f) in files.iter_mut().enumerate() {
            *f = u32::from(Kind::from_flags(self.flags[i]) != Kind::Dir);
        }
        for &id in order.iter().skip(1).rev() {
            let (c, p) = (id as usize, self.parent[id as usize] as usize);
            self.allocated[p] = self.allocated[p].saturating_add(self.allocated[c]);
            self.logical[p] = self.logical[p].saturating_add(self.logical[c]);
            files[p] = files[p].saturating_add(files[c]);
        }

        // Pass 2: siblings by allocated size descending, then emit the final pre-order.
        for p in 0..n {
            let (a, b) = (first[p] as usize, first[p + 1] as usize);
            let slice = &mut kids[a..b];
            slice.sort_by(|&x, &y| {
                self.allocated[y as usize]
                    .cmp(&self.allocated[x as usize])
                    .then(self.name_bytes(x).cmp(self.name_bytes(y)))
            });
        }
        let order = preorder(&first, &kids, n);
        let mut new_pos = vec![0u32; n];
        for (i, &id) in order.iter().enumerate() {
            new_pos[id as usize] = i as u32;
        }

        let mut parent = Vec::with_capacity(n);
        let mut name_off = Vec::with_capacity(n);
        let mut name_len = Vec::with_capacity(n);
        let mut flags = Vec::with_capacity(n);
        let mut allocated = Vec::with_capacity(n);
        let mut logical = Vec::with_capacity(n);
        let mut mtime = Vec::with_capacity(n);
        let mut files_out = Vec::with_capacity(n);
        for (i, &old) in order.iter().enumerate() {
            let o = old as usize;
            parent.push(if i == 0 { 0 } else { new_pos[self.parent[o] as usize] });
            name_off.push(self.name_off[o]);
            name_len.push(self.name_len[o]);
            flags.push(self.flags[o]);
            allocated.push(self.allocated[o]);
            logical.push(self.logical[o]);
            mtime.push(self.mtime[o]);
            files_out.push(files[o]);
        }
        let mut subtree_end: Vec<u32> = (1..=n as u32).collect();
        for i in (1..n).rev() {
            let p = parent[i] as usize;
            subtree_end[p] = subtree_end[p].max(subtree_end[i]);
        }
        let hard_links = hard_links
            .into_iter()
            .map(|(old, links)| (new_pos[old as usize], links))
            .collect();

        let dirs = flags.iter().filter(|f| Kind::from_flags(**f) == Kind::Dir).count() as u64;
        let reparse_points = flags
            .iter()
            .filter(|f| !matches!(Kind::from_flags(**f), Kind::File | Kind::Dir))
            .count() as u64;
        let cloud_placeholders = flags.iter().filter(|f| **f & FLAG_CLOUD != 0).count() as u64;

        let meta = IndexMeta {
            root: self.root,
            taken_at: crate::util::now_secs(),
            volume_id: init.volume_id,
            scanner: init.scanner,
            dredge_version: env!("CARGO_PKG_VERSION").to_string(),
            node_count: n as u64,
            allocated_total: allocated[0],
            logical_total: logical[0],
            files: u64::from(files_out[0]),
            dirs,
            case_insensitive: init.case_insensitive,
            errors: self.errors,
            hard_link_groups,
            reparse_points,
            cloud_placeholders,
            elapsed_ms: init.elapsed_ms,
            volume_total_bytes: None,
            volume_used_bytes: None,
        };
        Ok(Index {
            meta,
            names: self.names,
            parent,
            name_off,
            name_len,
            flags,
            allocated,
            logical,
            mtime,
            subtree_end,
            files: files_out,
            hard_links,
        })
    }
}

/// Pre-order of the tree described by CSR child slices, root first.
fn preorder(first: &[u32], kids: &[u32], n: usize) -> Vec<u32> {
    let mut order = Vec::with_capacity(n);
    let mut stack = vec![0u32];
    while let Some(id) = stack.pop() {
        order.push(id);
        let (a, b) = (first[id as usize] as usize, first[id as usize + 1] as usize);
        for &k in kids[a..b].iter().rev() {
            stack.push(k);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Index;

    fn init() -> MetaInit {
        MetaInit {
            scanner: "test".into(),
            volume_id: None,
            case_insensitive: false,
            elapsed_ms: 0,
        }
    }

    fn sample() -> Index {
        let mut b = IndexBuilder::new("/r");
        let a = b.add(0, RawEntry::dir("a"));
        b.add(a, RawEntry::file("x", 100));
        b.add(0, RawEntry::file("b", 300));
        let mut h1 = RawEntry::file("h1", 50);
        h1.file_id = Some(7);
        h1.nlink = Some(2);
        let mut h2 = h1.clone();
        h2.name = "h2".into();
        b.add(0, h1);
        b.add(0, h2);
        b.finish(init()).unwrap()
    }

    #[test]
    fn totals_hard_links_and_order() {
        let idx = sample();
        // root, b(300), a(100), a/x, h1(50), h2(alias, 0)
        assert_eq!(idx.len(), 6);
        assert_eq!(idx.allocated(0), 450);
        let names: Vec<&str> = idx.children(0).map(|c| idx.name(c)).collect();
        assert_eq!(names, vec!["b", "a", "h1", "h2"]);
        let h2 = idx.children(0).last().unwrap();
        assert_eq!(idx.allocated(h2), 0);
        assert!(idx.flags(h2) & FLAG_ALIAS != 0);
        assert_eq!(idx.hard_links(h2), Some(2));
        assert_eq!(idx.meta.hard_link_groups, 1);
        assert_eq!(idx.meta.files, 4);
        idx.validate().unwrap();
    }

    #[test]
    fn subtree_ranges_nest() {
        let idx = sample();
        assert_eq!(idx.subtree_end(0) as usize, idx.len());
        let a = idx.children(0).find(|&c| idx.name(c) == "a").unwrap();
        assert_eq!(idx.subtree_end(a), a + 2);
        assert_eq!(idx.allocated(a), 100);
        assert_eq!(idx.files(a), 1);
    }

    #[test]
    fn names_are_interned() {
        let mut b = IndexBuilder::new("/r");
        let d1 = b.add(0, RawEntry::dir("d1"));
        let d2 = b.add(0, RawEntry::dir("d2"));
        b.add(d1, RawEntry::file("index.js", 1));
        b.add(d2, RawEntry::file("index.js", 1));
        let idx = b.finish(init()).unwrap();
        let count = idx.names.windows(8).filter(|w| *w == b"index.js").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn denied_dirs_keep_flag() {
        let mut b = IndexBuilder::new("/r");
        let d = b.add(0, RawEntry::dir("locked"));
        b.mark_denied(d);
        b.record_error(ErrorCategory::Denied, "/r/locked");
        let idx = b.finish(init()).unwrap();
        let c = idx.children(0).next().unwrap();
        assert!(idx.flags(c) & FLAG_DENIED != 0);
        assert_eq!(idx.meta.errors.denied, 1);
        assert_eq!(idx.meta.errors.samples, vec!["/r/locked".to_string()]);
    }
}
