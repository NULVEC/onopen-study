# onopen-study

**How many of GitHub's most popular repositories execute code when you open them?**

Nobody has published the number. This measures it, with
[onopen](https://github.com/NULVEC/onopen), and ships the code and the dataset
so anybody can get the same answer.

The study never clones a repository and never runs anything it downloads. It
reads the search index, asks each repository for its file listing, downloads the
configuration files onopen knows how to read, and scans those. A study of what
runs when you open a repository cannot be produced by opening ten thousand of
them.

## Running it

```sh
export GITHUB_TOKEN=...          # a token with no scopes is enough; public data only

cargo run --release -- sample --limit 10000   # the census        → data/repos.jsonl
cargo run --release -- fetch                  # the config files  → data/cache/
cargo run --release -- scan                   # what onopen found → data/scans/
cargo run --release -- report                 # the numbers       → data/aggregate.json
                                              #                   + data/dataset.jsonl
cargo run --release -- verify --sample 20     # the honesty check
```

Every step is resumable. `fetch` skips repositories it has already completed, so
a run interrupted at hour three continues rather than restarting.

Roughly one API request per repository, plus the file downloads, which go to
`raw.githubusercontent.com` and cost no quota. Ten thousand repositories fit
inside an afternoon on the standard 5000-requests-an-hour limit.

## Why the numbers can be trusted

**The scanner is the published one.** `scan` calls `onopen::scan` and
`Report::build` — the same two calls the `onopen` binary makes. Each row in
`data/scans/` is byte-for-byte what `onopen --json` prints for those files. The
study does not reimplement a single rule.

**Every repository is pinned to a commit.** `fetch` records the SHA the listing
came from and downloads every file at that SHA, so a re-run months later reads
the same bytes rather than whatever the default branch has become.

**The skeleton is proven equivalent, not assumed.** Downloading a subset of a
repository is only sound if onopen reads that subset the way it reads a clone.
Two tests hold that:

- `cargo test` builds skeletons from onopen's own hostile fixtures and asserts
  the findings match scanning the whole fixture, and asserts that every file
  onopen ships a fixture for is a file the path filter would have downloaded.
  When onopen gains a scanner, this fails until the filter learns about it.
- `verify` clones real repositories, scans the clone, and compares finding for
  finding against the stored skeleton. One mismatch means the filter is
  incomplete and the headline is wrong. It exits non-zero and says which rule
  it missed.

**Incomplete is never counted as clean.** GitHub caps large file listings; some
downloads fail; some config files cannot be parsed by anyone. All three are
counted, kept out of the clean column, and reported at the top of the output
rather than in a footnote.

## What the number is not

**It is a floor.** The study sees what is committed to the default branch. Live
`.git/hooks` exist only in a working copy, so a repository pointing
`core.hooksPath` at a directory it ships is undercounted here, never
overcounted. Nothing in the method can inflate the figure.

**It is not an accusation.** A `build.rs`, a devcontainer, a `preinstall` that
compiles a native module — these are what those files are for. The finding is
that the surface is large and nearly nothing looks at it, not that any
particular project did something wrong. The write-up reports aggregates. The
dataset is published in full so the work is reproducible; it is not a list to
shame anyone with.

Anything that looks genuinely malicious rather than merely executable goes to
the maintainer privately before it goes anywhere else.

## Layout

```
src/paths.rs     which files get downloaded — the one place undercounting can start
src/github.rs    the only code that touches the network
src/fetch.rs     rebuilding a scannable skeleton from a file listing
src/analyze.rs   scans → the numbers the write-up quotes
src/main.rs      the five steps
data/repos.jsonl    the census: which repositories, and how popular
data/dataset.jsonl  the published dataset — one line per repository, carrying
                    the commit it was read at and everything onopen found
data/aggregate.json the numbers the write-up quotes
data/cache/         downloaded config files, kept for re-scanning (not published)
data/scans/         one scan per file so a run can resume; `report` bundles
                    these into dataset.jsonl (not published on its own)
```

Findings come from onopen. The number comes from `analyze.rs`. A figure in the
write-up that cannot be traced to a function there does not go in the write-up.

## Licence

Apache-2.0, matching onopen.
