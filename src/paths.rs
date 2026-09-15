//! Which files in a repository are worth downloading.
//!
//! The study never clones anything. It asks GitHub for the list of files in a
//! repository, keeps the ones onopen knows how to read, and downloads only
//! those. That makes this module the study's single point of undercounting: a
//! path that does not pass this filter is a path onopen never sees, and a
//! repository that would have reported a finding gets counted as clean.
//!
//! So the filter is deliberately wider than onopen's own rules. It costs a few
//! extra kilobytes per repository to fetch a file the scanners ignore, and it
//! costs the credibility of the whole study to miss one they would have read.
//! `tests/paths_cover_fixtures.rs` holds the line: it walks onopen's own test
//! fixtures and fails if any file they contain would not have been fetched.

/// Files onopen reads by name, wherever in the tree they appear. A repository
/// is rarely one project — onopen walks up to six directories down looking for
/// sub-projects — so these are matched on the file name, not on a full path.
const WATCHED_NAMES: &[&str] = &[
    // packages
    "package.json",
    "composer.json",
    "Gemfile",
    "pnpm-workspace.yaml",
    ".pnpmfile.cjs",
    ".pnpmfile.mjs",
    ".yarnrc.yml",
    ".npmrc",
    "bunfig.toml",
    "gems.rb",
    // cargo
    "Cargo.toml",
    "build.rs",
    // python
    "pyproject.toml",
    "setup.py",
    "sitecustomize.py",
    "conftest.py",
    "usercustomize.py",
    "noxfile.py",
    // environments
    ".envrc",
    "mise.toml",
    ".mise.toml",
    "shell.nix",
    "flake.nix",
    // git hooks
    ".pre-commit-config.yaml",
    // devcontainer, agents
    ".devcontainer.json",
    ".mcp.json",
    ".aider.conf.yml",
    // editors
    ".dir-locals.el",
    ".exrc",
    ".vimrc",
    ".nvimrc",
    ".nvim.lua",
];

/// Directories whose entire contents onopen may read. Everything below one of
/// these is fetched, because several of them hold files whose names onopen
/// discovers at runtime: a nested `devcontainer.json`, a hook script named
/// after a git event, a JetBrains run configuration named by another file.
const WATCHED_DIRS: &[&str] = &[
    ".vscode",
    ".claude",
    ".cursor",
    ".gemini",
    ".windsurf",
    ".continue",
    ".zed",
    ".idea",
    ".devcontainer",
    ".githooks",
    ".husky",
    ".hooks",
    ".cargo",
];

/// Directories onopen refuses to walk into, mirrored here so the study does not
/// pay to download a `package.json` that the scanner would never open. This is
/// `discover::WALK_SKIP` (called `ALWAYS_SKIP` before onopen 0.5.0); it is not
/// public, so it is repeated rather than imported.
///
/// Since 0.5.0 onopen also walks back into one of these when the git index says
/// a project there is committed. A skeleton has no index, so a config committed
/// under `build/` or `dist/` goes uncounted: one more reason the headline number
/// is a floor.
const ALWAYS_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    "coverage",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    "Pods",
];

/// How many directories below the repository root onopen looks for
/// sub-projects. Matches `onopen::DEFAULT_MAX_DEPTH`, which is what a plain
/// `onopen ./repo` uses, so the study measures what a reader would measure.
pub const MAX_DEPTH: usize = onopen::DEFAULT_MAX_DEPTH;

/// Bumped whenever the filter learns about files it did not fetch before, so a
/// cache built by an older filter can be topped up at the commits it was read
/// at instead of fetched again from scratch. Revision 1 covered onopen 0.4.0;
/// revision 2 adds what 0.5.1 started reading: Windsurf, Continue, Zed and Aider
/// configuration, `.npmrc`, `bunfig.toml`, `gems.rb`, nox, `usercustomize.py`
/// and `.pth` startup files.
pub const FILTER_REVISION: u32 = 2;

/// Whether a repository-relative path is one the study downloads.
///
/// `path` uses forward slashes, as the GitHub trees API returns them.
pub fn is_watched(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some((name, dirs)) = segments.split_last() else {
        return false;
    };

    if dirs.iter().any(|d| ALWAYS_SKIP.contains(d)) {
        return false;
    }

    // A file sitting deeper than onopen looks is a file onopen never reads.
    // The watched directories are part of the project that contains them, so
    // `.vscode/tasks.json` at depth 6 is still that project's config: only the
    // directories above the project count against the limit.
    let project_depth = dirs
        .iter()
        .take_while(|d| !WATCHED_DIRS.contains(d))
        .count();
    if project_depth > MAX_DEPTH {
        return false;
    }

    if dirs.iter().any(|d| WATCHED_DIRS.contains(d)) {
        return true;
    }

    WATCHED_NAMES.contains(name) || name.ends_with(".code-workspace") || name.ends_with(".pth")
}

/// Files that exist only in a working copy, never in a repository's tree.
///
/// `.git/hooks` and `.git/config` are the clearest example: git creates them on
/// clone, a repository cannot ship them, and `core.hooksPath` can point the
/// first at a directory the repository *does* ship. The study therefore cannot
/// see the most direct version of this attack, which is why its headline number
/// is reported as a floor rather than a measurement.
pub const UNREACHABLE_BY_API: &[&str] = &[".git/config", ".git/hooks/*"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_names_match_at_any_depth() {
        assert!(is_watched("package.json"));
        assert!(is_watched("packages/api/package.json"));
        assert!(is_watched("apps/web/sub/deep/one/Cargo.toml"));
    }

    #[test]
    fn watched_dirs_bring_their_whole_contents() {
        assert!(is_watched(".vscode/tasks.json"));
        assert!(is_watched(".claude/settings.local.json"));
        assert!(is_watched(".devcontainer/api/devcontainer.json"));
        assert!(is_watched(".idea/runConfigurations/Anything At All.xml"));
        assert!(is_watched(".githooks/pre-commit"));
    }

    #[test]
    fn code_workspace_matches_by_extension() {
        assert!(is_watched("my-project.code-workspace"));
        assert!(is_watched("nested/thing.code-workspace"));
    }

    #[test]
    fn skipped_directories_are_never_fetched() {
        assert!(!is_watched("node_modules/evil/package.json"));
        assert!(!is_watched("vendor/x/.vscode/tasks.json"));
        assert!(!is_watched(".git/hooks/pre-commit"));
    }

    #[test]
    fn ordinary_source_is_not_fetched() {
        assert!(!is_watched("src/main.rs"));
        assert!(!is_watched("README.md"));
        assert!(!is_watched("index.js"));
    }

    #[test]
    fn formats_onopen_learned_in_0_5_are_fetched() {
        // onopen's fixtures carry none of these, so the fixture test cannot
        // notice them missing. They are held here instead.
        for path in [
            ".windsurf/config.json",
            ".continue/config.yaml",
            ".zed/tasks.json",
            ".zed/debug.json",
            ".aider.conf.yml",
            ".npmrc",
            "packages/api/.npmrc",
            "bunfig.toml",
            "gems.rb",
            "noxfile.py",
            "usercustomize.py",
            "distutils-precedence.pth",
        ] {
            assert!(is_watched(path), "{path} is not fetched");
        }
    }

    #[test]
    fn past_the_depth_limit_is_out_of_reach() {
        assert!(!is_watched("a/b/c/d/e/f/g/package.json"));
    }
}
