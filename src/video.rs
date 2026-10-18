//! Save video or audio with yt-dlp. Runs on a worker thread and returns a status line.
//!
//! Tries the page URL first (yt-dlp knows how to read most video sites and
//! many embeds), then any direct video sources or player iframes the page
//! script found. DRM-protected streams are not supported; yt-dlp refuses them.

use crate::tools;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// MP4 (H.264 + AAC) up to 1080p: plays on everything.
    Mp4_1080,
    /// Best available quality up to 4K, usually WebM or MKV (VP9/AV1 + Opus).
    Best4k,
    /// The original audio stream, untouched (Opus or M4A, whichever the site has best).
    AudioBest,
    /// MP3 at 320 kbps with title and cover art.
    Mp3,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "best" => Mode::Best4k,
            "audio" => Mode::AudioBest,
            "mp3" => Mode::Mp3,
            _ => Mode::Mp4_1080,
        }
    }

    pub fn is_audio(self) -> bool {
        matches!(self, Mode::AudioBest | Mode::Mp3)
    }

    fn args(self) -> Vec<&'static str> {
        match self {
            // Prefer H.264 video and AAC audio at the highest resolution up to 1080p,
            // falling back gracefully if a site doesn't offer those.
            Mode::Mp4_1080 => vec![
                "-f", "bv*[height<=1080]+ba/b[height<=1080]/b",
                "-S", "res:1080,vcodec:h264,acodec:aac,ext:mp4:m4a",
                "--merge-output-format", "mp4",
                // Sites that only offer WebM get converted, so this mode always ends in MP4.
                "--recode-video", "mp4",
                "--embed-metadata",
            ],
            Mode::Best4k => vec![
                "-f", "bv*[height<=2160]+ba/b",
                "-S", "res:2160",
                "--embed-metadata",
            ],
            // -x with no --audio-format keeps the original stream: no quality lost to re-encoding.
            Mode::AudioBest => vec!["-f", "ba/b", "-x", "--embed-metadata", "--embed-thumbnail"],
            Mode::Mp3 => vec![
                "-f", "ba/b",
                "-x", "--audio-format", "mp3", "--audio-quality", "320K",
                "--embed-metadata", "--embed-thumbnail",
            ],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Mp4_1080 => "video",
            Mode::Best4k => "video",
            Mode::AudioBest | Mode::Mp3 => "audio",
        }
    }
}

/// `say` shows progress messages for slow first-time setup.
pub fn save(ytdlp: &str, dir: &Path, page: &str, candidates: &[String], mode: Mode, say: &dyn Fn(&str)) -> String {
    let ytdlp = match tools::ytdlp(ytdlp, say) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Merging video+audio, extracting audio and cover art all need ffmpeg.
    let ffmpeg_dir = match tools::ffmpeg(say) {
        Ok(d) => d,
        Err(e) => return e,
    };
    // YouTube needs a JavaScript runtime now; yt-dlp finds Deno on its PATH.
    let deno_dir = tools::deno(say);
    if let Err(e) = std::fs::create_dir_all(dir) {
        return format!("Can't create {}: {e}", dir.display());
    }
    let mut log = String::new();
    let template = dir.join("%(title).120B [%(id)s].%(ext)s");

    let mut last_err = String::new();
    for url in candidates {
        if url.starts_with("blob:") || url.starts_with("data:") || url.is_empty() {
            continue;
        }
        let mut cmd = tools::command(&ytdlp);
        if let Some(d) = &ffmpeg_dir {
            cmd.arg("--ffmpeg-location").arg(d);
        }
        if let Some(d) = &deno_dir {
            let mut paths = vec![d.clone()];
            if let Some(p) = std::env::var_os("PATH") { paths.extend(std::env::split_paths(&p)); }
            if let Ok(joined) = std::env::join_paths(paths) { cmd.env("PATH", joined); }
        }
        let out = cmd
            .args(["--no-playlist", "--no-progress", "--restrict-filenames"])
            .args(mode.args())
            .arg("--referer")
            .arg(page)
            .arg("-o")
            .arg(&template)
            .args(["--print", "after_move:filepath"])
            .arg(url)
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let path = String::from_utf8_lossy(&o.stdout).trim().lines().last().unwrap_or("").to_string();
                let name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(path);
                return format!("Saved {}: {name}", mode.label());
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                log.push_str(&format!("── {url}\n{err}\n"));
                last_err = err
                    .lines()
                    .rev()
                    .find(|l| l.contains("ERROR"))
                    .unwrap_or("unknown error")
                    .to_string();
            }
            Err(e) => { log.push_str(&format!("── {url}\ncouldn't start yt-dlp: {e}\n")); last_err = e.to_string() }
        }
    }
    // Keep the details where you can find them: the Save folder (the toolbar's folder button opens it).
    let _ = std::fs::write(dir.join("save-log.txt"), format!("Page: {page}\nDeno: {}\n\n{log}", deno_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "system or missing".into())));
    if last_err.contains("DRM") {
        "This video is DRM protected and can't be saved".into()
    } else if last_err.is_empty() {
        "No video found on this page".into()
    } else {
        format!("Save failed: {} (details in save-log.txt in the Save folder)", last_err.trim_start_matches("ERROR: "))
    }
}
