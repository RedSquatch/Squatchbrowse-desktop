//! Filter lists: download, cache, and turn into blocking rules.
//!
//! Linux: converted to WebKit content-blocker JSON (below).
//! Windows: loaded into Brave's engine, which checks each request (see win.rs).
//!
//! WebKit's content blocker understands the Safari rule format. adblock-rust
//! (Brave's engine) converts EasyList-style filters into that format, covering
//! both network blocking ("block") and cosmetic hiding ("css-display-none").
//! WebKit then compiles the rules into bytecode and applies them natively, so
//! blocking costs nothing per request at runtime.

use adblock::lists::{FilterSet, ParseOptions};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// WebKit compiles faster and more reliably with moderate chunks.
#[cfg(target_os = "linux")]
const RULES_PER_CHUNK: usize = 30_000;

pub struct Paths {
    pub lists_dir: PathBuf,
    pub store_dir: PathBuf,
    pub manifest: PathBuf,
    pub my_filters: PathBuf,
}

impl Paths {
    pub fn new(data_dir: &Path, config_dir: &Path) -> Self {
        Paths {
            lists_dir: data_dir.join("lists"),
            store_dir: data_dir.join("content-filters"),
            manifest: data_dir.join("filters-manifest.json"),
            my_filters: config_dir.join("my-filters.txt"),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Manifest {
    /// Identifiers of compiled filters in the WebKit store.
    pub ids: Vec<String>,
    pub rule_count: usize,
}

impl Manifest {
    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }
    pub fn write(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Lists are re-downloaded when older than this.
pub fn lists_are_stale(paths: &Paths, urls: &[String], max_age_days: u64) -> bool {
    let max_age = Duration::from_secs(max_age_days * 86_400);
    urls.iter().any(|url| {
        let file = paths.lists_dir.join(file_name_for(url));
        match fs::metadata(&file).and_then(|m| m.modified()) {
            Ok(t) => SystemTime::now().duration_since(t).unwrap_or_default() > max_age,
            Err(_) => true,
        }
    })
}

fn file_name_for(url: &str) -> String {
    url.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("list.txt")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect()
}

/// Download (or reuse cached) lists plus personal filters into one FilterSet.
/// `debug` must be true for WebKit content-blocking export.
fn load_lists(paths: &Paths, urls: &[String], force_download: bool, debug: bool) -> Result<(FilterSet, usize)> {
    fs::create_dir_all(&paths.lists_dir)?;
    let mut set = FilterSet::new(debug);
    let mut count = 0;

    for url in urls {
        let file = paths.lists_dir.join(file_name_for(url));
        let text = if force_download || !file.exists() {
            match download(url) {
                Ok(t) => {
                    fs::write(&file, &t)?;
                    t
                }
                Err(e) => {
                    eprintln!("[filters] download failed for {url}: {e:#}");
                    match fs::read_to_string(&file) {
                        Ok(t) => t, // keep using the old copy
                        Err(_) => continue,
                    }
                }
            }
        } else {
            fs::read_to_string(&file)?
        };
        count += rule_lines(&text);
        set.add_filter_list(text, ParseOptions::default());
    }

    // Personal filters (from Zap's "remember", or hand written).
    if let Ok(text) = fs::read_to_string(&paths.my_filters) {
        count += rule_lines(&text);
        set.add_filter_list(text, ParseOptions::default());
    }
    Ok((set, count))
}

/// Roughly how many rules a list has (lines that aren't blank or comments).
fn rule_lines(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('['))
        .count()
}

/// Linux: convert the lists to JSON chunks for WebKit's content blocker. Runs on a worker thread.
#[cfg(target_os = "linux")]
pub fn build_rule_chunks(paths: &Paths, urls: &[String], force_download: bool) -> Result<(Vec<String>, usize)> {
    let (set, _) = load_lists(paths, urls, force_download, true)?;
    let (rules, _unconverted) = set
        .into_content_blocking()
        .map_err(|_| anyhow::anyhow!("content blocking conversion failed"))?;

    let total = rules.len();
    let mut chunks = Vec::new();
    for chunk in rules.chunks(RULES_PER_CHUNK) {
        chunks.push(serde_json::to_string(chunk)?);
    }
    Ok((chunks, total))
}

/// Windows: build Brave's engine and hand it back serialized (the engine itself
/// stays on one thread). Runs on a worker thread. Returns (engine bytes, rule count).
#[cfg(windows)]
pub fn build_engine(paths: &Paths, urls: &[String], force_download: bool) -> Result<(Vec<u8>, usize)> {
    let (set, count) = load_lists(paths, urls, force_download, false)?;
    let engine = adblock::Engine::new_with_filter_set(set);
    Ok((engine.serialize(), count))
}

fn download(url: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut resp = agent.get(url).call().with_context(|| format!("GET {url}"))?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_string()?;
    Ok(text)
}

/// Append a cosmetic filter for a host, e.g. `example.com##div.popup`.
pub fn remember_cosmetic(paths: &Paths, host: &str, selector: &str) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = paths.my_filters.parent() {
        fs::create_dir_all(dir)?;
    }
    let line = format!("{host}##{selector}\n");
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&paths.my_filters)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

pub fn ensure_my_filters_file(paths: &Paths) {
    if paths.my_filters.exists() {
        return;
    }
    if let Some(dir) = paths.my_filters.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(
        &paths.my_filters,
        "! SquatchBrowse personal filters (uBlock / EasyList syntax)\n\
         ! Zap mode with Shift+click adds lines here automatically.\n\
         ! Examples:\n\
         !   ||annoying-ads.example^\n\
         !   storysite.example##.newsletter-popup\n",
    );
}
