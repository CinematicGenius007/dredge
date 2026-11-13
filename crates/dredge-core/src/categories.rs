//! Categories and cleanup hints.
//!
//! Both are static tables of data ([`RULES`] and [`HINTS`]) matched during linear walks over the
//! pre-order index. No path is rebuilt per node: a walk keeps a stack of the directories above
//! the current node, and matchers read path components from that stack (plus the components of
//! the snapshot root). A directory matched by a rule claims its whole subtree, which is then
//! skipped. Nothing here reads file contents or runs anything; a hint's command is text only.

use schemars::JsonSchema;
use serde::Serialize;

use crate::model::{Index, Kind, FLAG_CLOUD, FLAG_LOSSY, FLAG_MOUNT};
use crate::output::{CategoriesSummary, CategoryRecord, InsightRecord, InsightsSummary};
use crate::util::{components, human_bytes, pct};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Category {
    Apps,
    Developer,
    Photos,
    Movies,
    Music,
    Documents,
    Caches,
    Trash,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Apps,
        Category::Developer,
        Category::Photos,
        Category::Movies,
        Category::Music,
        Category::Documents,
        Category::Caches,
        Category::Trash,
        Category::System,
        Category::Other,
    ];
}

/// Which host a rule applies to. `Linux` also covers the other Unix systems.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Os {
    Any,
    Macos,
    Windows,
    Linux,
}

impl Os {
    fn applies(self, host: Os) -> bool {
        self == Os::Any || self == host
    }
}

/// The OS the snapshots on this machine come from.
pub fn host_os() -> Os {
    match std::env::consts::OS {
        "macos" => Os::Macos,
        "windows" => Os::Windows,
        _ => Os::Linux,
    }
}

/// How a rule recognizes a node. Only files and directories match; links never do.
/// Names compare case-insensitively (ASCII) when the snapshot's filesystem is; extensions always.
pub enum Matcher {
    /// A file whose extension (after the last dot, not a leading dot) is one of these.
    /// Directories never match: `com.apple.iWork.Pages` is a folder, not a document.
    Extension(&'static [&'static str]),
    /// Like `Extension`, but a directory with the extension matches too. Only for real bundle
    /// types (`Foo.app`, `x.photoslibrary`), each listed with its source.
    Bundle(&'static [&'static str]),
    /// A directory with this name that has a sibling file with the second name (a Cargo
    /// `target` next to `Cargo.toml`).
    DirBeside(&'static str, &'static str),
    /// A directory with exactly this name, anywhere.
    DirName(&'static str),
    /// A node whose last path components are these. `*` matches any one component.
    PathSuffix(&'static [&'static str]),
    /// A node whose whole path is these components (a drive like `C:` is one component). `*`
    /// matches any one component.
    RootPath(&'static [&'static str]),
}

pub struct Rule {
    pub category: Category,
    pub matcher: Matcher,
    pub os: Os,
    /// Where the convention is documented.
    pub source: &'static str,
}

const fn rule(category: Category, matcher: Matcher, os: Os, source: &'static str) -> Rule {
    Rule {
        category,
        matcher,
        os,
        source,
    }
}

use Category::*;
use Matcher::*;

