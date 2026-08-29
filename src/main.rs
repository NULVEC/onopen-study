//! onopen-study — how many of GitHub's most popular repositories execute code
//! when you open them.
//!
//! Four steps, each resumable, each writing what the next one reads:
//!
//! ```text
//! sample  → data/repos.jsonl     the census: which repositories, at what popularity
//! fetch   → data/cache/<repo>/   the config files, at a pinned commit
//! scan    → data/scans/<repo>.json   what onopen found in each
//! report  → data/aggregate.json  the numbers the write-up quotes
//! verify  →                      proof that a skeleton scans like a real clone
//! ```
//!
//! Nothing here clones a repository and nothing executes what it downloads.

use onopen_study::{analyze, fetch, github};

use analyze::{ScanRecord, StoredReport};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use github::{Client, Repo};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Parser)]
#[command(name = "onopen-study", version, about, long_about = None)]
struct Cli {
    /// Where the census, the cache and the results live.
    #[arg(long, default_value = "data", global = true)]
    data: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the census of the most-starred public repositories.
    Sample {
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
    },
    /// Download the configuration files of every repository in the census.
    Fetch {
        /// Stop after this many repositories. Use it for a smoke run.
        #[arg(long)]
        limit: Option<usize>,
        /// Parallel downloads. The hourly quota, not this, is the real limit.
        #[arg(long, default_value_t = 8)]
        jobs: usize,
        /// Fetch again even where a complete result already exists.
        #[arg(long)]
        refresh: bool,
    },
    /// Run onopen over every fetched repository.
    Scan {
        #[arg(long, default_value_t = 8)]
        jobs: usize,
    },
    /// Reduce the scans to the numbers the write-up quotes.
    Report,
    /// Clone real repositories and check the skeletons scan identically.
    Verify {
        #[arg(long, default_value_t = 10)]
        sample: usize,
        /// Check these repositories instead of a random selection.
        #[arg(long)]
        repo: Vec<String>,
        /// Skip repositories larger than this, in kilobytes. Cloning the kernel
        /// to check a path filter costs gigabytes and proves nothing extra.
        #[arg(long, default_value_t = 400_000)]
        max_size_kb: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data = &cli.data;
    std::fs::create_dir_all(data)?;

    match cli.command {
        Command::Sample { limit } => sample(data, limit),
        Command::Fetch {
            limit,
            jobs,
            refresh,
        } => fetch_all(data, limit, jobs, refresh),
        Command::Scan { jobs } => scan_all(data, jobs),
        Command::Report => report(data),
        Command::Verify {
            sample,
            repo,
            max_size_kb,
        } => verify(data, sample, &repo, max_size_kb),
    }
}

fn census_path(data: &Path) -> PathBuf {
    data.join("repos.jsonl")
}

fn cache_dir(data: &Path) -> PathBuf {
    data.join("cache")
}

fn scans_dir(data: &Path) -> PathBuf {
    data.join("scans")
}

fn read_census(data: &Path) -> Result<Vec<Repo>> {
    let path = census_path(data);
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!("no census at {} — run `sample` first", path.display())
    })?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).context("parse census line"))
        .collect()
}

fn sample(data: &Path, limit: usize) -> Result<()> {
    let client = Client::from_env();
    if !client.is_authenticated() {
        eprintln!(
            "warning: no GITHUB_TOKEN. Unauthenticated search allows 10 requests a\n\
             minute, so a census of {limit} will take hours instead of minutes."
        );
    }

    let path = census_path(data);
    // The census is written as it arrives: a run interrupted after eighty bands
    // should not throw away eighty bands of search quota.
    let file = std::fs::File::create(&path)?;
    let writer = std::sync::Mutex::new(std::io::BufWriter::new(file));
    let count = AtomicUsize::new(0);

    let repos = client.most_starred(limit, |batch| {
        use std::io::Write;
        let mut w = writer.lock().unwrap();
        for repo in batch {
            if let Ok(line) = serde_json::to_string(repo) {
                let _ = writeln!(w, "{line}");
            }
        }
        let _ = w.flush();
        let n = count.fetch_add(batch.len(), Ordering::Relaxed) + batch.len();
        if let Some(last) = batch.last() {
            eprintln!("  {n} repositories, down to {} stars", last.stars);
        }
    })?;

    println!("{} repositories written to {}", repos.len(), path.display());
    Ok(())
}

