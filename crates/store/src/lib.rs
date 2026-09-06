//! File materialization of the data directory.
//!
//! The database is the index; the files are the artifact. A write is
//! `tmp` → `fsync` → `rename` so a reader never sees a partial file and a crash leaves
//! either the old version or the new one (`docs/11-file-layout.md`).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use jobseeker_core::hash::{content_hash, shard};
use jobseeker_core::ids::JobId;
use jobseeker_core::slug::slugify_max;
use jobseeker_core::{Error, Result};
use serde::Serialize;

pub mod discover;
pub mod jobfile;

/// Atomically replace `path` with `bytes`.
///
/// The temporary file lives next to the target so `rename` cannot fail by crossing
/// filesystems. The directory is fsynced after the rename so a crash cannot lose the
/// directory entry.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// Serialize `value` as stable, git-friendly JSON: sorted keys, two-space indent, a
/// trailing newline, and no `null` fields. A re-export of unchanged data is then
/// byte-identical, so `git status` stays clean.
pub fn to_stable_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let raw = serde_json::to_value(value).map_err(|e| Error::Serde(e.to_string()))?;
    let cleaned = strip_nulls(raw);
    let mut bytes = serde_json::to_vec_pretty(&cleaned).map_err(|e| Error::Serde(e.to_string()))?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn strip_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let v = strip_nulls(v);
                match &v {
                    serde_json::Value::Null => {}
                    serde_json::Value::Array(a) if a.is_empty() => {}
                    serde_json::Value::Object(o) if o.is_empty() => {}
                    _ => {
                        out.insert(k, v);
                    }
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(strip_nulls).collect())
        }
        other => other,
    }
}

/// Content-addressed blob path: `captures/<hh>/<hash>.<ext>.zst`.
pub fn blob_path(root: &Path, hash: &str, ext: &str) -> PathBuf {
    root.join("captures")
        .join(shard(hash))
        .join(format!("{hash}.{ext}.zst"))
}

/// Compress `bytes` with zstd level 10 and write them atomically to the content-addressed
/// store. Identical captures share one file.
pub fn write_blob(root: &Path, bytes: &[u8], ext: &str) -> Result<(String, PathBuf)> {
    let hash = content_hash(bytes);
    let path = blob_path(root, &hash, ext);
    if !path.exists() {
        let compressed = zstd::encode_all(bytes, 10).map_err(|e| Error::Storage(e.to_string()))?;
        atomic_write(&path, &compressed)?;
    }
    Ok((hash, path))
}

pub fn read_blob(path: &Path) -> Result<Vec<u8>> {
    let compressed = fs::read(path)?;
    zstd::decode_all(compressed.as_slice()).map_err(|e| Error::Storage(e.to_string()))
}

/// `jobs/<company-slug>/<date>-<job-slug>-<id8>/`
pub fn job_dir(
    root: &Path,
    company_slug: &str,
    posted: Option<&str>,
    title: &str,
    id: &JobId,
) -> PathBuf {
    let date = posted.unwrap_or("undated");
    let slug = slugify_max(title, 60);
    root.join("jobs")
        .join(slugify_max(company_slug, 60))
        .join(format!("{date}-{slug}-{}", id.short()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_crash_between_write_and_rename_cannot_leave_a_partial_target() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("job.json");
        atomic_write(&path, b"{\"ok\":true}\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"ok\":true}\n");
        atomic_write(&path, b"{\"ok\":false}\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"ok\":false}\n");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "tmp files must not survive a successful write"
        );
    }

    #[test]
    fn stable_json_omits_nulls_and_empty_collections_and_ends_with_a_newline() {
        #[derive(Serialize)]
        struct Rec {
            title: String,
            notes: Option<String>,
            tags: Vec<String>,
        }
        let bytes = to_stable_json(&Rec {
            title: "Engineer".into(),
            notes: None,
            tags: vec![],
        })
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.ends_with('\n'));
        assert!(!text.contains("null"));
        assert!(!text.contains("tags"));
        assert!(text.contains("\"title\": \"Engineer\""));
    }

    #[test]
    fn identical_blobs_share_one_file() {
        let dir = TempDir::new().unwrap();
        let (a, pa) = write_blob(dir.path(), b"<html>job</html>", "html").unwrap();
        let (b, pb) = write_blob(dir.path(), b"<html>job</html>", "html").unwrap();
        assert_eq!(a, b);
        assert_eq!(pa, pb);
        assert_eq!(read_blob(&pa).unwrap(), b"<html>job</html>");
    }

    #[test]
    fn different_blobs_do_not_collide() {
        let dir = TempDir::new().unwrap();
        let (a, _) = write_blob(dir.path(), b"one", "html").unwrap();
        let (b, _) = write_blob(dir.path(), b"two", "html").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn job_directories_sort_by_date_and_are_unique_per_id() {
        let id = JobId::new();
        let path = job_dir(
            Path::new("/data"),
            "Acme Robotics",
            Some("2026-09-02"),
            "Senior Platform Engineer",
            &id,
        );
        let s = path.to_string_lossy();
        assert!(s.contains("jobs/acme-robotics/2026-09-02-senior-platform-engineer-"));
        assert!(s.ends_with(id.short()));
    }
}
