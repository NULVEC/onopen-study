//! Turning ten thousand scans into the handful of numbers the study reports.
//!
//! Every figure here is a count over `data/scans/`, and the code that produces
//! it is the code that gets published alongside it. A number in the write-up
//! that cannot be traced to a function in this file does not go in the write-up.

use crate::github::Repo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// onopen's `Report`, as it comes back off disk.
///
/// `onopen::report::Report` is `Serialize` but not `Deserialize` — the scanner
/// has no reason to read its own output back. The shape is mirrored rather than
/// reinvented, and `tests/stored_report_matches_onopen.rs` fails if the two
/// ever disagree, so this cannot drift into quietly dropping a field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredReport {
    pub tool: String,
    pub version: String,
    pub root: String,
    pub findings: Vec<StoredFinding>,
    #[serde(default)]
    pub suppressed: Vec<StoredFinding>,
    #[serde(default)]
    pub unreadable: Vec<StoredUnreadable>,
    #[serde(default)]
    pub stale_ignore_lines: Vec<usize>,
    pub summary: StoredSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredFinding {
    pub rule: String,
    pub file: String,
    pub trigger: String,
    pub command: String,
    pub severity: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredUnreadable {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredSummary {
    pub immediate: usize,
    pub deferred: usize,
    pub note: usize,
    pub files_cleared: usize,
    pub suppressed: usize,
    pub unreadable: usize,
}

/// One repository's row in the dataset: what was scanned, at which commit, and
/// what came back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRecord {
    pub repo: Repo,
    pub sha: String,
    /// Configuration files actually present and downloaded.
    pub config_files: Vec<String>,
    /// GitHub capped the file listing for this repository.
    pub truncated: bool,
    /// Matched files that could not be downloaded.
    pub fetch_failed: usize,
    pub report: StoredReport,
}

impl ScanRecord {
    /// A repository whose answer is partial. It is counted separately
    /// everywhere rather than folded in either direction: calling it clean
    /// overstates safety, calling it a finding overstates the headline.
    pub fn is_partial(&self) -> bool {
        self.truncated || self.fetch_failed > 0 || self.report.summary.unreadable > 0
    }

    pub fn has_immediate(&self) -> bool {
        self.report.summary.immediate > 0
    }

    pub fn has_any_finding(&self) -> bool {
        !self.report.findings.is_empty()
    }

    /// Whether the repository ships configuration for a coding agent.
    ///
    /// This is the surface that did not exist two years ago, so it is counted
    /// on presence of the files rather than on findings: the interesting figure
    /// is how fast the surface itself is spreading.
    pub fn has_agent_surface(&self) -> bool {
        self.config_files.iter().any(|f| {
            f.starts_with(".claude/")
                || f.starts_with(".cursor/")
                || f.starts_with(".gemini/")
                || f == ".mcp.json"
                || f.ends_with("/.mcp.json")
        })
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Aggregate {
    pub onopen_version: String,
    pub repos_scanned: usize,
    /// Repositories whose scan answered the question completely.
    pub repos_complete: usize,
    pub repos_partial: usize,

    /// The headline: repositories with at least one thing that runs on open,
    /// on session start, or on install.
    pub repos_with_immediate: usize,
    pub repos_with_any_finding: usize,
    pub repos_clean: usize,

    /// The same headline restricted to complete scans, which is the honest
    /// denominator. Both are published; they should be close, and if they are
    /// not, that gap is itself the story.
    pub complete_with_immediate: usize,

    pub repos_with_agent_surface: usize,
    pub agent_surface_with_immediate: usize,

    pub findings_total: usize,
    pub by_severity: BTreeMap<String, usize>,
    /// Repositories, not findings — one repository with forty hooks should not
    /// look like forty repositories.
    pub repos_by_family: BTreeMap<String, usize>,
    pub repos_by_rule: BTreeMap<String, usize>,

    pub by_star_band: BTreeMap<String, Slice>,
    pub by_language: BTreeMap<String, Slice>,
    /// Keyed by the year of the last push, to show whether the agent surface is
    /// spreading or already settled.
    pub by_push_year: BTreeMap<String, Slice>,

    /// What the study could not read, reported at the top rather than in a
    /// footnote.
    pub repos_truncated: usize,
    pub repos_with_fetch_failures: usize,
    pub repos_with_unreadable_config: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Slice {
    pub repos: usize,
    pub with_immediate: usize,
    pub with_agent_surface: usize,
}

impl Slice {
    fn add(&mut self, r: &ScanRecord) {
        self.repos += 1;
        if r.has_immediate() {
            self.with_immediate += 1;
        }
        if r.has_agent_surface() {
            self.with_agent_surface += 1;
        }
    }
}

fn star_band(stars: u64) -> &'static str {
    match stars {
        0..=999 => "<1k",
        1_000..=4_999 => "1k-5k",
        5_000..=19_999 => "5k-20k",
        20_000..=49_999 => "20k-50k",
        _ => "50k+",
    }
}

pub fn aggregate(records: &[ScanRecord]) -> Aggregate {
    let mut agg = Aggregate {
        onopen_version: records
            .first()
            .map(|r| r.report.version.clone())
            .unwrap_or_default(),
        repos_scanned: records.len(),
        ..Default::default()
    };

    for r in records {
        if r.is_partial() {
            agg.repos_partial += 1;
        } else {
            agg.repos_complete += 1;
            if r.has_immediate() {
                agg.complete_with_immediate += 1;
            }
        }

        if r.has_immediate() {
            agg.repos_with_immediate += 1;
        }
        if r.has_any_finding() {
            agg.repos_with_any_finding += 1;
        } else {
            agg.repos_clean += 1;
        }
        if r.has_agent_surface() {
            agg.repos_with_agent_surface += 1;
            if r.has_immediate() {
                agg.agent_surface_with_immediate += 1;
            }
        }

        if r.truncated {
            agg.repos_truncated += 1;
        }
        if r.fetch_failed > 0 {
            agg.repos_with_fetch_failures += 1;
        }
        if r.report.summary.unreadable > 0 {
            agg.repos_with_unreadable_config += 1;
        }

        agg.findings_total += r.report.findings.len();

        let mut families = std::collections::BTreeSet::new();
        let mut rules = std::collections::BTreeSet::new();
        for f in &r.report.findings {
            *agg.by_severity.entry(f.severity.clone()).or_default() += 1;
            families.insert(f.rule.split('/').next().unwrap_or("other").to_string());
            rules.insert(f.rule.clone());
        }
        for family in families {
            *agg.repos_by_family.entry(family).or_default() += 1;
        }
        for rule in rules {
            *agg.repos_by_rule.entry(rule).or_default() += 1;
        }

        agg.by_star_band
            .entry(star_band(r.repo.stars).to_string())
            .or_default()
            .add(r);
        agg.by_language
            .entry(r.repo.language.clone().unwrap_or_else(|| "none".into()))
            .or_default()
            .add(r);
        agg.by_push_year
            .entry(r.repo.pushed_at.chars().take(4).collect())
            .or_default()
            .add(r);
    }

    agg
}

/// The lines the write-up quotes, so the prose and the dataset cannot disagree.
pub fn headline(agg: &Aggregate) -> String {
    let pct = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 * 100.0 / d as f64 };
    let mut out = String::new();
    out.push_str(&format!("onopen {}\n", agg.onopen_version));
    out.push_str(&format!(
        "{} repositories scanned — {} complete, {} partial\n",
        agg.repos_scanned, agg.repos_complete, agg.repos_partial
    ));
    out.push_str(&format!(
        "{} ({:.1}%) execute something on open, on session start, or on install\n",
        agg.repos_with_immediate,
        pct(agg.repos_with_immediate, agg.repos_scanned)
    ));
    out.push_str(&format!(
        "  over complete scans only: {} of {} ({:.1}%)\n",
        agg.complete_with_immediate,
        agg.repos_complete,
        pct(agg.complete_with_immediate, agg.repos_complete)
    ));
    out.push_str(&format!(
        "{} ({:.1}%) ship configuration for a coding agent; {} of those ({:.1}%) execute something\n",
        agg.repos_with_agent_surface,
        pct(agg.repos_with_agent_surface, agg.repos_scanned),
        agg.agent_surface_with_immediate,
        pct(agg.agent_surface_with_immediate, agg.repos_with_agent_surface)
    ));
    out.push_str(&format!(
        "not fully readable: {} truncated listings, {} with failed downloads, {} with unreadable config\n",
        agg.repos_truncated, agg.repos_with_fetch_failures, agg.repos_with_unreadable_config
    ));
    out.push_str("\nthis is a floor: live .git/hooks are not in any repository, so hook-path\nexposure is undercounted, never overcounted.\n");
    out
}
