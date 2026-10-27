//! Versioned binary snapshot format:
//!
//! ```text
//! magic    8 B   "DREDGE\0\x01"
//! version  u32   format version (little endian)
//! hdr_len  u32
//! header   JSON (UTF-8, uncompressed) = IndexMeta, so listing never decompresses the body
//! body     one zstd frame with content checksum: name arena, then each column, then hard links
//! ```
//! Columns are raw little-endian arrays, each prefixed with a u64 element count.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::error::Error;
use crate::model::{Index, IndexMeta};

const MAGIC: &[u8; 8] = b"DREDGE\0\x01";
pub const FORMAT_VERSION: u32 = 1;
const MAX_HEADER_BYTES: u32 = 16 * 1024 * 1024;
const CHUNK: usize = 1 << 14;
const ZSTD_LEVEL: i32 = 3;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

macro_rules! columns {
    ($write:ident, $read:ident, $t:ty, $size:expr) => {
        fn $write<W: Write>(w: &mut W, v: &[$t]) -> io::Result<()> {
            w.write_all(&(v.len() as u64).to_le_bytes())?;
            let mut buf = Vec::with_capacity(CHUNK * $size);
            for chunk in v.chunks(CHUNK) {
                buf.clear();
                for x in chunk {
                    buf.extend_from_slice(&x.to_le_bytes());
                }
                w.write_all(&buf)?;
            }
            Ok(())
        }

        /// `expect` pins the element count (all node columns must match the header).
        fn $read<R: Read>(r: &mut R, expect: Option<usize>, max: usize) -> io::Result<Vec<$t>> {
            let n = read_len(r)?;
            if n > max || expect.is_some_and(|e| e != n) {
                return Err(bad("column length mismatch"));
            }
            let mut out = Vec::with_capacity(n.min(1 << 24));
            let mut buf = vec![0u8; CHUNK * $size];
            let mut left = n;
            while left > 0 {
                let k = left.min(CHUNK);
                r.read_exact(&mut buf[..k * $size])?;
                for c in buf[..k * $size].chunks_exact($size) {
                    out.push(<$t>::from_le_bytes(c.try_into().map_err(|_| bad("short column"))?));
                }
                left -= k;
            }
            Ok(out)
        }
    };
}

columns!(write_u16s, read_u16s, u16, 2);
columns!(write_u32s, read_u32s, u32, 4);
columns!(write_u64s, read_u64s, u64, 8);

fn read_len<R: Read>(r: &mut R) -> io::Result<usize> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    usize::try_from(u64::from_le_bytes(b)).map_err(|_| bad("length overflow"))
}

fn write_bytes<W: Write>(w: &mut W, v: &[u8]) -> io::Result<()> {
    w.write_all(&(v.len() as u64).to_le_bytes())?;
    w.write_all(v)
}

fn read_bytes<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let n = read_len(r)?;
    if n >= u32::MAX as usize {
        return Err(bad("name arena too large"));
    }
    let mut out = Vec::new();
    r.take(n as u64).read_to_end(&mut out)?;
    if out.len() != n {
        return Err(bad("truncated name arena"));
    }
    Ok(out)
}

/// Write atomically (temp file + rename). Returns the size of the snapshot file in bytes.
/// The temp name carries the pid so concurrent processes scanning one root cannot collide.
pub fn write(path: &Path, index: &Index) -> Result<u64, Error> {
    let tmp = path.with_extension(format!("dredge.{}.tmp", std::process::id()));
    let ctx = |e: io::Error| Error::io(format!("writing snapshot {}", path.display()), e);
    write_to(&tmp, index).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        ctx(e)
    })?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        ctx(e)
    })?;
    fs::metadata(path).map(|m| m.len()).map_err(ctx)
}

