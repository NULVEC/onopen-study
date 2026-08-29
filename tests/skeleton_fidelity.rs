//! The study downloads a subset of each repository and scans that. These tests
//! hold the two claims that subset rests on.
//!
//! They run against onopen's own test fixtures, which are a deliberately
//! hostile repository covering every scanner family. The fixtures are excluded
//! from the published crate, so the tests skip when only the packaged onopen is
//! available and fail loudly whenever the checkout is there — which is the case
//! that matters, because that is where the numbers get produced.

use onopen_study::{analyze::StoredReport, paths};
use std::path::{Path, PathBuf};

fn fixtures() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../onopen/tests/fixtures");
    dir.is_dir().then(|| dir)
}

fn scan(root: &Path) -> StoredReport {
    let unit = onopen::scan(root, &onopen::ScanOptions::default()).expect("scan");
    let report = onopen::report::Report::build("fixture".into(), unit);
    serde_json::from_str(&onopen::report::render_json(&report)).expect("round-trip")
}

/// Every file onopen ships a fixture for is a file the study downloads.
///
/// A fixture exists because some scanner reads that file. If the path filter
/// would not have fetched it, the study cannot see that rule fire, and every
/// repository relying on it is silently counted clean. When onopen gains a
/// scanner with a new fixture, this test fails until the filter learns about
/// it — which is the whole point.
#[test]
fn every_fixture_file_would_be_fetched() {
    let Some(fixtures) = fixtures() else {
        eprintln!("onopen checkout not next door; skipping");
        return;
    };

    let mut missed = Vec::new();
    for entry in walkdir::WalkDir::new(&fixtures).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        // Relative to the fixture repository, not to the fixtures directory:
        // each of `clean`, `trapped` and `monorepo` is one repository.
        let rel = entry
            .path()
            .strip_prefix(&fixtures)
            .unwrap()
            .components()
            .skip(1)
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if rel.is_empty() {
            continue;
        }
        if !paths::is_watched(&rel) {
            missed.push(rel);
        }
    }

    assert!(
        missed.is_empty(),
        "the path filter would not have downloaded these files, so the study \
         cannot see what they contain:\n  {}",
        missed.join("\n  ")
    );
}

/// A skeleton scans exactly like the repository it was built from.
///
/// This is the fixture-sized version of the `verify` subcommand: same claim, no
/// network, runs in CI. `verify` then repeats it against real repositories,
/// where the surprises actually live.
#[test]
fn skeleton_scans_like_the_whole_repository() {
    let Some(fixtures) = fixtures() else {
        eprintln!("onopen checkout not next door; skipping");
        return;
    };

    for name in ["clean", "trapped", "monorepo"] {
        let whole = fixtures.join(name);
        if !whole.is_dir() {
            continue;
        }

        let skeleton = std::env::temp_dir().join(format!("onopen-study-skel-{name}"));
        if skeleton.exists() {
            std::fs::remove_dir_all(&skeleton).ok();
        }
        std::fs::create_dir_all(&skeleton).unwrap();

        // Copy in exactly what a fetch would have downloaded, and nothing else.
        for entry in walkdir::WalkDir::new(&whole).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&whole)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !paths::is_watched(&rel) {
                continue;
            }
            let dest = skeleton.join(&rel);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(entry.path(), &dest).unwrap();
        }

        let from_whole = scan(&whole);
        let from_skeleton = scan(&skeleton);

        assert_eq!(
            from_whole.findings, from_skeleton.findings,
            "fixture {name}: scanning a skeleton gave a different answer from \
             scanning the repository it was built from"
        );
        assert_eq!(
            from_whole.summary.unreadable, from_skeleton.summary.unreadable,
            "fixture {name}: the skeleton disagreed about what could not be read"
        );

        std::fs::remove_dir_all(&skeleton).ok();
    }
}

/// The mirrored `StoredReport` reads everything onopen writes.
///
/// `analyze::StoredReport` restates a struct that lives in onopen because the
/// scanner only serializes. If onopen adds a field and this mirror does not,
/// the study starts dropping it. Comparing against the raw JSON catches that.
#[test]
fn stored_report_keeps_every_field_onopen_writes() {
    let Some(fixtures) = fixtures() else {
        eprintln!("onopen checkout not next door; skipping");
        return;
    };

    let unit = onopen::scan(&fixtures.join("trapped"), &onopen::ScanOptions::default()).unwrap();
    let report = onopen::report::Report::build("trapped".into(), unit);
    let raw = onopen::report::render_json(&report);

    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let stored: StoredReport = serde_json::from_str(&raw).unwrap();
    let round_tripped: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();

    assert_eq!(
        original, round_tripped,
        "the study's copy of onopen's report shape has drifted from onopen's own"
    );
}
