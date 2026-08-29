//! The only part of the study that touches the network.
//!
//! It reads three things and writes nothing: the search index, a repository's
//! file listing, and the contents of individual files. It never clones a
//! repository, never runs a git hook, and never executes anything it downloads.
//! That is not a courtesy — a study of what code runs when you open a
//! repository cannot be produced by opening ten thousand of them.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API: &str = "https://api.github.com";
const RAW: &str = "https://raw.githubusercontent.com";
const UA: &str = "onopen-study (+https://github.com/NULVEC/onopen)";

/// How much of a quota to leave in reserve rather than spending to zero: a
/// parallel fetch has requests already in flight when the counter is read.
///
/// It has to be a fraction, not a fixed count. GitHub runs several buckets with
/// very different sizes — 5000 an hour for the API, 30 a minute for search — and
/// a fixed floor of 50 is above the whole search budget, so every single search
/// response looks like an exhausted quota and the client sleeps until the reset.
/// That turns a five-minute census into a three-hour one while GitHub is
/// throttling nothing at all.
fn reserve_for(limit: u64) -> u64 {
    (limit / 100).max(2)
}

pub struct Client {
    token: Option<String>,
    agent: ureq::Agent,
    /// When the core quota is exhausted, every thread waits for the same reset.
    core_reset: Mutex<Option<SystemTime>>,
}

/// One repository as the search index describes it.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Repo {
    pub full_name: String,
    #[serde(rename = "stargazers_count")]
    pub stars: u64,
    pub default_branch: String,
    pub language: Option<String>,
    /// Repository size in kilobytes, as GitHub reports it.
    pub size: u64,
    pub pushed_at: String,
    #[serde(default, deserialize_with = "license_key")]
    pub license: Option<String>,
}

/// Reads a licence from either shape this field ever has.
///
/// GitHub sends an object; the census this program writes keeps only the key,
/// as a string. Both have to parse, because `sample` writes the census and
/// `fetch` reads it back — a field that survives serialization but not
/// deserialization breaks the moment the run is resumed rather than when it is
/// written, which is the worst time to find out.
fn license_key<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct License {
        key: String,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Key(String),
        Object(License),
    }
    Ok(Option::<Either>::deserialize(d)?.map(|e| match e {
        Either::Key(k) => k,
        Either::Object(l) => l.key,
    }))
}

#[derive(Deserialize)]
struct SearchPage {
    /// Kept because it is what tells a reader of the log how much of the tail
    /// the thousand-result cap is hiding behind each query.
    #[allow(dead_code)]
    total_count: u64,
    items: Vec<Repo>,
}

/// One entry in a repository's file listing.
#[derive(Debug, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct TreeResponse {
    sha: String,
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

/// A repository's file listing, and whether GitHub gave us all of it.
pub struct Tree {
    /// The commit the listing came from. Recording it is what lets someone
    /// else re-run the study against the same bytes and get the same number.
    pub sha: String,
    pub entries: Vec<TreeEntry>,
    /// GitHub caps a recursive listing. A capped repository is reported as
    /// incomplete rather than counted clean.
    pub truncated: bool,
}

impl Client {
    pub fn new(token: Option<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(UA)
            .build()
            .into();
        Self {
            token,
            agent,
            core_reset: Mutex::new(None),
        }
    }