fn write_to(tmp: &Path, index: &Index) -> io::Result<()> {
    let mut w = BufWriter::new(File::create(tmp)?);
    w.write_all(MAGIC)?;
    w.write_all(&FORMAT_VERSION.to_le_bytes())?;
    let header = serde_json::to_vec(&index.meta).map_err(io::Error::other)?;
    w.write_all(&(header.len() as u32).to_le_bytes())?;
    w.write_all(&header)?;

    let mut enc = zstd::stream::Encoder::new(w, ZSTD_LEVEL)?;
    enc.include_checksum(true)?;
    write_bytes(&mut enc, &index.names)?;
    write_u32s(&mut enc, &index.parent)?;
    write_u32s(&mut enc, &index.name_off)?;
    write_u16s(&mut enc, &index.name_len)?;
    write_u16s(&mut enc, &index.flags)?;
    write_u64s(&mut enc, &index.allocated)?;
    write_u64s(&mut enc, &index.logical)?;
    write_u32s(&mut enc, &index.mtime)?;
    write_u32s(&mut enc, &index.subtree_end)?;
    write_u32s(&mut enc, &index.files)?;
    let mut links: Vec<(u32, u32)> = index.hard_links.iter().map(|(k, v)| (*k, *v)).collect();
    links.sort_unstable();
    let flat: Vec<u32> = links.into_iter().flat_map(|(a, b)| [a, b]).collect();
    write_u32s(&mut enc, &flat)?;
    let w = enc.finish()?;
    let f = w.into_inner().map_err(|e| e.into_error())?;
    f.sync_all()
}

fn read_prelude<R: Read>(r: &mut R, path: &Path) -> Result<IndexMeta, Error> {
    let corrupt = |reason: &str| Error::SnapshotCorrupt {
        path: path.display().to_string(),
        reason: reason.to_string(),
    };
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).map_err(|_| corrupt("file too short"))?;
    if &magic != MAGIC {
        return Err(corrupt("not a dredge snapshot"));
    }
    let mut v = [0u8; 4];
    r.read_exact(&mut v).map_err(|_| corrupt("file too short"))?;
    let version = u32::from_le_bytes(v);
    if version != FORMAT_VERSION {
        return Err(Error::SnapshotVersion {
            path: path.display().to_string(),
            found: version,
        });
    }
    let mut l = [0u8; 4];
    r.read_exact(&mut l).map_err(|_| corrupt("file too short"))?;
    let len = u32::from_le_bytes(l);
    if len > MAX_HEADER_BYTES {
        return Err(corrupt("header too large"));
    }
    let mut hdr = vec![0u8; len as usize];
    r.read_exact(&mut hdr).map_err(|_| corrupt("truncated header"))?;
    serde_json::from_slice(&hdr).map_err(|e| corrupt(&format!("bad header: {e}")))
}

fn open(path: &Path) -> Result<BufReader<File>, Error> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|e| Error::io(format!("opening snapshot {}", path.display()), e))
}

/// Read only the header; does not touch the compressed body.
pub fn read_header(path: &Path) -> Result<IndexMeta, Error> {
    read_prelude(&mut open(path)?, path)
}