fn fetch_all(data: &Path, limit: Option<usize>, jobs: usize, refresh: bool) -> Result<()> {
    let census = read_census(data)?;
    let cache = cache_dir(data);
    std::fs::create_dir_all(&cache)?;

    let todo: Vec<&Repo> = census
        .iter()
        .filter(|r| refresh || !fetch::already_done(&cache, &r.full_name))
        .take(limit.unwrap_or(usize::MAX))
        .collect();

    println!(
        "{} repositories to fetch ({} already done)",
        todo.len(),
        census.len() - todo.len()
    );

    let client = Client::from_env();
    if !client.is_authenticated() {
        bail!("fetch needs GITHUB_TOKEN: 60 unauthenticated requests an hour is not a study");
    }

    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;

    pool.install(|| {
        todo.par_iter().for_each(|repo| {
            match fetch::one(&client, &cache, repo) {
                Ok(meta) => {
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 50 == 0 || meta.truncated {
                        eprintln!(
                            "  [{n}/{}] {} — {} config files{}",
                            todo.len(),
                            repo.full_name,
                            meta.files.len(),
                            if meta.truncated { " (listing truncated)" } else { "" }
                        );
                    }
                }
                Err(e) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("  ! {}: {e:#}", repo.full_name);
                }
            }
        });
    });

    println!(
        "fetched {}, failed {}",
        done.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed)
    );
    Ok(())
}

/// Run the scanner over one fetched skeleton.
///
/// This is the same call the `onopen` binary makes — `scan`, then
/// `Report::build` — so the dataset holds exactly what someone would get by
/// running `onopen --json` on the same files.
fn scan_skeleton(tree: &Path, label: String) -> Result<onopen::report::Report> {
    let unit = onopen::scan(tree, &onopen::ScanOptions::default())?;
    Ok(onopen::report::Report::build(label, unit))
}

fn scan_all(data: &Path, jobs: usize) -> Result<()> {
    let cache = cache_dir(data);
    let scans = scans_dir(data);
    std::fs::create_dir_all(&scans)?;

    let slots: Vec<PathBuf> = std::fs::read_dir(&cache)
        .with_context(|| format!("no cache at {} — run `fetch` first", cache.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("meta.json").is_file())
        .collect();

    println!("{} repositories to scan", slots.len());

    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;

    pool.install(|| {
        slots.par_iter().for_each(|slot| {
            if let Err(e) = scan_one(slot, &scans) {
                eprintln!("  ! {}: {e:#}", slot.display());
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 500 == 0 {
                eprintln!("  [{n}/{}]", slots.len());
            }
        });
    });

    println!("scanned {}", done.load(Ordering::Relaxed));
    Ok(())
}

fn scan_one(slot: &Path, scans: &Path) -> Result<()> {
    let meta: fetch::Meta =
        serde_json::from_str(&std::fs::read_to_string(slot.join("meta.json"))?)?;
    let tree = slot.join("tree");

    let report = scan_skeleton(&tree, meta.repo.full_name.clone())?;
    // Round-tripping through onopen's own serializer means the record holds the
    // scanner's output, not this program's idea of it.
    let stored: StoredReport = serde_json::from_str(&onopen::report::render_json(&report))?;

    let record = ScanRecord {
        repo: meta.repo.clone(),
        sha: meta.sha,
        config_files: meta.files,
        truncated: meta.truncated,
        fetch_failed: meta.failed.len(),
        report: stored,
    };

    let name = meta.repo.full_name.replace('/', "__");
    std::fs::write(
        scans.join(format!("{name}.json")),
        serde_json::to_string(&record)?,
    )?;
    Ok(())
}

fn read_scans(data: &Path) -> Result<Vec<ScanRecord>> {
    let scans = scans_dir(data);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&scans)
        .with_context(|| format!("no scans at {} — run `scan` first", scans.display()))?
    {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path)?;
            out.push(
                serde_json::from_str(&text)
                    .with_context(|| format!("parse {}", path.display()))?,
            );
        }
    }
    out.sort_by(|a: &ScanRecord, b: &ScanRecord| b.repo.stars.cmp(&a.repo.stars));
    Ok(out)
}

fn report(data: &Path) -> Result<()> {
    let records = read_scans(data)?;
    if records.is_empty() {
        bail!("no scans to report on");
    }
    let agg = analyze::aggregate(&records);

    // `data/scans/` is one file per repository because `scan` has to be
    // resumable. That shape is right for the run and wrong for a reader: ten
    // thousand files is not something anyone downloads to check a number. The
    // published dataset is one line per repository, in the order the study
    // reports them.
    let dataset = data.join("dataset.jsonl");
    {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&dataset)?);
        for record in &records {
            writeln!(w, "{}", serde_json::to_string(record)?)?;
        }
        w.flush()?;
    }

    let out = data.join("aggregate.json");
    std::fs::write(&out, serde_json::to_string_pretty(&agg)?)?;
    print!("{}", analyze::headline(&agg));
    println!(
        "\nwritten to {} and {} ({} rows)",
        out.display(),
        dataset.display(),
        records.len()
    );
    Ok(())
}

