//! Walk `jobs/**/job.json` for reconcile.

use std::fs;
use std::path::{Path, PathBuf};

use jobseeker_core::{Error, Result};

use crate::jobfile::{FileJob, FileRequirement};

/// A job directory found on disk.
#[derive(Debug, Clone)]
pub struct DiscoveredJob {
    /// Path of the job directory relative to the data root, POSIX slashes.
    pub rel_dir: String,
    pub abs_dir: PathBuf,
    pub job: FileJob,
    pub requirements: Vec<FileRequirement>,
}

/// Recursively find every `job.json` under `$DATA_DIR/jobs`.
pub fn discover_jobs(data_dir: &Path) -> Result<Vec<DiscoveredJob>> {
    let jobs_root = data_dir.join("jobs");
    if !jobs_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut json_paths = Vec::new();
    collect_job_json(&jobs_root, &mut json_paths)?;
    json_paths.sort();
    let mut out = Vec::with_capacity(json_paths.len());
    for path in json_paths {
        out.push(load_job_dir(data_dir, &path)?);
    }
    Ok(out)
}

fn collect_job_json(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(Error::from)?;
    for entry in entries {
        let entry = entry.map_err(Error::from)?;
        let path = entry.path();
        if path.is_dir() {
            collect_job_json(&path, out)?;
        } else if path.file_name().is_some_and(|n| n == "job.json") {
            out.push(path);
        }
    }
    Ok(())
}

fn load_job_dir(data_dir: &Path, job_json: &Path) -> Result<DiscoveredJob> {
    let abs_dir = job_json
        .parent()
        .ok_or_else(|| Error::Storage("job.json has no parent".into()))?
        .to_path_buf();
    let text = fs::read_to_string(job_json)?;
    let mut job: FileJob =
        serde_json::from_str(&text).map_err(|e| Error::Serde(format!("job.json: {e}")))?;
    if job.description_md.is_empty() {
        let md_path = abs_dir.join("job.md");
        if md_path.is_file() {
            job.description_md = description_from_job_md(&fs::read_to_string(md_path)?);
        }
    }
    let req_path = abs_dir.join("requirements.json");
    let requirements = if req_path.is_file() {
        let raw = fs::read_to_string(req_path)?;
        serde_json::from_str(&raw).map_err(|e| Error::Serde(format!("requirements.json: {e}")))?
    } else {
        Vec::new()
    };
    let rel_dir = abs_dir
        .strip_prefix(data_dir)
        .unwrap_or(&abs_dir)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(DiscoveredJob {
        rel_dir,
        abs_dir,
        job,
        requirements,
    })
}

/// Body after YAML frontmatter, preferring a `## Description` section.
pub fn description_from_job_md(md: &str) -> String {
    let body = strip_frontmatter(md);
    if let Some(rest) = body
        .split_once("## Description")
        .map(|(_, r)| r.trim_start_matches('\n'))
    {
        return rest.trim().to_string();
    }
    body.trim().to_string()
}

fn strip_frontmatter(md: &str) -> &str {
    let trimmed = md.trim_start();
    if !trimmed.starts_with("---") {
        return md;
    }
    let after = &trimmed[3..];
    if let Some(end) = after.find("\n---") {
        let rest = &after[end + 4..];
        return rest.trim_start_matches('\n');
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_write;
    use tempfile::TempDir;

    #[test]
    fn discover_finds_nested_job_json() {
        let dir = TempDir::new().unwrap();
        let job_dir = dir.path().join("jobs/acme/2026-09-02-engineer-abcd1234");
        atomic_write(
            &job_dir.join("job.json"),
            br#"{
  "id": "01927f3c-0000-7000-8000-000000000001",
  "title": "Engineer",
  "company": "Acme",
  "status": "open",
  "work_mode": "remote",
  "seniority": "senior",
  "employment_type": "full_time",
  "content_hash": "b3:abc",
  "locations": ["US"]
}
"#,
        )
        .unwrap();
        atomic_write(
            &job_dir.join("requirements.json"),
            br#"[{"id":"01927f3c-0000-7000-8000-000000000002","text":"Rust","normalized_text":"rust","kind":"skill","necessity":"required"}]"#,
        )
        .unwrap();
        let found = discover_jobs(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].job.title, "Engineer");
        assert_eq!(found[0].requirements.len(), 1);
        assert!(found[0].rel_dir.starts_with("jobs/"));
    }

    #[test]
    fn description_falls_back_to_job_md() {
        let md = "---\ntitle: X\n---\n\n# X — Acme\n\n## Description\n\nOwn the platform.\n";
        assert_eq!(description_from_job_md(md), "Own the platform.");
    }
}
