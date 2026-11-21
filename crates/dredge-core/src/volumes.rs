//! Mounted volumes, via `sysinfo` so no platform-specific code is needed here.

use crate::util::{components, covers, strip_verbatim};

pub struct VolumeInfo {
    pub mount: String,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
}

/// Volumes with a nonzero size, sorted by mount point.
pub fn list() -> Vec<VolumeInfo> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<VolumeInfo> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| VolumeInfo {
            mount: d.mount_point().to_string_lossy().into_owned(),
            label: d.name().to_string_lossy().into_owned(),
            fs: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            free: d.available_space(),
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

/// The volume that holds `path`: the one with the deepest mount point covering it. `path` may be
/// a Windows verbatim path (what `fs::canonicalize` returns there); the prefix is ignored.
/// Compared by whole components, so `/mnt/data2` is not on `/mnt/data`.
pub fn containing(vols: Vec<VolumeInfo>, path: &str, case_insensitive: bool) -> Option<VolumeInfo> {
    let path = strip_verbatim(path);
    vols.into_iter()
        .filter(|v| covers(&v.mount, &path, case_insensitive))
        .max_by_key(|v| components(&v.mount).len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(mount: &str) -> VolumeInfo {
        VolumeInfo {
            mount: mount.into(),
            label: String::new(),
            fs: String::new(),
            total: 1,
            free: 1,
        }
    }

    #[test]
    fn deepest_mount_wins_by_whole_components() {
        let vols = vec![vol("/"), vol("/mnt/data"), vol("/mnt/data2")];
        assert_eq!(
            containing(vols, "/mnt/data2/x/y", false).unwrap().mount,
            "/mnt/data2"
        );
        let vols = vec![vol("/"), vol("/mnt/data")];
        assert_eq!(containing(vols, "/mnt/data2/x", false).unwrap().mount, "/");
    }

    #[test]
    fn no_covering_mount_is_none() {
        assert!(containing(vec![vol("/mnt/data")], "/home/x", false).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_paths_match_their_drive() {
        let vols = vec![vol(r"C:\"), vol(r"D:\")];
        let got = containing(vols, r"\\?\C:\Users\a\AppData\Local\dredge", true).unwrap();
        assert_eq!(got.mount, r"C:\");
        let got = containing(vec![vol(r"C:\")], r"\\?\c:\users\a", true).unwrap();
        assert_eq!(got.mount, r"C:\");
    }
}
