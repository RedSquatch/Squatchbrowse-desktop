//! Settings live in ~/.config/squatchbrowse/config.toml (created on first run).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("squatchbrowse")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("squatchbrowse")
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub homepage: String,
    /// `{}` is replaced with the URL-encoded search text.
    pub search_url: String,
    pub video_dir: String,
    pub audio_dir: String,
    pub downloads_dir: String,
    pub ytdlp_path: String,
    pub block_popups: bool,
    /// Upgrade http:// pages to https:// and warn before loading unencrypted sites.
    pub https_only: bool,
    /// Reopen the tabs from last time on launch.
    pub restore_session: bool,
    /// Leave empty to use WebKit's default.
    pub user_agent: Option<String>,
    pub list_max_age_days: u64,
    pub filter_lists: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            homepage: "squatch://home".into(),
            search_url: "https://duckduckgo.com/?q={}".into(),
            video_dir: "~/Videos/SquatchBrowse".into(),
            audio_dir: "~/Music/SquatchBrowse".into(),
            downloads_dir: "~/Downloads".into(),
            ytdlp_path: "yt-dlp".into(),
            block_popups: true,
            restore_session: true,
            https_only: true,
            user_agent: None,
            list_max_age_days: 4,
            filter_lists: vec![
                "https://easylist.to/easylist/easylist.txt".into(),
                "https://easylist.to/easylist/easyprivacy.txt".into(),
                "https://secure.fanboy.co.nz/fanboy-annoyance.txt".into(),
                "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt".into(),
            ],
        }
    }
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let path = config_dir().join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&path) {
            match toml::from_str(&text) {
                Ok(cfg) => return Ok(cfg),
                Err(e) => eprintln!("[config] {path:?} is invalid, using defaults: {e}"),
            }
            return Ok(Config::default());
        }
        let cfg = Config::default();
        std::fs::create_dir_all(config_dir())?;
        std::fs::write(&path, toml::to_string_pretty(&cfg)?)?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(config_dir())?;
        std::fs::write(config_dir().join("config.toml"), toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn video_dir(&self) -> PathBuf {
        expand_tilde(&self.video_dir)
    }

    pub fn audio_dir(&self) -> PathBuf {
        expand_tilde(&self.audio_dir)
    }

    pub fn download_dir(&self) -> PathBuf {
        expand_tilde(&self.downloads_dir)
    }

    /// Turn address bar text into a URL: real URLs pass through, anything else is a search.
    pub fn resolve_input(&self, input: &str) -> String {
        let has_scheme = ["http://", "https://", "file://", "about:", "squatch://"]
            .iter()
            .any(|s| input.to_ascii_lowercase().starts_with(s));
        if has_scheme {
            return input.to_string();
        }
        let looks_like_host = !input.contains(' ')
            && (input.contains('.') || input.starts_with("localhost"))
            && !input.ends_with('.');
        if looks_like_host {
            // Home network addresses usually don't have certificates.
            let host = input.split(['/', ':']).next().unwrap_or("");
            let local = is_local_host(host);
            format!("{}://{input}", if local { "http" } else { "https" })
        } else {
            self.search_url.replace("{}", &url_encode(input))
        }
    }
}

/// Home network and loopback addresses, which normally have no public certificate.
pub fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
        || host.parse::<std::net::Ipv4Addr>().map(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local()).unwrap_or(false)
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_input() {
        let c = Config::default();
        assert_eq!(c.resolve_input("example.com"), "https://example.com");
        assert_eq!(c.resolve_input("https://a.b/c"), "https://a.b/c");
        assert_eq!(c.resolve_input("rust axum"), "https://duckduckgo.com/?q=rust+axum");
        assert_eq!(c.resolve_input("localhost:8080"), "http://localhost:8080");
        assert_eq!(c.resolve_input("192.168.1.20/admin"), "http://192.168.1.20/admin");
        assert_eq!(c.resolve_input("nas.local"), "http://nas.local");
        assert_eq!(c.resolve_input("8.8.8.8"), "https://8.8.8.8");
    }
}