    pub fn from_env() -> Self {
        Self::new(
            std::env::var("GITHUB_TOKEN")
                .or_else(|_| std::env::var("GH_TOKEN"))
                .ok()
                .filter(|t| !t.trim().is_empty()),
        )
    }

    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// The most-starred repositories, walked downwards a thousand at a time.
    ///
    /// The search index refuses to page past 1000 results for one query, so
    /// "the top 10 000" is not a request GitHub answers. What it will answer is
    /// "the thousand most-starred repositories at or below N stars", so the
    /// walk takes a thousand, reads the star count of the last one, and asks
    /// again from there. Ten queries' worth of paging per thousand
    /// repositories, and no band can hide a tail behind the cap.
    ///
    /// The new ceiling is inclusive, so a run of repositories sharing a star
    /// count is never cut in half at the boundary; the overlap that creates is
    /// what the deduplication is for. It also absorbs a repository gaining a
    /// star mid-run and appearing twice.
    pub fn most_starred(
        &self,
        want: usize,
        mut on_batch: impl FnMut(&[Repo]),
    ) -> Result<Vec<Repo>> {
        /// What the search index will page through for one query.
        const PAGE_CAP: usize = 1000;
        /// Below this a repository is not "popular" in any useful sense, and
        /// the tail is long enough to walk forever.
        const STOP_AT: u64 = 100;

        let mut out: Vec<Repo> = Vec::with_capacity(want);
        let mut seen = std::collections::HashSet::new();
        // Open-ended at the top: nothing on GitHub sits above the first query.
        let mut ceiling: Option<u64> = None;

        while out.len() < want {
            let query = match ceiling {
                Some(c) => format!("stars:{STOP_AT}..{c}"),
                None => format!("stars:>={STOP_AT}"),
            };

            let mut lowest = None;
            let mut page = 1;

            loop {
                let batch = self.search_page(&query, page)?;
                if batch.items.is_empty() {
                    break;
                }
                lowest = batch.items.last().map(|r| r.stars);

                // Trimmed to what is still wanted before the callback sees it:
                // the caller streams these to disk as they arrive, and a census
                // file holding more repositories than were asked for is one the
                // later steps would silently fetch and scan.
                let room = want - out.len();
                let fresh: Vec<Repo> = batch
                    .items
                    .iter()
                    .filter(|r| seen.insert(r.full_name.clone()))
                    .take(room)
                    .cloned()
                    .collect();
                out.extend(fresh.iter().cloned());
                on_batch(&fresh);

                if out.len() >= want || batch.items.len() < 100 || page * 100 >= PAGE_CAP {
                    break;
                }
                page += 1;
            }

            let Some(lowest) = lowest else { break };
            // A ceiling that did not move means more than a thousand
            // repositories share this exact star count, and no query can reach
            // past them. Stopping is the honest answer; carrying on would loop.
            if Some(lowest) == ceiling || lowest <= STOP_AT {
                break;
            }
            ceiling = Some(lowest);
        }

        out.truncate(want);
        Ok(out)
    }

    fn search_page(&self, query: &str, page: usize) -> Result<SearchPage> {
        let url = format!(
            "{API}/search/repositories?q={}&sort=stars&order=desc&per_page=100&page={page}",
            urlencode(query)
        );
        let body = self
            .get_text(&url)
            .with_context(|| format!("search {query:?} page {page}"))?;
        // Search has its own, much smaller budget: 30 requests a minute
        // authenticated. Pacing here is cheaper than being throttled.
        std::thread::sleep(Duration::from_millis(if self.is_authenticated() {
            2100
        } else {
            6500
        }));
        serde_json::from_str(&body).context("search response")
    }

    /// Every file in a repository, in one request.
    pub fn tree(&self, full_name: &str, branch: &str) -> Result<Tree> {
        let url = format!("{API}/repos/{full_name}/git/trees/{branch}?recursive=1");
        let body = self
            .get_text(&url)
            .with_context(|| format!("tree for {full_name}"))?;
        let parsed: TreeResponse = serde_json::from_str(&body).context("tree response")?;
        Ok(Tree {
            sha: parsed.sha,
            entries: parsed
                .tree
                .into_iter()
                .filter(|e| e.kind == "blob")
                .collect(),
            truncated: parsed.truncated,
        })
    }

