//! The in-memory index: struct-of-arrays, DFS pre-order, siblings sorted by allocated size
//! descending. A node's subtree is the contiguous range `[i, subtree_end[i])`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::util::{components, name_eq, push_join};

pub const KIND_MASK: u16 = 0b111;
pub const FLAG_CLOUD: u16 = 1 << 3;
pub const FLAG_DENIED: u16 = 1 << 4;
pub const FLAG_ALIAS: u16 = 1 << 5;
pub const FLAG_ESTIMATED: u16 = 1 << 6;
pub const FLAG_LOSSY: u16 = 1 << 7;
pub const FLAG_MOUNT: u16 = 1 << 8;

/// Flag bits paired with their output names.
pub const FLAG_NAMES: [(u16, &str); 6] = [
    (FLAG_CLOUD, "cloud_placeholder"),
    (FLAG_DENIED, "denied"),
    (FLAG_ALIAS, "hardlink_alias"),
    (FLAG_ESTIMATED, "estimated"),
    (FLAG_LOSSY, "name_lossy"),
    (FLAG_MOUNT, "mount_point"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File = 0,
    Dir = 1,
    Symlink = 2,
    Junction = 3,
    Reparse = 4,
}

impl Kind {
    pub fn from_flags(flags: u16) -> Kind {
        match flags & KIND_MASK {
            1 => Kind::Dir,
            2 => Kind::Symlink,
            3 => Kind::Junction,
            4 => Kind::Reparse,
            _ => Kind::File,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Dir => "dir",
            Kind::Symlink => "symlink",
            Kind::Junction => "junction",
            Kind::Reparse => "reparse",
        }
    }
}

/// One directory entry as reported by a platform backend, before it is placed in the tree.
#[derive(Clone, Debug)]
pub struct RawEntry {
    pub name: String,
    pub kind: Kind,
    /// Flag bits only (`FLAG_*`); the kind is carried separately.
    pub flags: u16,
    pub allocated: u64,
    pub logical: u64,
    /// Unix seconds, 0 when unknown.
    pub mtime: u32,
    /// Filesystem file ID, used to dedupe hard links. Unique within one volume.
    pub file_id: Option<u128>,
    /// Link count when the platform reports it (`Some(1)` means "not hard linked").
    pub nlink: Option<u32>,
}

impl RawEntry {
    pub fn new(name: impl Into<String>, kind: Kind, allocated: u64, logical: u64) -> Self {
        RawEntry {
            name: name.into(),
            kind,
            flags: 0,
            allocated,
            logical,
            mtime: 0,
            file_id: None,
            nlink: None,
        }
    }

    pub fn file(name: impl Into<String>, allocated: u64) -> Self {
        Self::new(name, Kind::File, allocated, allocated)
    }

    pub fn dir(name: impl Into<String>) -> Self {
        Self::new(name, Kind::Dir, 0, 0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScanErrors {
    pub denied: u64,
    pub locked: u64,
    pub other: u64,
    /// At most 20 example paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<String>,
}

impl ScanErrors {
    pub fn total(&self) -> u64 {
        self.denied + self.locked + self.other
    }
}

/// Snapshot header: everything about a scan that is not the tree itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexMeta {
    pub root: String,
    /// Unix seconds.
    pub taken_at: i64,
    pub volume_id: Option<String>,
    pub scanner: String,
    pub dredge_version: String,
    pub node_count: u64,
    pub allocated_total: u64,
    pub logical_total: u64,
    pub files: u64,
    pub dirs: u64,
    pub case_insensitive: bool,
    #[serde(default)]
    pub errors: ScanErrors,
    #[serde(default)]
    pub hard_link_groups: u64,
    #[serde(default)]
    pub reparse_points: u64,
    #[serde(default)]
    pub cloud_placeholders: u64,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub volume_total_bytes: Option<u64>,
    #[serde(default)]
    pub volume_used_bytes: Option<u64>,
}

pub struct Index {
    pub meta: IndexMeta,
    pub(crate) names: Vec<u8>,
    pub(crate) parent: Vec<u32>,
    pub(crate) name_off: Vec<u32>,
    pub(crate) name_len: Vec<u16>,
    pub(crate) flags: Vec<u16>,
    pub(crate) allocated: Vec<u64>,
    pub(crate) logical: Vec<u64>,
    pub(crate) mtime: Vec<u32>,
    pub(crate) subtree_end: Vec<u32>,
    pub(crate) files: Vec<u32>,
    /// Owner and aliases of each hard-link group: node -> links seen in the scan.
    pub(crate) hard_links: HashMap<u32, u32>,
}

pub struct Children<'a> {
    idx: &'a Index,
    next: u32,
    end: u32,
}

impl Iterator for Children<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.next >= self.end {
            return None;
        }
        let c = self.next;
        self.next = self.idx.subtree_end[c as usize];
        Some(c)
    }
}

