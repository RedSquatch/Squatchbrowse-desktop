//! Finds yt-dlp and ffmpeg for Save, fetching them on first use when they aren't installed.
//!
//! - yt-dlp: uses ytdlp_path from config.toml if set; otherwise downloads the official
//!   standalone build into the app's data folder and keeps it up to date (`yt-dlp -U`).
//! - ffmpeg: uses the one on PATH if present. On Windows, otherwise downloads the build
//!   yt-dlp recommends. On Linux the package manager provides it (the .deb depends on it).

use crate::config;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Re-check for a newer yt-dlp after this long. Sites change often; old copies break.
const YTDLP_REFRESH: Duration = Duration::from_secs(7 * 86_400);

#[cfg(windows)]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

#[cfg(windows)]
const FFMPEG_ZIP_URL: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

pub fn bin_dir() -> PathBuf {
    config::data_dir().join("bin")
}

/// A `Command` that doesn't flash a console window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn runs(program: &Path, arg: &str) -> bool {
    command(program).arg(arg).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Path to a working yt-dlp. `say` reports slow first-time steps to the user.
pub fn ytdlp(configured: &str, say: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let configured = PathBuf::from(configured);
    let ours = bin_dir().join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });

    // A custom path in config.toml wins. Otherwise use our own copy: distro packages of
    // yt-dlp are often months old, and sites break old versions quickly.
    if configured.as_os_str() != "yt-dlp" {
        return if runs(&configured, "--version") {
            Ok(configured)
        } else {
            Err(format!("yt-dlp not found at {} (ytdlp_path in config.toml)", configured.display()))
        };
    }

    if ours.exists() {
        let age = std::fs::metadata(&ours)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .unwrap_or_default();
        if age > YTDLP_REFRESH {
            say("Checking for a newer yt-dlp…");
            let _ = command(&ours).arg("-U").output();
            touch(&ours);
        }
        return Ok(ours);
    }

    say("Getting yt-dlp (first save only)…");
    download_to(YTDLP_URL, &ours).map_err(|e| format!("Couldn't download yt-dlp: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o755));
    }
    if runs(&ours, "--version") {
        Ok(ours)
    } else {
        let _ = std::fs::remove_file(&ours);
        Err("yt-dlp downloaded but won't run".into())
    }
}

/// Where ffmpeg is: `Ok(None)` means it's on PATH, `Ok(Some(dir))` means pass `--ffmpeg-location dir`.
pub fn ffmpeg(say: &dyn Fn(&str)) -> Result<Option<PathBuf>, String> {
    if runs(Path::new("ffmpeg"), "-version") {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        let dir = bin_dir();
        let exe = dir.join("ffmpeg.exe");
        if exe.exists() && runs(&exe, "-version") {
            return Ok(Some(dir));
        }
        say("Getting ffmpeg (first save only, about 100 MB)…");
        fetch_ffmpeg_windows(&dir).map_err(|e| format!("Couldn't download ffmpeg: {e}"))?;
        return Ok(Some(dir));
    }
    #[cfg(not(windows))]
    {
        let _ = say;
        Err("ffmpeg not found. Install it with: sudo apt install ffmpeg".into())
    }
}

#[cfg(windows)]
fn fetch_ffmpeg_windows(dir: &Path) -> anyhow::Result<()> {
    let zip_path = dir.join("ffmpeg-download.zip");
    download_to(FFMPEG_ZIP_URL, &zip_path)?;
    let result = (|| -> anyhow::Result<()> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path)?)?;
        let mut found = 0;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name()?.replace('\\', "/");
            let Some(file) = ["ffmpeg.exe", "ffprobe.exe"].into_iter().find(|f| name.ends_with(&format!("bin/{f}")))
            else {
                continue;
            };
            let tmp = dir.join(format!("{file}.part"));
            std::io::copy(&mut entry, &mut std::fs::File::create(&tmp)?)?;
            std::fs::rename(&tmp, dir.join(file))?;
            found += 1;
        }
        anyhow::ensure!(found == 2, "ffmpeg.exe not found in the download");
        Ok(())
    })();
    let _ = std::fs::remove_file(&zip_path);
    result
}

/// Stream a URL to a file (via a temporary name, so a cut-off download never looks finished).
fn download_to(url: &str, dest: &Path) -> anyhow::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .into();
    let resp = agent.get(url).call()?;
    let mut reader = resp.into_body().into_reader();
    let tmp = dest.with_extension("part");
    let mut out = std::fs::File::create(&tmp)?;
    std::io::copy(&mut reader, &mut out)?;
    drop(out);
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

fn touch(path: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}