/// Category rules. The first match wins and a matched directory claims its whole subtree, so
/// path and directory rules come first and extension rules last.
pub static RULES: &[Rule] = &[
    // https://docs.npmjs.com/cli/v10/configuring-npm/folders#node-modules
    rule(
        Developer,
        DirName("node_modules"),
        Os::Any,
        "https://docs.npmjs.com/cli/v10/configuring-npm/folders",
    ),
    // https://doc.rust-lang.org/cargo/guide/cargo-home.html
    rule(
        Developer,
        PathSuffix(&[".cargo", "registry"]),
        Os::Any,
        "https://doc.rust-lang.org/cargo/guide/cargo-home.html",
    ),
    // https://doc.rust-lang.org/cargo/reference/build-cache.html (Cargo) and
    // https://maven.apache.org/guides/introduction/introduction-to-the-standard-directory-layout.html
    rule(
        Developer,
        DirBeside("target", "Cargo.toml"),
        Os::Any,
        "https://doc.rust-lang.org/cargo/reference/build-cache.html",
    ),
    // https://docs.gradle.org/current/userguide/directory_layout.html
    rule(
        Developer,
        DirName(".gradle"),
        Os::Any,
        "https://docs.gradle.org/current/userguide/directory_layout.html",
    ),
    // https://docs.npmjs.com/cli/v10/commands/npm-cache (default cache on Unix: ~/.npm)
    rule(
        Developer,
        DirName(".npm"),
        Os::Any,
        "https://docs.npmjs.com/cli/v10/commands/npm-cache",
    ),
    // https://docs.npmjs.com/cli/v10/commands/npm-cache (default cache on Windows)
    rule(
        Developer,
        PathSuffix(&["AppData", "Local", "npm-cache"]),
        Os::Windows,
        "https://docs.npmjs.com/cli/v10/commands/npm-cache",
    ),
    // https://peps.python.org/pep-3147/
    rule(
        Developer,
        DirName("__pycache__"),
        Os::Any,
        "https://peps.python.org/pep-3147/",
    ),
    // https://docs.python.org/3/library/venv.html
    rule(
        Developer,
        DirName(".venv"),
        Os::Any,
        "https://docs.python.org/3/library/venv.html",
    ),
    // Xcode build data and simulators live under ~/Library/Developer.
    // https://developer.apple.com/documentation/xcode/build-system
    rule(
        Developer,
        PathSuffix(&["Library", "Developer", "Xcode", "DerivedData"]),
        Os::Macos,
        "https://developer.apple.com/documentation/xcode/build-system",
    ),
    // https://developer.apple.com/documentation/xcode/running-your-app-in-simulator-or-on-a-device
    rule(
        Developer,
        PathSuffix(&["Library", "Developer", "CoreSimulator"]),
        Os::Macos,
        "https://developer.apple.com/documentation/xcode/running-your-app-in-simulator-or-on-a-device",
    ),
    // `Library/Caches` (user, local and app-container libraries), never a bare `Caches`.
    // https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html
    rule(
        Caches,
        PathSuffix(&["Library", "Caches"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html",
    ),
    // The per-user Trash is ~/.Trash.
    // https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html
    rule(
        Trash,
        PathSuffix(&["Users", "*", ".Trash"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html",
    ),
    // https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html
    rule(
        Apps,
        RootPath(&["Applications"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html",
    ),
    rule(
        Apps,
        PathSuffix(&["Users", "*", "Applications"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html",
    ),
    rule(
        System,
        RootPath(&["System"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html",
    ),
    // Swap files and the sleep image (`man 8 dynamic_pager`); on the Data volume since 10.15.
    rule(
        System,
        PathSuffix(&["private", "var", "vm"]),
        Os::Macos,
        "https://developer.apple.com/library/archive/documentation/Darwin/Reference/ManPages/man8/dynamic_pager.8.html",
    ),
    // $XDG_DATA_HOME/Trash, by default ~/.local/share/Trash.
    // https://specifications.freedesktop.org/trash-spec/latest/
    rule(
        Trash,
        PathSuffix(&[".local", "share", "Trash"]),
        Os::Linux,
        "https://specifications.freedesktop.org/trash-spec/latest/",
    ),
    // $XDG_CACHE_HOME defaults to $HOME/.cache: only a home directory's `.cache`.
    // https://specifications.freedesktop.org/basedir-spec/latest/
    rule(
        Caches,
        PathSuffix(&["home", "*", ".cache"]),
        Os::Linux,
        "https://specifications.freedesktop.org/basedir-spec/latest/",
    ),
    rule(
        Caches,
        RootPath(&["root", ".cache"]),
        Os::Linux,
        "https://specifications.freedesktop.org/basedir-spec/latest/",
    ),
    // %TEMP% for a user is %LOCALAPPDATA%\Temp.
    // https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-gettemppathw
    rule(
        Caches,
        PathSuffix(&["Users", "*", "AppData", "Local", "Temp"]),
        Os::Windows,
        "https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-gettemppathw",
    ),
    // https://learn.microsoft.com/powershell/module/microsoft.powershell.management/clear-recyclebin
    rule(
        Trash,
        RootPath(&["*", "$Recycle.Bin"]),
        Os::Windows,
        "https://learn.microsoft.com/powershell/module/microsoft.powershell.management/clear-recyclebin",
    ),
    // https://learn.microsoft.com/windows/win32/shell/knownfolderid
    rule(
        System,
        RootPath(&["*", "Windows"]),
        Os::Windows,
        "https://learn.microsoft.com/windows/win32/shell/knownfolderid",
    ),
    rule(
        Apps,
        RootPath(&["*", "Program Files"]),
        Os::Windows,
        "https://learn.microsoft.com/windows/win32/shell/knownfolderid",
    ),
    rule(
        Apps,
        RootPath(&["*", "Program Files (x86)"]),
        Os::Windows,
        "https://learn.microsoft.com/windows/win32/shell/knownfolderid",
    ),
    // https://learn.microsoft.com/troubleshoot/windows-client/performance/introduction-to-the-page-file
    rule(
        System,
        RootPath(&["*", "pagefile.sys"]),
        Os::Windows,
        "https://learn.microsoft.com/troubleshoot/windows-client/performance/introduction-to-the-page-file",
    ),
    rule(
        System,
        RootPath(&["*", "swapfile.sys"]),
        Os::Windows,
        "https://learn.microsoft.com/troubleshoot/windows-client/performance/introduction-to-the-page-file",
    ),
    rule(
        System,
        RootPath(&["*", "hiberfil.sys"]),
        Os::Windows,
        "https://learn.microsoft.com/troubleshoot/windows-client/deployment/disable-and-re-enable-hibernation",
    ),
    // Extension rules match files only. Directories match only through `Bundle`, which lists
    // real bundle types.
    // `.app`: https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html
    rule(
        Apps,
        Bundle(&["app"]),
        Os::Any,
        "https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html",
    ),
    // https://learn.microsoft.com/windows/win32/msi/windows-installer-file-extensions
    rule(
        Apps,
        Extension(&["dmg", "pkg", "exe", "msi", "apk"]),
        Os::Any,
        "https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html",
    ),
    // https://www.iana.org/assignments/media-types/media-types.xhtml#image plus camera raw
    // formats (https://helpx.adobe.com/camera-raw/kb/camera-raw-plug-supported-cameras.html)
    // and the macOS Photos library package.
    // `.photoslibrary` is a bundle: https://support.apple.com/guide/photos/welcome/mac
    rule(
        Photos,
        Bundle(&["photoslibrary"]),
        Os::Any,
        "https://support.apple.com/guide/photos/welcome/mac",
    ),
    rule(
        Photos,
        Extension(&[
            "jpg",
            "jpeg",
            "png",
            "heic",
            "heif",
            "gif",
            "webp",
            "tif",
            "tiff",
            "raw",
            "dng",
            "cr2",
            "cr3",
            "nef",
            "arw",
        ]),
        Os::Any,
        "https://www.iana.org/assignments/media-types/media-types.xhtml#image",
    ),
    // https://www.iana.org/assignments/media-types/media-types.xhtml#video
    rule(
        Movies,
        Extension(&["mp4", "m4v", "mov", "mkv", "avi", "webm"]),
        Os::Any,
        "https://www.iana.org/assignments/media-types/media-types.xhtml#video",
    ),
    // https://www.iana.org/assignments/media-types/media-types.xhtml#audio
    rule(
        Music,
        Extension(&["mp3", "m4a", "aac", "flac", "wav", "ogg"]),
        Os::Any,
        "https://www.iana.org/assignments/media-types/media-types.xhtml#audio",
    ),
    // https://www.iana.org/assignments/media-types/media-types.xhtml#application
    rule(
        Documents,
        Extension(&[
            "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt",
            "md", "epub", "pages", "numbers",
        ]),
        Os::Any,
        "https://www.iana.org/assignments/media-types/media-types.xhtml#application",
    ),
];

/// What a hint's command does, as text for the user to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Remove the directory and everything in it.
    RemoveDir,
    /// Remove everything in the directory, keeping the directory itself.
    RemoveContents,
    /// Empty the Windows Recycle Bin of the path's drive.
    ClearRecycleBin,
}

/// A cleanup hint: a directory whose contents the tool that owns them re-creates (or, for
/// Trash, that the user already threw away).
pub struct Hint {
    pub id: &'static str,
    pub matcher: Matcher,
    pub os: Os,
    /// When not empty, the directory must have a sibling with one of these names.
    pub requires_sibling: &'static [&'static str],
    /// The hint does not apply below a directory with one of these names (`*.ext` matches an
    /// extension).
    pub deny_ancestors: &'static [&'static str],
    pub action: Action,
    pub message: &'static str,
    pub source: &'static str,
}

/// Lockfiles from which npm, Yarn, pnpm and Bun reinstall `node_modules`.
const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
];

/// Places that ship a prebuilt `node_modules` nobody reinstalls: apps, editor extensions,
/// global installs and nested packages.
const PREBUILT_NODE_MODULES: &[&str] = &[
    "node_modules",
    "*.app",
    "Applications",
    "Library",
    "AppData",
    "Program Files",
    "Program Files (x86)",
    "extensions",
    ".vscode",
    ".vscode-insiders",
    ".vscode-server",
    ".cursor",
    ".cursor-server",
];

/// Hint rules. Every entry must name a directory whose loss costs only a re-download, a rebuild
/// or (Trash) items the user already deleted. Matching stops at the first hint; nothing nested
/// inside a matched directory gets a second hint.
pub static HINTS: &[Hint] = &[
    // `npm ci` / `npm install` (and Yarn, pnpm, Bun) recreate node_modules from the lockfile.
    // https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json
    Hint {
        id: "node_modules",
        matcher: DirName("node_modules"),
        os: Os::Any,
        requires_sibling: LOCKFILES,
        deny_ancestors: PREBUILT_NODE_MODULES,
        action: Action::RemoveDir,
        message: "Installed packages for the project beside it; the package manager reinstalls \
them from the lockfile.",
        source: "https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json",
    },
    // The npm cache is content-addressed and refetched on demand (`npm cache clean --force`).
    // https://docs.npmjs.com/cli/v10/commands/npm-cache
    Hint {
        id: "npm_cache",
        matcher: PathSuffix(&[".npm", "_cacache"]),
        os: Os::Any,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveDir,
        message: "npm's download cache; npm fetches packages again when needed (same effect as \
`npm cache clean --force`).",
        source: "https://docs.npmjs.com/cli/v10/commands/npm-cache",
    },
    Hint {
        id: "npm_cache",
        matcher: PathSuffix(&["AppData", "Local", "npm-cache", "_cacache"]),
        os: Os::Windows,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveDir,
        message: "npm's download cache; npm fetches packages again when needed (same effect as \
`npm cache clean --force`).",
        source: "https://docs.npmjs.com/cli/v10/commands/npm-cache",
    },
    // $CARGO_HOME/registry holds the index, crate archives and extracted sources.
    // https://doc.rust-lang.org/cargo/guide/cargo-home.html
    Hint {
        id: "cargo_registry",
        matcher: PathSuffix(&[".cargo", "registry"]),
        os: Os::Any,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveDir,
        message: "Cargo's downloaded crate index, archives and sources; Cargo downloads them \
again on the next build (offline builds need them).",
        source: "https://doc.rust-lang.org/cargo/guide/cargo-home.html",
    },
    // https://docs.gradle.org/current/userguide/directory_layout.html#dir:gradle_user_home
    Hint {
        id: "gradle_caches",
        matcher: PathSuffix(&[".gradle", "caches"]),
        os: Os::Any,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveDir,
        message: "Gradle's dependency and build caches; Gradle downloads or rebuilds them. Stop \
running daemons first (`gradle --stop`).",
        source: "https://docs.gradle.org/current/userguide/directory_layout.html",
    },
    // https://developer.apple.com/documentation/xcode/build-system
    Hint {
        id: "xcode_derived_data",
        matcher: PathSuffix(&["Users", "*", "Library", "Developer", "Xcode", "DerivedData"]),
        os: Os::Macos,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveContents,
        message: "Xcode build products and indexes; Xcode rebuilds them. Quit Xcode first.",
        source: "https://developer.apple.com/documentation/xcode/build-system",
    },
    // "Your app should be able to re-create these files as needed."
    // https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html
    Hint {
        id: "user_caches",
        matcher: PathSuffix(&["Users", "*", "Library", "Caches"]),
        os: Os::Macos,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveContents,
        message: "Per-user app caches; apps re-create them. Quit apps first; some will download \
content again.",
        source: "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html",
    },
    // $XDG_CACHE_HOME is for "user-specific non-essential (cached) data".
    // https://specifications.freedesktop.org/basedir-spec/latest/
    Hint {
        id: "user_caches",
        matcher: PathSuffix(&["home", "*", ".cache"]),
        os: Os::Linux,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveContents,
        message: "Per-user non-essential cached data; programs re-create it. Close programs \
first; some will download content again.",
        source: "https://specifications.freedesktop.org/basedir-spec/latest/",
    },
    // https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html
    Hint {
        id: "trash",
        matcher: PathSuffix(&["Users", "*", ".Trash"]),
        os: Os::Macos,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveContents,
        message: "Items in the Trash. Emptying it deletes them permanently.",
        source: "https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html",
    },
    // https://specifications.freedesktop.org/trash-spec/latest/
    Hint {
        id: "trash",
        matcher: PathSuffix(&[".local", "share", "Trash"]),
        os: Os::Linux,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::RemoveContents,
        message: "Items in the Trash. Emptying it deletes them permanently.",
        source: "https://specifications.freedesktop.org/trash-spec/latest/",
    },
    // https://learn.microsoft.com/powershell/module/microsoft.powershell.management/clear-recyclebin
    Hint {
        id: "trash",
        matcher: RootPath(&["*", "$Recycle.Bin"]),
        os: Os::Windows,
        requires_sibling: &[],
        deny_ancestors: &[],
        action: Action::ClearRecycleBin,
        message: "Items in the Recycle Bin on this drive. Emptying it deletes them permanently.",
        source: "https://learn.microsoft.com/powershell/module/microsoft.powershell.management/clear-recyclebin",
    },
];

/// Hints below this size are not worth a line.
pub const MIN_INSIGHT_BYTES: u64 = 100 * 1024 * 1024;

const EMPTY: u8 = 0xFE;
const MIXED: u8 = 0xFF;

/// The classification of every node of one snapshot, one byte per node.
///
/// For a file, or for any node inside a directory claimed by a rule, the code is that node's
/// category. For an unclaimed directory it is the single category of all the bytes below it,
/// `EMPTY` when nothing below it has bytes, or `MIXED`. Zero-byte nodes (empty files, hard-link
/// aliases, placeholders) never make a directory mixed.
pub struct Classes {
    codes: Vec<u8>,
}

impl Classes {
    /// The node's category: a file's own, a claimed directory's, or the one category of
    /// everything below a directory. `None` for a directory whose content spans categories.
    pub fn category(&self, i: u32) -> Option<Category> {
        match self.codes[i as usize] {
            MIXED => None,
            EMPTY => Some(Other),
            c => Some(Category::ALL[c as usize]),
        }
    }

    fn is_mixed(&self, i: u32) -> bool {
        self.codes[i as usize] == MIXED
    }
}

fn combine(a: u8, b: u8) -> u8 {
    match (a, b) {
        (EMPTY, x) | (x, EMPTY) => x,
        (x, y) if x == y => x,
        _ => MIXED,
    }
}

/// Fold a finished node's code into its parent's.
fn fold(idx: &Index, codes: &mut [u8], j: u32) {
    if j == 0 {
        return;
    }
    let c = if idx.allocated(j) == 0 {
        EMPTY
    } else {
        codes[j as usize]
    };
    let p = idx.parent(j) as usize;
    codes[p] = combine(codes[p], c);
}

fn name_eq(a: &str, b: &str, ci: bool) -> bool {
    if ci {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Extension without the dot; `None` for no extension or a leading-dot name like `.gitignore`.
fn ext_of(name: &str) -> Option<&str> {
    match name.rfind('.') {
        Some(p) if p > 0 && p + 1 < name.len() => Some(&name[p + 1..]),
        _ => None,
    }
}

/// Path context for a pre-order walk: the snapshot root's components and the directories on the
/// path from the root (exclusive) to the current node (exclusive).
struct Walker<'a> {
    idx: &'a Index,
    root: Vec<&'a str>,
    stack: Vec<u32>,
    ci: bool,
}

impl<'a> Walker<'a> {
    /// A walk that starts at `start`; its ancestors are put on the stack once.
    fn new(idx: &'a Index, start: u32) -> Self {
        let mut stack = Vec::new();
        let mut cur = start;
        while cur != 0 {
            cur = idx.parent(cur);
            if cur != 0 {
                stack.push(cur);
            }
        }
        stack.reverse();
        Walker {
            idx,
            root: components(&idx.meta.root),
            stack,
            ci: idx.meta.case_insensitive,
        }
    }

    /// Pop directories whose subtree ended before `i`, handing each to `done`.
    fn advance(&mut self, i: u32, mut done: impl FnMut(u32)) {
        while let Some(&t) = self.stack.last() {
            if self.idx.subtree_end(t) > i {
                break;
            }
            self.stack.pop();
            done(t);
        }
    }

    fn descend(&mut self, i: u32) {
        if i != 0 && self.idx.is_dir(i) {
            self.stack.push(i);
        }
    }

    /// Number of components in node `i`'s path.
    fn depth(&self, i: u32) -> usize {
        if i == 0 {
            self.root.len()
        } else {
            self.root.len() + self.stack.len() + 1
        }
    }

    /// The `k`-th component of node `i`'s path counted from the end; 0 is `i`'s own name.
    fn component(&self, i: u32, k: usize) -> Option<&'a str> {
        let from_root = |k: usize| self.root.len().checked_sub(k + 1).map(|j| self.root[j]);
        if i == 0 {
            return from_root(k);
        }
        if k == 0 {
            return Some(self.idx.name(i));
        }
        let s = self.stack.len();
        if k - 1 < s {
            return Some(self.idx.name(self.stack[s - k]));
        }
        from_root(k - 1 - s)
    }

    fn suffix(&self, i: u32, parts: &[&str]) -> bool {
        parts.iter().rev().enumerate().all(|(k, p)| {
            self.component(i, k)
                .is_some_and(|c| *p == "*" || name_eq(c, p, self.ci))
        })
    }

    fn ext_in(&self, i: u32, list: &[&str]) -> bool {
        self.component(i, 0)
            .and_then(ext_of)
            .is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }

    fn matches(&self, m: &Matcher, i: u32) -> bool {
        let kind = self.idx.kind(i);
        if !matches!(kind, Kind::File | Kind::Dir) {
            return false;
        }
        match m {
            Extension(list) => kind == Kind::File && self.ext_in(i, list),
            Bundle(list) => self.ext_in(i, list),
            DirBeside(n, sibling) => {
                kind == Kind::Dir
                    && i != 0
                    && self.component(i, 0).is_some_and(|c| name_eq(c, n, self.ci))
                    && self.idx.children(self.idx.parent(i)).any(|s| {
                        self.idx.kind(s) == Kind::File
                            && name_eq(self.idx.name(s), sibling, self.ci)
                    })
            }
            DirName(n) => {
                kind == Kind::Dir && self.component(i, 0).is_some_and(|c| name_eq(c, n, self.ci))
            }
            PathSuffix(parts) => self.depth(i) >= parts.len() && self.suffix(i, parts),
            RootPath(parts) => self.depth(i) == parts.len() && self.suffix(i, parts),
        }
    }

    fn deny_match(&self, c: &str, pattern: &str) -> bool {
        match pattern.strip_prefix("*.") {
            Some(ext) => ext_of(c).is_some_and(|e| e.eq_ignore_ascii_case(ext)),
            None => name_eq(c, pattern, self.ci),
        }
    }

    fn hint_applies(&self, h: &Hint, i: u32) -> bool {
        if !self.idx.is_dir(i) || !self.matches(&h.matcher, i) {
            return false;
        }
        if !h.deny_ancestors.is_empty()
            && (1..self.depth(i)).any(|k| {
                self.component(i, k)
                    .is_some_and(|c| h.deny_ancestors.iter().any(|d| self.deny_match(c, d)))
            })
        {
            return false;
        }
        if !h.requires_sibling.is_empty() {
            if i == 0 {
                return false;
            }
            let idx = self.idx;
            return idx.children(idx.parent(i)).any(|s| {
                s != i
                    && idx.kind(s) == Kind::File
                    && h.requires_sibling
                        .iter()
                        .any(|n| name_eq(idx.name(s), n, self.ci))
            });
        }
        true
    }
}

/// Classify every node in one linear pass over the pre-order index.
pub fn classify(idx: &Index, os: Os) -> Classes {
    let n = idx.len() as u32;
    let mut codes = vec![EMPTY; n as usize];
    let mut w = Walker::new(idx, 0);
    let rules: Vec<&Rule> = RULES.iter().filter(|r| r.os.applies(os)).collect();
    let mut i = 0;
    while i < n {
        w.advance(i, |d| fold(idx, &mut codes, d));
        let end = idx.subtree_end(i);
        if let Some(r) = rules.iter().find(|r| w.matches(&r.matcher, i)) {
            // A matched directory claims its whole subtree without matching inside it.
            codes[i as usize..end as usize].fill(r.category as u8);
            fold(idx, &mut codes, i);
            i = end;
        } else if idx.is_dir(i) {
            // Stays EMPTY until its children fold in when it is popped.
            w.descend(i);
            i += 1;
        } else {
            codes[i as usize] = Other as u8;
            fold(idx, &mut codes, i);
            i += 1;
        }
    }
    w.advance(n, |d| fold(idx, &mut codes, d));
    Classes { codes }
}

/// Keep the three best `(bytes, node)` pairs: larger first, then earlier in pre-order.
fn push_top(top: &mut Vec<(u64, u32)>, item: (u64, u32)) {
    let pos = top
        .iter()
        .position(|&(b, n)| item.0 > b || (item.0 == b && item.1 < n))
        .unwrap_or(top.len());
    if pos < 3 {
        top.insert(pos, item);
        top.truncate(3);
    }
}

/// Space under `root` by category. Rows sum to `root`'s allocated bytes: a directory whose
/// content spans categories contributes its own bytes to `other`, and every single-category
/// subtree is counted whole without being walked.
pub fn categories(
    idx: &Index,
    classes: &Classes,
    root: u32,
    limit: usize,
) -> (Vec<CategoryRecord>, CategoriesSummary) {
    let total = idx.allocated(root);
    let mut bytes = [0i128; Category::ALL.len()];
    let mut files = [0u64; Category::ALL.len()];
    let mut top: Vec<Vec<(u64, u32)>> = vec![Vec::new(); Category::ALL.len()];
    let other = Other as usize;
    let end = idx.subtree_end(root);
    let mut i = root;
    while i < end {
        if classes.is_mixed(i) {
            // A mixed directory's own bytes are `other`; each child below takes its share back.
            if i == root {
                bytes[other] += i128::from(total);
            }
            i += 1;
            continue;
        }
        let c = classes.category(i).unwrap_or(Other) as usize;
        let a = idx.allocated(i);
        bytes[c] += i128::from(a);
        if i != root {
            bytes[other] -= i128::from(a);
        }
        files[c] += u64::from(idx.files(i));
        if a > 0 {
            push_top(&mut top[c], (a, i));
        }
        i = idx.subtree_end(i);
    }
    let mut rows: Vec<CategoryRecord> = Category::ALL
        .iter()
        .enumerate()
        .filter(|&(c, _)| bytes[c] > 0 || files[c] > 0)
        .map(|(c, &category)| {
            let allocated_bytes = u64::try_from(bytes[c].max(0)).unwrap_or(u64::MAX);
            CategoryRecord {
                category,
                allocated_bytes,
                human: human_bytes(allocated_bytes),
                share: pct(allocated_bytes, total),
                files: files[c],
                top: Vec::new(),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.allocated_bytes
            .cmp(&a.allocated_bytes)
            .then(a.category.cmp(&b.category))
    });
    let count = rows.len();
    rows.truncate(limit);
    for r in &mut rows {
        r.top = top[r.category as usize]
            .iter()
            .map(|&(_, n)| idx.path(n))
            .collect();
    }
    (
        rows,
        CategoriesSummary {
            path: idx.path(root),
            allocated_bytes: total,
            human: human_bytes(total),
            categories: count,
            shown: count.min(limit),
            truncated: count > limit,
        },
    )
}

/// POSIX shell single quoting: `'` becomes `'\''`.
pub fn quote_posix(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// PowerShell single quoting. PowerShell also ends a single-quoted string at the typographic
/// single quotes U+2018 to U+201B, so each of those is doubled like `'`.
pub fn quote_powershell(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(ch);
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

/// The command text for a hint, or `None` when no safe command can be written.
pub fn suggested_command(action: Action, path: &str, os: Os) -> Option<String> {
    let windows = os == Os::Windows;
    Some(match action {
        Action::RemoveDir if windows => format!(
            "Remove-Item -LiteralPath {} -Recurse -Force",
            quote_powershell(path)
        ),
        Action::RemoveDir => format!("rm -rf -- {}", quote_posix(path)),
        Action::RemoveContents if windows => format!(
            "Get-ChildItem -LiteralPath {} -Force | Remove-Item -Recurse -Force",
            quote_powershell(path)
        ),
        Action::RemoveContents => {
            format!("find {} -xdev -mindepth 1 -delete", quote_posix(path))
        }
        Action::ClearRecycleBin => {
            let mut it = path.chars();
            match (it.next(), it.next()) {
                (Some(d), Some(':')) if windows && d.is_ascii_alphabetic() => format!(
                    "Clear-RecycleBin -DriveLetter {} -Force",
                    d.to_ascii_uppercase()
                ),
                _ => return None,
            }
        }
    })
}

/// Why a matched directory gets no hint: something in it a delete command could mishandle.
fn unsafe_to_suggest(idx: &Index, i: u32, os: Os) -> bool {
    let mut cur = i;
    loop {
        if idx.flags(cur) & FLAG_LOSSY != 0 {
            // The stored name is not the real one, so a command would name the wrong path.
            return true;
        }
        if cur == 0 {
            break;
        }
        cur = idx.parent(cur);
    }
    (i..idx.subtree_end(i)).any(|j| {
        // Not entered (other filesystem), or synced: a delete would reach beyond what was seen.
        idx.flags(j) & (FLAG_CLOUD | FLAG_MOUNT) != 0
            || match idx.kind(j) {
                Kind::File | Kind::Dir => false,
                // `rm` removes a symlink itself; Windows PowerShell 5.1 may recurse into links.
                Kind::Symlink => os == Os::Windows,
                Kind::Junction | Kind::Reparse => true,
            }
    })
}

fn has_hard_links(idx: &Index, i: u32) -> bool {
    !idx.hard_links.is_empty() && (i..idx.subtree_end(i)).any(|j| idx.hard_links(j).is_some())
}

/// Cleanup hints under `root`, largest first. Text only: dredge never runs the commands.
/// `own_data` is dredge's own data directory, when it lies inside this snapshot: it gets the
/// `dredge_snapshots` hint, which points at `dredge forget` rather than any delete command.
pub fn insights(
    idx: &Index,
    classes: &Classes,
    root: u32,
    limit: usize,
    os: Os,
    own_data: Option<u32>,
) -> (Vec<InsightRecord>, InsightsSummary) {
    let hints: Vec<&Hint> = HINTS.iter().filter(|h| h.os.applies(os)).collect();
    let mut w = Walker::new(idx, root);
    let mut rows = Vec::new();
    let end = idx.subtree_end(root);
    let mut i = root;
    while i < end {
        w.advance(i, |_| {});
        if Some(i) == own_data {
            let bytes = idx.allocated(i);
            if bytes >= MIN_INSIGHT_BYTES {
                rows.push(InsightRecord {
                    id: "dredge_snapshots",
                    category: classes.category(i).unwrap_or(Other),
                    path: idx.path(i),
                    allocated_bytes: bytes,
                    human: human_bytes(bytes),
                    message: "dredge's own snapshots. `dredge snapshots` lists them; `dredge forget --prune` removes the ones no query uses, and `dredge forget <root>` removes one.".to_string(),
                    suggested_command: "dredge forget --prune".to_string(),
                });
            }
            i = idx.subtree_end(i);
            continue;
        }
        if let Some(h) = hints.iter().find(|h| w.hint_applies(h, i)) {
            let bytes = idx.allocated(i);
            if bytes >= MIN_INSIGHT_BYTES && !unsafe_to_suggest(idx, i, os) {
                let path = idx.path(i);
                if let Some(command) = suggested_command(h.action, &path, os) {
                    let mut message = h.message.to_string();
                    if has_hard_links(idx, i) {
                        message.push_str(
                            " Some files are hard-linked elsewhere, so less space may be freed.",
                        );
                    }
                    rows.push(InsightRecord {
                        id: h.id,
                        category: classes.category(i).unwrap_or(Other),
                        path,
                        allocated_bytes: bytes,
                        human: human_bytes(bytes),
                        message,
                        suggested_command: command,
                    });
                }
            }
            // Nothing nested in a hinted directory gets a second hint.
            i = idx.subtree_end(i);
            continue;
        }
        w.descend(i);
        i += 1;
    }
    rows.sort_by(|a, b| {
        b.allocated_bytes
            .cmp(&a.allocated_bytes)
            .then(a.path.cmp(&b.path))
    });
    let count = rows.len();
    rows.truncate(limit);
    (
        rows,
        InsightsSummary {
            path: idx.path(root),
            insights: count,
            shown: count.min(limit),
            truncated: count > limit,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{IndexBuilder, MetaInit};
    use crate::model::{RawEntry, FLAG_CLOUD, FLAG_MOUNT};

    const MIB: u64 = 1024 * 1024;

    fn build(root: &str, ci: bool, f: impl FnOnce(&mut IndexBuilder)) -> Index {
        let mut b = IndexBuilder::new(root);
        f(&mut b);
        b.finish(MetaInit {
            scanner: "test".into(),
            volume_id: None,
            case_insensitive: ci,
            elapsed_ms: 0,
        })
        .unwrap()
    }

    /// Node at `rel` (slash-separated names) below the snapshot root.
    fn node(idx: &Index, rel: &str) -> u32 {
        let mut cur = 0;
        for name in rel.split('/').filter(|s| !s.is_empty()) {
            cur = idx
                .children(cur)
                .find(|&c| idx.name(c) == name)
                .unwrap_or_else(|| panic!("no {name} in {rel}"));
        }
        cur
    }

    fn dirs(b: &mut IndexBuilder, parent: u32, rel: &str) -> u32 {
        rel.split('/')
            .fold(parent, |p, name| b.add(p, RawEntry::dir(name)))
    }

    fn with_flags(mut e: RawEntry, flags: u16) -> RawEntry {
        e.flags = flags;
        e
    }

    fn cats(idx: &Index, os: Os, at: u32) -> Vec<CategoryRecord> {
        categories(idx, &classify(idx, os), at, 100).0
    }

    fn row(rows: &[CategoryRecord], c: Category) -> &CategoryRecord {
        rows.iter().find(|r| r.category == c).unwrap()
    }

    fn hints(idx: &Index, os: Os) -> Vec<InsightRecord> {
        insights(idx, &classify(idx, os), 0, 100, os, None).0
    }

    fn sample() -> Index {
        build("/home/u/proj", false, |b| {
            let nm = b.add(0, RawEntry::dir("node_modules"));
            b.add(nm, RawEntry::file("x.jpg", 150 * MIB));
            let inner = dirs(b, nm, "pkg/node_modules");
            b.add(inner, RawEntry::file("y", MIB));
            let pics = b.add(0, RawEntry::dir("pics"));
            b.add(pics, RawEntry::file("a.jpg", 10));
            b.add(pics, RawEntry::file("b.PNG", 20));
            b.add(pics, RawEntry::file("empty.txt", 0));
            let sub = b.add(pics, RawEntry::dir("sub"));
            b.add(sub, RawEntry::file("c.jpg", 5));
            let mixed = b.add(0, RawEntry::dir("mixed"));
            b.add(mixed, RawEntry::file("a.mp3", 7));
            b.add(mixed, RawEntry::file("b.pdf", 9));
            b.add(0, RawEntry::dir("emptydir"));
            let denied = b.add(0, RawEntry::dir("locked"));
            b.mark_denied(denied);
            b.add(0, RawEntry::file("notes", 3));
        })
    }

    #[test]
    fn rows_sum_to_the_root_total_and_subtrees_are_claimed() {
        let idx = sample();
        let rows = cats(&idx, Os::Linux, 0);
        assert_eq!(
            rows.iter().map(|r| r.allocated_bytes).sum::<u64>(),
            idx.allocated(0)
        );
        // The path rule beats the extension rule, and the nested node_modules is not re-matched.
        let dev = row(&rows, Developer);
        assert_eq!(
            dev.allocated_bytes,
            idx.allocated(node(&idx, "node_modules"))
        );
        assert_eq!(dev.files, 2);
        assert_eq!(dev.top, vec![idx.path(node(&idx, "node_modules"))]);
        let classes = classify(&idx, Os::Linux);
        assert_eq!(
            classes.category(node(&idx, "node_modules/x.jpg")),
            Some(Developer)
        );
        // A directory that is all photos (a zero-byte text file does not count) is one entry.
        let photos = row(&rows, Photos);
        assert_eq!(photos.allocated_bytes, 35);
        assert_eq!(photos.top, vec![idx.path(node(&idx, "pics"))]);
        assert_eq!(classes.category(node(&idx, "pics")), Some(Photos));
        assert_eq!(classes.category(node(&idx, "mixed")), None);
        assert_eq!(classes.category(node(&idx, "locked")), Some(Other));
        assert_eq!(row(&rows, Music).allocated_bytes, 7);
        assert_eq!(row(&rows, Documents).allocated_bytes, 9);
        assert_eq!(row(&rows, Other).allocated_bytes, 3);
        assert!(rows
            .windows(2)
            .all(|w| w[0].allocated_bytes >= w[1].allocated_bytes));
    }

    #[test]
    fn a_subpath_inside_a_claimed_directory_is_one_category() {
        let idx = sample();
        let at = node(&idx, "node_modules/pkg");
        let rows = cats(&idx, Os::Linux, at);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, Developer);
        assert_eq!(rows[0].allocated_bytes, idx.allocated(at));
        assert_eq!(rows[0].share, 100.0);
    }

    #[test]
    fn a_file_or_an_empty_directory_as_the_scope() {
        let idx = sample();
        let file = node(&idx, "pics/a.jpg");
        let rows = cats(&idx, Os::Linux, file);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, Photos);
        assert_eq!(rows[0].top, vec![idx.path(file)]);
        let (rows, sum) = categories(&idx, &classify(&idx, Os::Linux), node(&idx, "emptydir"), 10);
        assert!(rows.is_empty());
        assert_eq!(sum.allocated_bytes, 0);
        assert_eq!(sum.categories, 0);

        let empty = build("/home/u/empty", false, |_| {});
        let (rows, sum) = categories(&empty, &classify(&empty, Os::Linux), 0, 10);
        assert!(rows.is_empty());
        assert_eq!(sum.allocated_bytes, 0);
    }

    #[test]
    fn top_is_the_three_largest_disjoint_subtrees() {
        let idx = build("/home/u", false, |b| {
            let p1 = b.add(0, RawEntry::dir("p1"));
            b.add(p1, RawEntry::file("a.jpg", 40));
            b.add(p1, RawEntry::file("x", 1));
            b.add(0, RawEntry::file("p2.jpg", 30));
            let p3 = b.add(0, RawEntry::dir("p3"));
            b.add(p3, RawEntry::file("b.jpg", 20));
            b.add(p3, RawEntry::file("c.jpg", 10));
            b.add(0, RawEntry::file("p4.jpg", 5));
        });
        let rows = cats(&idx, Os::Linux, 0);
        let photos = row(&rows, Photos);
        assert_eq!(photos.allocated_bytes, 105);
        assert_eq!(photos.files, 5);
        let want: Vec<String> = ["p1/a.jpg", "p2.jpg", "p3"]
            .iter()
            .map(|p| idx.path(node(&idx, p)))
            .collect();
        assert_eq!(photos.top, want);
    }

    #[test]
    fn hard_link_aliases_do_not_make_a_directory_mixed() {
        let linked = |name: &str| {
            let mut e = RawEntry::file(name, 100);
            e.file_id = Some(7);
            e.nlink = Some(2);
            e
        };
        let idx = build("/home/u", false, |b| {
            let owner = b.add(0, RawEntry::dir("a_owner"));
            b.add(owner, linked("orig.jpg"));
            let pics = b.add(0, RawEntry::dir("pics"));
            b.add(pics, RawEntry::file("a.jpg", 50));
            b.add(pics, linked("zz.dat"));
        });
        let alias = node(&idx, "pics/zz.dat");
        assert_eq!(idx.allocated(alias), 0);
        let classes = classify(&idx, Os::Linux);
        assert_eq!(classes.category(node(&idx, "pics")), Some(Photos));
        let rows = cats(&idx, Os::Linux, 0);
        assert_eq!(
            rows.iter().map(|r| r.allocated_bytes).sum::<u64>(),
            idx.allocated(0)
        );
    }

    #[test]
    fn os_rules_match_only_their_anchored_location() {
        // macOS: `Library/Caches` is a cache, a bare `Caches` is not.
        let mac = build("/Users/u", true, |b| {
            let c = dirs(b, 0, "Library/Caches");
            b.add(c, RawEntry::file("x", 10));
            let c = dirs(b, 0, "proj/Caches");
            b.add(c, RawEntry::file("x", 10));
            let t = b.add(0, RawEntry::dir(".Trash"));
            b.add(t, RawEntry::file("old.jpg", 10));
            let t = dirs(b, 0, "work/sub/.Trash");
            b.add(t, RawEntry::file("y", 10));
        });
        let c = classify(&mac, Os::Macos);
        assert_eq!(c.category(node(&mac, "Library/Caches")), Some(Caches));
        assert_eq!(c.category(node(&mac, "proj/Caches")), Some(Other));
        assert_eq!(c.category(node(&mac, ".Trash/old.jpg")), Some(Trash));
        assert_eq!(c.category(node(&mac, "work/sub/.Trash")), Some(Other));
        // Rules for another OS never apply.
        let c = classify(&mac, Os::Linux);
        assert_eq!(c.category(node(&mac, "Library/Caches")), Some(Other));

        // Linux: only a home directory's `.cache`.
        let linux = build("/home/u", false, |b| {
            let c = b.add(0, RawEntry::dir(".cache"));
            b.add(c, RawEntry::file("x", 10));
            let c = dirs(b, 0, "proj/.cache");
            b.add(c, RawEntry::file("x", 10));
            let t = dirs(b, 0, ".local/share/Trash");
            b.add(t, RawEntry::file("x", 10));
        });
        let c = classify(&linux, Os::Linux);
        assert_eq!(c.category(node(&linux, ".cache")), Some(Caches));
        assert_eq!(c.category(node(&linux, "proj/.cache")), Some(Other));
        assert_eq!(c.category(node(&linux, ".local/share/Trash")), Some(Trash));
        let elsewhere = build("/srv/data", false, |b| {
            let c = b.add(0, RawEntry::dir(".cache"));
            b.add(c, RawEntry::file("x", 10));
        });
        let c = classify(&elsewhere, Os::Linux);
        assert_eq!(c.category(node(&elsewhere, ".cache")), Some(Other));

        // Windows: only %LOCALAPPDATA%\Temp, and system folders only at the drive root.
        let win = build("C:", true, |b| {
            let home = dirs(b, 0, "Users/u");
            let t = dirs(b, home, "AppData/Local/Temp");
            b.add(t, RawEntry::file("x", 10));
            let t = dirs(b, home, "proj/Temp");
            b.add(t, RawEntry::file("x", 10));
            let w = b.add(0, RawEntry::dir("Windows"));
            b.add(w, RawEntry::file("x", 10));
            let w = b.add(home, RawEntry::dir("Windows"));
            b.add(w, RawEntry::file("x", 10));
            b.add(0, RawEntry::file("pagefile.sys", 10));
        });
        let c = classify(&win, Os::Windows);
        let at = |p| c.category(node(&win, p));
        assert_eq!(at("Users/u/AppData/Local/Temp"), Some(Caches));
        assert_eq!(at("Users/u/proj/Temp"), Some(Other));
        assert_eq!(at("Windows"), Some(System));
        assert_eq!(at("Users/u/Windows"), Some(Other));
        assert_eq!(at("pagefile.sys"), Some(System));
    }

    #[test]
    fn the_snapshot_root_itself_can_match() {
        let idx = build("/Users/u/Library/Caches", true, |b| {
            b.add(0, RawEntry::file("x.jpg", 10));
        });
        let rows = cats(&idx, Os::Macos, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, Caches);
    }

    #[test]
    fn extension_rules_come_last_and_bundles_claim_their_subtree() {
        let first_ext = RULES
            .iter()
            .position(|r| matches!(r.matcher, Extension(_) | Bundle(_)))
            .unwrap();
        assert!(RULES[first_ext..]
            .iter()
            .all(|r| matches!(r.matcher, Extension(_) | Bundle(_))));
        let idx = build("/Users/u", true, |b| {
            let app = dirs(b, 0, "Tools/Foo.app/Contents");
            b.add(app, RawEntry::file("icon.png", 10));
            let nm = dirs(b, 0, "proj/node_modules/x.app");
            b.add(nm, RawEntry::file("y", 10));
        });
        let c = classify(&idx, Os::Macos);
        assert_eq!(
            c.category(node(&idx, "Tools/Foo.app/Contents/icon.png")),
            Some(Apps)
        );
        assert_eq!(
            c.category(node(&idx, "proj/node_modules/x.app")),
            Some(Developer)
        );
    }

    #[test]
    fn extension_rules_skip_directories_except_bundles() {
        let idx = build("/Users/u", true, |b| {
            let d = dirs(b, 0, "Library/Containers/com.apple.iWork.Pages");
            b.add(d, RawEntry::file("blob", 10));
            let d = dirs(b, 0, "stuff/notes.pdf");
            b.add(d, RawEntry::file("blob", 10));
            b.add(0, RawEntry::file("tls.key", 10));
            b.add(0, RawEntry::file("doc.pdf", 10));
            let d = dirs(b, 0, "Pictures/Photos Library.photoslibrary");
            b.add(d, RawEntry::file("blob", 10));
        });
        let c = classify(&idx, Os::Macos);
        let cat = |p: &str| c.category(node(&idx, p));
        assert_ne!(
            cat("Library/Containers/com.apple.iWork.Pages"),
            Some(Documents)
        );
        assert_ne!(cat("stuff/notes.pdf"), Some(Documents));
        assert_eq!(cat("tls.key"), Some(Other));
        assert_eq!(cat("doc.pdf"), Some(Documents));
        assert_eq!(cat("Pictures/Photos Library.photoslibrary"), Some(Photos));
    }

    #[test]
    fn target_is_developer_only_beside_cargo_toml() {
        let idx = build("/Users/u", true, |b| {
            let p = dirs(b, 0, "rs");
            b.add(p, RawEntry::file("Cargo.toml", 10));
            let t = dirs(b, p, "target");
            b.add(t, RawEntry::file("x", 10));
            let q = dirs(b, 0, "misc/target");
            b.add(q, RawEntry::file("y", 10));
        });
        let c = classify(&idx, Os::Macos);
        assert_eq!(c.category(node(&idx, "rs/target")), Some(Developer));
        assert_ne!(c.category(node(&idx, "misc/target")), Some(Developer));
    }

    #[test]
    fn every_rule_and_hint_has_a_source_link() {
        assert!(RULES.iter().all(|r| r.source.starts_with("https://")));
        assert!(HINTS.iter().all(|h| h.source.starts_with("https://")));
        assert!(HINTS.iter().all(|h| !h.message.is_empty()));
    }

    fn project(b: &mut IndexBuilder, at: &str, bytes: u64) -> u32 {
        let p = dirs(b, 0, at);
        b.add(p, RawEntry::file("package-lock.json", 10));
        let nm = b.add(p, RawEntry::dir("node_modules"));
        b.add(nm, RawEntry::file("blob", bytes));
        nm
    }

    #[test]
    fn node_modules_hint_needs_a_lockfile_and_a_project() {
        let idx = build("/home/u", false, |b| {
            let big = project(b, "big", 300 * MIB);
            project(b, "small", MIB);
            let nm = dirs(b, 0, "nolock/node_modules");
            b.add(nm, RawEntry::file("blob", 300 * MIB));
            project(b, ".vscode/extensions/ext", 300 * MIB);
            project(b, "Tools/Foo.app/Contents/Resources/app", 300 * MIB);
            // Nested inside a hinted node_modules: no second hint.
            let inner = b.add(big, RawEntry::dir("inner"));
            b.add(inner, RawEntry::file("yarn.lock", 1));
            let nm = b.add(inner, RawEntry::dir("node_modules"));
            b.add(nm, RawEntry::file("blob", 200 * MIB));
        });
        let rows = hints(&idx, Os::Linux);
        assert_eq!(
            rows.len(),
            1,
            "{:?}",
            rows.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        let r = &rows[0];
        assert_eq!(r.id, "node_modules");
        assert_eq!(r.category, Developer);
        assert_eq!(r.path, idx.path(node(&idx, "big/node_modules")));
        assert_eq!(
            r.allocated_bytes,
            idx.allocated(node(&idx, "big/node_modules"))
        );
        assert!(r.allocated_bytes >= 500 * MIB);
        assert_eq!(r.suggested_command, format!("rm -rf -- '{}'", r.path));
    }

    #[test]
    fn hints_skip_links_mounts_cloud_files_and_lossy_names() {
        let idx = build("/home/u", false, |b| {
            let nm = project(b, "cloud", 300 * MIB);
            b.add(nm, with_flags(RawEntry::file("p", 0), FLAG_CLOUD));
            let nm = project(b, "mount", 300 * MIB);
            b.add(nm, with_flags(RawEntry::dir("m"), FLAG_MOUNT));
            let nm = project(b, "junction", 300 * MIB);
            b.add(nm, RawEntry::new("j", Kind::Junction, 0, 0));
            let nm = project(b, "link", 300 * MIB);
            b.add(nm, RawEntry::new("s", Kind::Symlink, 0, 0));
            let lossy = b.add(0, with_flags(RawEntry::dir("bad\u{fffd}"), FLAG_LOSSY));
            let p = b.add(lossy, RawEntry::dir("p"));
            b.add(p, RawEntry::file("yarn.lock", 1));
            let nm = b.add(p, RawEntry::dir("node_modules"));
            b.add(nm, RawEntry::file("blob", 300 * MIB));
        });
        let paths = |os| -> Vec<String> { hints(&idx, os).into_iter().map(|r| r.path).collect() };
        // `rm` removes a symlink itself, so only the symlink case is left on Unix.
        assert_eq!(
            paths(Os::Linux),
            vec![idx.path(node(&idx, "link/node_modules"))]
        );
        assert!(paths(Os::Windows).is_empty());
    }

    #[test]
    fn own_data_dir_gets_its_own_hint_and_nothing_nested() {
        let idx = build("/home/u", false, |b| {
            let d = dirs(b, 0, ".local/share/dredge");
            b.add(d, RawEntry::file("home-u-0123.dredge", 150 * MIB));
            // Never a second hint inside dredge's own directory.
            let x = b.add(d, RawEntry::dir("x"));
            b.add(x, RawEntry::file("package-lock.json", 1));
            let nm = b.add(x, RawEntry::dir("node_modules"));
            b.add(nm, RawEntry::file("blob", 300 * MIB));
            let small = dirs(b, 0, "elsewhere/dredge");
            b.add(small, RawEntry::file("s.dredge", MIB));
        });
        let own = idx.resolve("/home/u/.local/share/dredge");
        let classes = classify(&idx, Os::Linux);
        let (rows, _) = insights(&idx, &classes, 0, 100, Os::Linux, own);
        assert_eq!(
            rows.len(),
            1,
            "{:?}",
            rows.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        assert_eq!(rows[0].id, "dredge_snapshots");
        assert_eq!(rows[0].suggested_command, "dredge forget --prune");
        assert!(!rows[0].suggested_command.contains("rm "));
        // Below the usual minimum, no hint at all.
        let small = idx.resolve("/home/u/elsewhere/dredge");
        assert!(insights(&idx, &classes, 0, 100, Os::Linux, small)
            .0
            .iter()
            .all(|r| r.id != "dredge_snapshots"));
    }

    #[test]
    fn hints_are_sorted_limited_and_note_hard_links() {
        let idx = build("/Users/u", true, |b| {
            let c = dirs(b, 0, "Library/Caches");
            b.add(c, RawEntry::file("x", 500 * MIB));
            let d = dirs(b, 0, "Library/Developer/Xcode/DerivedData");
            b.add(d, RawEntry::file("x", 200 * MIB));
            let t = b.add(0, RawEntry::dir(".Trash"));
            b.add(t, RawEntry::file("old", 150 * MIB));
            let nm = project(b, "proj", 120 * MIB);
            let mut e = RawEntry::file("linked", 10);
            e.file_id = Some(1);
            e.nlink = Some(2);
            b.add(nm, e.clone());
            let other = b.add(0, RawEntry::dir("zz"));
            b.add(other, e);
        });
        let (rows, sum) = insights(&idx, &classify(&idx, Os::Macos), 0, 2, Os::Macos, None);
        assert_eq!(sum.insights, 4);
        assert_eq!(sum.shown, 2);
        assert!(sum.truncated);
        assert_eq!(rows[0].id, "user_caches");
        assert_eq!(rows[0].category, Caches);
        assert_eq!(
            rows[0].suggested_command,
            format!("find '{}' -xdev -mindepth 1 -delete", rows[0].path)
        );
        assert_eq!(rows[1].id, "xcode_derived_data");
        let all = hints(&idx, Os::Macos);
        let trash = all.iter().find(|r| r.id == "trash").unwrap();
        assert_eq!(trash.category, Trash);
        assert!(trash.message.contains("permanently"));
        let nm = all.iter().find(|r| r.id == "node_modules").unwrap();
        assert!(nm.message.contains("hard-linked"));
        // A Library/Caches that is not a user's own gets no hint.
        let nested = build("/Users/u/proj", true, |b| {
            let c = dirs(b, 0, "Library/Caches");
            b.add(c, RawEntry::file("x", 500 * MIB));
        });
        assert!(hints(&nested, Os::Macos).is_empty());
    }

    #[test]
    fn windows_hints_use_powershell_and_the_recycle_bin_cmdlet() {
        let idx = build("C:", true, |b| {
            let r = b.add(0, RawEntry::dir("$Recycle.Bin"));
            b.add(r, RawEntry::file("x", 300 * MIB));
            project(b, "Users/u/it's ‘q’", 200 * MIB);
        });
        let rows = hints(&idx, Os::Windows);
        assert_eq!(rows[0].id, "trash");
        assert_eq!(
            rows[0].suggested_command,
            "Clear-RecycleBin -DriveLetter C -Force"
        );
        assert_eq!(rows[1].id, "node_modules");
        assert!(rows[1]
            .suggested_command
            .starts_with("Remove-Item -LiteralPath '"));
        assert!(rows[1].suggested_command.contains("it''s ‘‘q’’"));
        assert!(rows[1]
            .suggested_command
            .ends_with("node_modules' -Recurse -Force"));
    }

    #[test]
    fn quoting_handles_spaces_and_quotes() {
        assert_eq!(quote_posix("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(quote_powershell(r"C:\a b\it's"), r"'C:\a b\it''s'");
        assert_eq!(quote_powershell("x’y"), "'x’’y'");
        assert_eq!(
            suggested_command(Action::RemoveDir, "/a b/it's", Os::Linux).unwrap(),
            r"rm -rf -- '/a b/it'\''s'"
        );
        assert_eq!(
            suggested_command(Action::RemoveContents, r"C:\a b\it's", Os::Windows).unwrap(),
            r"Get-ChildItem -LiteralPath 'C:\a b\it''s' -Force | Remove-Item -Recurse -Force"
        );
        assert_eq!(
            suggested_command(
                Action::ClearRecycleBin,
                r"\\srv\share\$Recycle.Bin",
                Os::Windows
            ),
            None
        );
    }

    /// The quoted text survives a real POSIX shell unchanged. Only `printf` runs; nothing is
    /// read or written.
    #[cfg(unix)]
    #[test]
    fn posix_quoting_round_trips_through_sh() {
        for s in [
            "/a b/it's",
            "/x'; echo pwned '",
            "/$(id) `id` \"q\"\n$HOME",
            "/'''",
        ] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote_posix(s)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), s);
        }
    }
}