pub fn read(path: &Path) -> Result<Index, Error> {
    let mut r = open(path)?;
    let meta = read_prelude(&mut r, path)?;
    let corrupt = |reason: String| Error::SnapshotCorrupt {
        path: path.display().to_string(),
        reason,
    };
    let n = usize::try_from(meta.node_count).map_err(|_| corrupt("node count overflow".into()))?;
    if n == 0 || n >= u32::MAX as usize {
        return Err(corrupt("bad node count".into()));
    }
    let body = |r: BufReader<File>| -> io::Result<Index> {
        let mut dec = zstd::stream::Decoder::new(r)?;
        let names = read_bytes(&mut dec)?;
        let parent = read_u32s(&mut dec, Some(n), n)?;
        let name_off = read_u32s(&mut dec, Some(n), n)?;
        let name_len = read_u16s(&mut dec, Some(n), n)?;
        let flags = read_u16s(&mut dec, Some(n), n)?;
        let allocated = read_u64s(&mut dec, Some(n), n)?;
        let logical = read_u64s(&mut dec, Some(n), n)?;
        let mtime = read_u32s(&mut dec, Some(n), n)?;
        let subtree_end = read_u32s(&mut dec, Some(n), n)?;
        let files = read_u32s(&mut dec, Some(n), n)?;
        let flat = read_u32s(&mut dec, None, n.saturating_mul(2))?;
        // Drain to the end of the frame so the zstd content checksum is verified.
        let mut rest = Vec::new();
        dec.read_to_end(&mut rest)?;
        if !rest.is_empty() {
            return Err(bad("trailing data"));
        }
        if flat.len() % 2 != 0 {
            return Err(bad("odd hard link table"));
        }
        let hard_links: HashMap<u32, u32> = flat.chunks_exact(2).map(|c| (c[0], c[1])).collect();
        Ok(Index {
            meta,
            names,
            parent,
            name_off,
            name_len,
            flags,
            allocated,
            logical,
            mtime,
            subtree_end,
            files,
            hard_links,
        })
    };
    let index = body(r).map_err(|e| corrupt(e.to_string()))?;
    index.validate().map_err(corrupt)?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{IndexBuilder, MetaInit};
    use crate::model::RawEntry;

    fn build() -> Index {
        let mut b = IndexBuilder::new("/r");
        let d = b.add(0, RawEntry::dir("docs"));
        b.add(d, RawEntry::file("ünïcode.txt", 4096));
        let mut h1 = RawEntry::file("h1", 50);
        h1.file_id = Some(9);
        h1.nlink = Some(2);
        let mut h2 = h1.clone();
        h2.name = "h2".into();
        b.add(0, h1);
        b.add(0, h2);
        b.add(0, RawEntry::file("big.bin", 1 << 30));
        b.finish(MetaInit {
            scanner: "test".into(),
            volume_id: Some("V1".into()),
            case_insensitive: false,
            elapsed_ms: 5,
        })
        .unwrap()
    }

    #[test]
    fn round_trip_preserves_everything() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.dredge");
        let idx = build();
        let size = write(&p, &idx).unwrap();
        assert!(size > 0);

        let hdr = read_header(&p).unwrap();
        assert_eq!(hdr.root, "/r");
        assert_eq!(hdr.node_count, idx.meta.node_count);

        let back = read(&p).unwrap();
        assert_eq!(back.len(), idx.len());
        for i in 0..idx.len() as u32 {
            assert_eq!(back.name(i), idx.name(i));
            assert_eq!(back.parent(i), idx.parent(i));
            assert_eq!(back.flags(i), idx.flags(i));
            assert_eq!(back.allocated(i), idx.allocated(i));
            assert_eq!(back.logical(i), idx.logical(i));
            assert_eq!(back.subtree_end(i), idx.subtree_end(i));
            assert_eq!(back.files(i), idx.files(i));
            assert_eq!(back.hard_links(i), idx.hard_links(i));
        }
        assert_eq!(back.meta.volume_id.as_deref(), Some("V1"));
        assert_eq!(back.meta.hard_link_groups, 1);
    }

    #[test]
    fn rejects_garbage_and_wrong_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.dredge");
        fs::write(&p, b"not a snapshot at all").unwrap();
        assert!(matches!(read(&p), Err(Error::SnapshotCorrupt { .. })));

        let mut v = Vec::new();
        v.extend_from_slice(MAGIC);
        v.extend_from_slice(&99u32.to_le_bytes());
        fs::write(&p, v).unwrap();
        assert!(matches!(read(&p), Err(Error::SnapshotVersion { found: 99, .. })));
    }

    #[test]
    fn truncated_body_is_corrupt_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.dredge");
        write(&p, &build()).unwrap();
        let bytes = fs::read(&p).unwrap();
        fs::write(&p, &bytes[..bytes.len() - 10]).unwrap();
        assert!(matches!(read(&p), Err(Error::SnapshotCorrupt { .. })));
    }
}
