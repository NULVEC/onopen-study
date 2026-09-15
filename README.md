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

## The answer, as of 2026-08-29

Of the 10,000 most-starred public repositories on GitHub, 9,997 were scanned —
three could not be fetched. Every one of them has at least 6,045 stars.

The repositories were read on 2026-08-29 and rescanned on 2026-09-15 with onopen
0.5.2, pinned to the same commits. The headline did not move: the families the
newer scanner learned (Zed, Bun, Bundler's `gems.rb`) add repositories to the
total without touching the agent figure.

| | |
|---|---|
| Execute something on open, on session start, or on install | **2,510 — 25.1%** |
| Same, over the 9,803 scans that were complete | 2,420 — 24.7% |
| Ship configuration for a coding agent | 879 — 8.8% |
| **Of those, execute something** | **548 — 62.3%** |

The likelihood rises with popularity rather than falling with it: 36.9% above
50,000 stars, 30.6% between 20,000 and 50,000, 23.1% below that.

Not fully readable, and therefore never counted clean: 60 truncated listings, 57
failed downloads, 82 repositories with a config file nothing can parse.

Full write-up: <https://veltron.cc/research/what-runs-when-you-open-a-repository>

## Running it

```sh
export GITHUB_TOKEN=...          # a token with no scopes is enough; public data only

cargo run --release --locked -- sample --limit 10000  # the census        → data/repos.jsonl
cargo run --release --locked -- fetch                 # the config files  → data/cache/
cargo run --release --locked -- scan                  # what onopen found → data/scans/
cargo run --release --locked -- report                # the numbers       → data/aggregate.json
                                                      #                   + data/dataset.jsonl
cargo run --release --locked -- verify --sample 20    # the honesty check
```

Every step is resumable. `fetch` skips repositories it has already completed, so
a run interrupted at hour three continues rather than restarting.

Roughly one API request per repository, plus the file downloads, which go to
`raw.githubusercontent.com` and cost no quota. Ten thousand repositories fit
inside an afternoon on the standard 5000-requests-an-hour limit.

## Why the numbers can be trusted

**The scanner is the published one.** The dependency is `onopen = "0.5.2"` from
crates.io, not a local checkout, and `Cargo.lock` is committed — so the version
that produced these numbers is the one `cargo install onopen` gives you. `scan`
calls `onopen::scan` and `Report::build` — the same two calls the binary makes.
Each row in the dataset is byte-for-byte what `onopen --json` prints for those
files. The study does not reimplement a single rule.

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
particular project did something wrong.

**The dataset names repositories. The write-up does not.** `dataset.jsonl`
carries every repository by name with what was found in it, because a study
whose aggregate cannot be recomputed is the kind of number this work exists to
argue against. That is a deliberate trade, and it comes with a limit: this is
not a ranking, not a shame list, and 2,510 named repositories running something
on open is a description of how the ecosystem works, not of who is careless in
it. Everything here is derived from public files in public repositories and can
be reproduced by anyone with the tool and an afternoon.

Anything that looked genuinely malicious rather than merely executable would go
to the maintainer privately before it went anywhere else, and the launch would
wait. Every immediate finding in this run was searched for the shapes that
would qualify — a pipe from `curl` into a shell, base64 decoded and executed,
`eval` over fetched data, a reverse shell, credentials read and sent somewhere,
an encoded PowerShell command. What came back was one toolchain installer from
a known vendor and four Gemfiles using the ordinary `eval(File.read(...))`
idiom to load a local override. Nothing was disclosed because nothing needed to
be.

That absence is worth stating plainly: 2,510 repositories run something when
you open them and, as far as this scan can tell, none of it is an attack. The
finding is the size of the surface, not evidence that it is being used.

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