impl Index {
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    pub fn name(&self, i: u32) -> &str {
        let i = i as usize;
        let off = self.name_off[i] as usize;
        let end = off + self.name_len[i] as usize;
        self.names
            .get(off..end)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("\u{fffd}")
    }

    pub fn parent(&self, i: u32) -> u32 {
        self.parent[i as usize]
    }

    pub fn flags(&self, i: u32) -> u16 {
        self.flags[i as usize]
    }

    pub fn kind(&self, i: u32) -> Kind {
        Kind::from_flags(self.flags[i as usize])
    }

    pub fn is_dir(&self, i: u32) -> bool {
        self.kind(i) == Kind::Dir
    }

    pub fn allocated(&self, i: u32) -> u64 {
        self.allocated[i as usize]
    }

    pub fn logical(&self, i: u32) -> u64 {
        self.logical[i as usize]
    }

    pub fn mtime(&self, i: u32) -> u32 {
        self.mtime[i as usize]
    }

    pub fn files(&self, i: u32) -> u32 {
        self.files[i as usize]
    }

    pub fn subtree_end(&self, i: u32) -> u32 {
        self.subtree_end[i as usize]
    }

    pub fn hard_links(&self, i: u32) -> Option<u32> {
        self.hard_links.get(&i).copied()
    }

    /// Directories strictly below `i`.
    pub fn dir_count_below(&self, i: u32) -> u64 {
        let end = self.subtree_end[i as usize] as usize;
        self.flags[i as usize + 1..end]
            .iter()
            .filter(|f| Kind::from_flags(**f) == Kind::Dir)
            .count() as u64
    }

    /// Immediate children, in stored order (allocated size descending).
    pub fn children(&self, i: u32) -> Children<'_> {
        Children {
            idx: self,
            next: i + 1,
            end: self.subtree_end[i as usize],
        }
    }

    pub fn depth(&self, mut i: u32) -> u32 {
        let mut d = 0;
        while i != 0 {
            i = self.parent[i as usize];
            d += 1;
        }
        d
    }

    pub fn path(&self, i: u32) -> String {
        let mut chain = Vec::new();
        let mut cur = i;
        while cur != 0 {
            chain.push(cur);
            cur = self.parent[cur as usize];
        }
        let mut s = self.meta.root.clone();
        for &n in chain.iter().rev() {
            push_join(&mut s, self.name(n));
        }
        s
    }

    /// Find the node for an absolute, normalized path. Exact name matches win over case-folded ones.
    pub fn resolve(&self, path: &str) -> Option<u32> {
        let ci = self.meta.case_insensitive;
        let root = components(&self.meta.root);
        let q = components(path);
        if q.len() < root.len() || !root.iter().zip(q.iter()).all(|(a, b)| name_eq(a, b, ci)) {
            return None;
        }
        let mut cur = 0u32;
        for comp in &q[root.len()..] {
            let mut exact = None;
            let mut folded = None;
            for c in self.children(cur) {
                let n = self.name(c);
                if n == *comp {
                    exact = Some(c);
                    break;
                }
                if ci && folded.is_none() && name_eq(n, comp, true) {
                    folded = Some(c);
                }
            }
            cur = exact.or(folded)?;
        }
        Some(cur)
    }

    /// Structural checks used after loading a snapshot from disk.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.parent.len();
        let same = [
            self.name_off.len(),
            self.name_len.len(),
            self.flags.len(),
            self.allocated.len(),
            self.logical.len(),
            self.mtime.len(),
            self.subtree_end.len(),
            self.files.len(),
        ];
        if n == 0 || same.iter().any(|l| *l != n) {
            return Err("column length mismatch".into());
        }
        if n as u64 != self.meta.node_count {
            return Err("node count does not match header".into());
        }
        for i in 0..n {
            let end = self.subtree_end[i] as usize;
            if end <= i || end > n {
                return Err(format!("bad subtree range at node {i}"));
            }
            if i > 0 && (self.parent[i] as usize) >= i {
                return Err(format!("bad parent at node {i}"));
            }
            if self.name_off[i] as usize + self.name_len[i] as usize > self.names.len() {
                return Err(format!("name out of range at node {i}"));
            }
        }
        Ok(())
    }
}
