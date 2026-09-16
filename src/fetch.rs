//! Rebuilding just enough of a repository for the scanner to read it.
//!
//! What lands on disk is a skeleton: the configuration files, at the paths they
//! occupy in the repository, and nothing else. onopen reads exactly those
//! files, so a skeleton and a full clone give it the same thing to look at —
//! and `verify` proves that on real repositories rather than asserting it.
//!
//! Skeletons are faithful in one way that matters: the trees API returns
//! tracked files only, which is precisely what someone cloning the repository
//! receives.

use crate::github::{Client, Repo};
use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// onopen refuses to read a config file larger than this, so downloading one
/// buys nothing. Mirrors `onopen::scanners::MAX_CONFIG_BYTES`.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// What the study knows about one repository after fetching it. Written next to
/// the skeleton, never inside it, so nothing the study creates can end up in
/// its own scan.
#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub repo: Repo,
    /// The commit every file below was read at.
    pub sha: String,
    /// GitHub capped the file listing, so the scan of this repository is
    /// partial and is reported that way rather than counted clean.
    pub truncated: bool,
    pub files: Vec<String>,
    /// Paths that matched but could not be downloaded, with the reason. Carried
    /// for the same reason onopen carries `unreadable`: a partial answer must
    /// not be able to pass for a clean one.
    pub failed: Vec<(String, String)>,
    /// Files skipped for being past the size onopen will read.
    pub oversized: Vec<String>,
    /// Which revision of the path filter built this skeleton. Caches written
    /// before the field existed were built by revision 1.
    #[serde(default = "first_filter_revision")]
    pub filter_revision: u32,
}

fn first_filter_revision() -> u32 {
    1
}

impl Meta {
    /// Whether the scan built on this fetch answers the question completely.
    pub fn is_complete(&self) -> bool {
        !self.truncated && self.failed.is_empty()
    }
}

/// Where one repository's fetch lives. `owner/name` becomes `owner__name`,
/// which is unambiguous because neither half may contain an underscore pair.
pub fn slot(cache: &Path, full_name: &str) -> PathBuf {
    cache.join(full_name.replace('/', "__"))
}

pub fn meta_path(cache: &Path, full_name: &str) -> PathBuf {
    slot(cache, full_name).join("meta.json")
}

pub fn tree_path(cache: &Path, full_name: &str) -> PathBuf {
    slot(cache, full_name).join("tree")
}

/// Whether this repository has already been fetched in full.
///
/// Resumption reads the meta file rather than trusting the directory to exist:
/// a run killed mid-write leaves a directory with a partial skeleton, and
/// treating that as done would quietly under-report it.
pub fn already_done(cache: &Path, full_name: &str) -> bool {
    let path = meta_path(cache, full_name);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Meta>(&s).ok())
        .is_some()
}

/// Download one repository's configuration files into a skeleton.
pub fn one(client: &Client, cache: &Path, repo: &Repo) -> Result<Meta> {
    let slot = slot(cache, &repo.full_name);
    let tree_dir = slot.join("tree");
    // A previous partial attempt would otherwise leave files from an older
    // commit sitting next to files from this one.
    if tree_dir.exists() {
        std::fs::remove_dir_all(&tree_dir).ok();
    }
    std::fs::create_dir_all(&tree_dir)
        .with_context(|| format!("create {}", tree_dir.display()))?;

    let listing = client.tree(&repo.full_name, &repo.default_branch)?;

    let mut meta = Meta {
        repo: repo.clone(),
        sha: listing.sha.clone(),
        truncated: listing.truncated,
        files: Vec::new(),
        failed: Vec::new(),
        oversized: Vec::new(),
        filter_revision: paths::FILTER_REVISION,
    };

    for entry in &listing.entries {
        if !paths::is_watched(&entry.path) {
            continue;
        }
        if entry.size.is_some_and(|s| s > MAX_FILE_BYTES) {
            meta.oversized.push(entry.path.clone());
            continue;
        }

        match client.blob(&repo.full_name, &listing.sha, &entry.path) {
            Ok(bytes) => {
                let dest = tree_dir.join(&entry.path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &bytes)
                    .with_context(|| format!("write {}", dest.display()))?;
                meta.files.push(entry.path.clone());
            }
            Err(e) => meta.failed.push((entry.path.clone(), e.to_string())),
        }
    }

    let json = serde_json::to_string_pretty(&meta)?;
    std::fs::write(slot.join("meta.json"), json)?;
    Ok(meta)
}

/// Bring a skeleton built by an older path filter up to the current one,
/// without moving it off the commit it was read at.
///
/// The listing is requested at `meta.sha`, not at the default branch, so the
/// files already on disk and the ones added now come from the same commit, and
/// a rerun measures what changed in onopen rather than what changed in the
/// repositories. Paths that failed or were oversized before are left as they
/// were: retrying them would change the study's completeness for a reason that
/// has nothing to do with the scanner.
///
/// Returns how many files were added.
pub fn topup(client: &Client, cache: &Path, meta: &mut Meta) -> Result<usize> {
    let tree_dir = tree_path(cache, &meta.repo.full_name);
    let listing = client.tree(&meta.repo.full_name, &meta.sha)?;

    let mut added = 0;
    for entry in &listing.entries {
        if !paths::is_watched(&entry.path)
            || meta.files.contains(&entry.path)
            || meta.oversized.contains(&entry.path)
            || meta.failed.iter().any(|(path, _)| path == &entry.path)
        {
            continue;
        }
        if entry.size.is_some_and(|s| s > MAX_FILE_BYTES) {
            meta.oversized.push(entry.path.clone());
            continue;
        }
        match client.blob(&meta.repo.full_name, &meta.sha, &entry.path) {
            Ok(bytes) => {
                let dest = tree_dir.join(&entry.path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &bytes)
                    .with_context(|| format!("write {}", dest.display()))?;
                meta.files.push(entry.path.clone());
                added += 1;
            }
            Err(e) => meta.failed.push((entry.path.clone(), e.to_string())),
        }
    }

    meta.filter_revision = paths::FILTER_REVISION;
    let json = serde_json::to_string_pretty(&*meta)?;
    std::fs::write(meta_path(cache, &meta.repo.full_name), json)?;
    Ok(added)
}

/// Read back a fetch that a previous run wrote.
pub fn load_meta(cache: &Path, full_name: &str) -> Result<Meta> {
    let path = meta_path(cache, full_name);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}