/// The check the study does not ship without.
///
/// A skeleton is only worth anything if onopen reads it the way it reads a real
/// clone. This clones repositories for real, scans the clone, and compares
/// finding for finding against what the skeleton produced. A single mismatch
/// means the path filter is missing something and the headline is wrong.
fn verify(data: &Path, sample: usize, only: &[String], max_size_kb: u64) -> Result<()> {
    let records = read_scans(data)?;
    if records.is_empty() {
        bail!("nothing to verify — run `scan` first");
    }

    let chosen: Vec<&ScanRecord> = if only.is_empty() {
        // A partial record is one the study already knows it could not read in
        // full — a truncated listing, a download that failed. Comparing it
        // against a complete clone is guaranteed to differ, and counting that
        // as a mismatch would hide the mismatches that mean something. Those
        // repositories are excluded from the clean column in `report` instead,
        // which is where an unreadable repository belongs.
        //
        // Very large repositories are skipped for a duller reason: cloning the
        // kernel to check a path filter costs gigabytes and proves nothing that
        // a hundred smaller repositories do not.
        let eligible: Vec<&ScanRecord> = records
            .iter()
            .filter(|r| !r.is_partial() && r.repo.size <= max_size_kb)
            .collect();
        if eligible.is_empty() {
            bail!("no complete scans small enough to verify");
        }
        // Spread across the census rather than taking the top, which is all
        // very large repositories and not representative of what it scans.
        let step = (eligible.len() / sample.max(1)).max(1);
        eligible.into_iter().step_by(step).take(sample).collect()
    } else {
        records
            .iter()
            .filter(|r| only.contains(&r.repo.full_name))
            .collect()
    };

    let skipped = records.len() - records.iter().filter(|r| !r.is_partial()).count();
    if only.is_empty() && skipped > 0 {
        println!("{skipped} partial scans are not comparable and were left out");
    }

    let work = data.join("verify");
    std::fs::create_dir_all(&work)?;

    // Counted apart from `chosen`, because a repository that could not be
    // cloned was never compared to anything. Reporting the size of the
    // selection as the size of the evidence is how a verification tool comes to
    // overstate what it checked, which is the one failure it cannot afford.
    let mut mismatches = 0;
    let mut compared = 0;
    let mut unchecked: Vec<(String, &str)> = Vec::new();

    for record in &chosen {
        // Reachable only via an explicit --repo; the automatic selection has
        // already excluded these.
        if record.is_partial() {
            println!(
                "  ? {} — the study could not read it in full, so it is not comparable",
                record.repo.full_name
            );
            unchecked.push((record.repo.full_name.clone(), "scan was partial"));
            continue;
        }

        let dir = work.join(record.repo.full_name.replace('/', "__"));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }

        let url = format!("https://github.com/{}.git", record.repo.full_name);
        let status = std::process::Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--branch"])
            .arg(&record.repo.default_branch)
            .arg(&url)
            .arg(&dir)
            .status()
            .context("git clone — is git on PATH?")?;
        if !status.success() {
            println!("  ? {} — clone failed, not compared", record.repo.full_name);
            unchecked.push((record.repo.full_name.clone(), "clone failed"));
            continue;
        }

        let fresh = scan_skeleton(&dir, record.repo.full_name.clone())?;
        let fresh: StoredReport = serde_json::from_str(&onopen::report::render_json(&fresh))?;

        // The clone is at HEAD of the default branch and the skeleton is pinned
        // to the commit the listing came from. Those are usually the same
        // commit; when they are not, a difference is the repository moving, not
        // the skeleton being wrong, so it is reported and not counted.
        let same = fresh.findings == record.report.findings;
        let moved = clone_head(&dir).is_ok_and(|h| h != record.sha);

        if same {
            compared += 1;
            println!("  ok {} ({} findings)", record.repo.full_name, fresh.findings.len());
        } else if moved {
            println!(
                "  ? {} — repository moved since the fetch, not compared",
                record.repo.full_name
            );
            unchecked.push((record.repo.full_name.clone(), "moved since the fetch"));
        } else {
            mismatches += 1;
            println!("  MISMATCH {}", record.repo.full_name);
            report_difference(&record.report, &fresh);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    if mismatches > 0 {
        bail!(
            "{mismatches} of {compared} compared repositories scanned differently from a real \
             clone — the path filter is incomplete and the numbers are not publishable"
        );
    }

    println!("\n{compared} repositories scanned identically to a real clone");
    for (name, why) in &unchecked {
        println!("  not compared: {name} ({why})");
    }
    if compared == 0 {
        bail!("nothing was compared, so nothing was verified");
    }
    Ok(())
}

fn clone_head(dir: &Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn report_difference(skeleton: &StoredReport, clone: &StoredReport) {
    let key = |f: &analyze::StoredFinding| format!("{} {} {}", f.rule, f.file, f.trigger);
    let from_skeleton: std::collections::BTreeSet<String> =
        skeleton.findings.iter().map(key).collect();
    let from_clone: std::collections::BTreeSet<String> = clone.findings.iter().map(key).collect();

    for missed in from_clone.difference(&from_skeleton) {
        println!("      only in the clone: {missed}");
    }
    for extra in from_skeleton.difference(&from_clone) {
        println!("      only in the skeleton: {extra}");
    }
}