    /// One file's contents, pinned to the commit the listing came from.
    ///
    /// This goes to the raw CDN rather than the contents API on purpose: it
    /// does not spend the hourly quota, which is what makes ten thousand
    /// repositories affordable in an afternoon.
    pub fn blob(&self, full_name: &str, sha: &str, path: &str) -> Result<Vec<u8>> {
        let url = format!("{RAW}/{full_name}/{sha}/{path}");
        let mut req = self.agent.get(&url);
        if let Some(token) = &self.token {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
        let mut res = req
            .call()
            .with_context(|| format!("fetch {path} from {full_name}"))?;
        let mut reader = res.body_mut().as_reader();
        let mut buf = Vec::new();
        std::io::copy(&mut reader, &mut buf)?;
        Ok(buf)
    }

    /// A GET that waits out the rate limit instead of failing on it.
    fn get_text(&self, url: &str) -> Result<String> {
        for attempt in 0..6u32 {
            self.wait_for_quota();

            let mut req = self
                .agent
                .get(url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(token) = &self.token {
                req = req.header("Authorization", &format!("Bearer {token}"));
            }

            match req.call() {
                Ok(mut res) => {
                    self.note_quota(res.headers());
                    return Ok(res.body_mut().read_to_string()?);
                }
                Err(ureq::Error::StatusCode(code @ (403 | 429))) => {
                    // Secondary rate limit, or the primary one noticed a
                    // request too late. Back off and come back.
                    let wait = Duration::from_secs(30u64 << attempt.min(4));
                    eprintln!("  rate limited ({code}), waiting {}s", wait.as_secs());
                    std::thread::sleep(wait);
                }
                Err(ureq::Error::StatusCode(404)) => bail!("404"),
                Err(ureq::Error::StatusCode(code)) => bail!("HTTP {code}"),
                Err(e) if attempt < 5 => {
                    eprintln!("  {e}, retrying");
                    std::thread::sleep(Duration::from_secs(2u64 << attempt));
                }
                Err(e) => return Err(e).context("request failed"),
            }
        }
        bail!("gave up after repeated rate limiting")
    }

    fn note_quota(&self, headers: &ureq::http::HeaderMap) {
        let get = |k: &str| -> Option<u64> { headers.get(k)?.to_str().ok()?.parse().ok() };
        let (Some(remaining), Some(reset), Some(limit)) = (
            get("x-ratelimit-remaining"),
            get("x-ratelimit-reset"),
            get("x-ratelimit-limit"),
        ) else {
            return;
        };
        let mut slot = self.core_reset.lock().unwrap();
        // Only ever move the wait later, never earlier: a search response
        // carrying a reset one minute out must not cancel a wait the API bucket
        // set an hour out.
        let exhausted = remaining <= reserve_for(limit);
        let until = UNIX_EPOCH + Duration::from_secs(reset);
        *slot = match (*slot, exhausted) {
            (Some(existing), _) if existing > until => Some(existing),
            (_, true) => Some(until),
            (existing, false) => existing.filter(|e| *e > SystemTime::now()),
        };
    }

    fn wait_for_quota(&self) {
        let until = *self.core_reset.lock().unwrap();
        let Some(until) = until else { return };
        match until.duration_since(SystemTime::now()) {
            Ok(left) => {
                eprintln!("  hourly quota spent, resuming in {}s", left.as_secs() + 1);
                std::thread::sleep(left + Duration::from_secs(2));
            }
            Err(_) => {}
        }
        *self.core_reset.lock().unwrap() = None;
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this guards: a fixed reserve of 50 is larger than the entire
    /// search budget, so every search response reads as an exhausted quota and
    /// the client sleeps until the reset instead of carrying on.
    #[test]
    fn the_reserve_fits_inside_every_bucket() {
        assert_eq!(reserve_for(5000), 50, "hourly API bucket keeps a real margin");
        assert_eq!(reserve_for(30), 2, "search bucket keeps a margin it can afford");
        assert!(reserve_for(30) < 30, "a reserve must never exceed its own budget");
        assert!(reserve_for(10) < 10, "including the unauthenticated search budget");
    }
}
