//! Mounted volumes, via `sysinfo` so no platform-specific code is needed here.

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
