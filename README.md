# SquatchBrowse

A small reading browser for Linux and Windows, written in Rust (WebKitGTK on
Linux, WebView2 on Windows).
It blocks ads and trackers natively, strips pages down to the story, clears
overlays that lock up the page, and saves embedded video with yt-dlp.

## Install

Get it from SquatchVault (**Get the App**) or from this repo's Releases:

- **Windows 10/11:** run `SquatchBrowse-Setup-1.N.exe`. It installs for your user
  only (no admin). Windows may show "Windows protected your PC" because the
  installer isn't code-signed: click **More info → Run anyway**. The first time you
  use Save, it downloads yt-dlp and ffmpeg (about 100 MB) by itself.
- **Linux (Ubuntu, Mint, Debian, Pop!_OS):** open `SquatchBrowse-1.N.deb`, or run
  `sudo apt install ./SquatchBrowse-1.N.deb`. The first Save downloads yt-dlp,
  which then keeps itself up to date.

Every push to `main` builds both and publishes a release.

## Build from source (Ubuntu 24.04)

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev build-essential
sudo apt install ffmpeg      # optional, for "Save"
pipx install yt-dlp          # optional, for "Save"
cargo build --release
./target/release/squatchbrowse
```

The first launch downloads the filter lists and compiles them (about 30 seconds).
The shield dot turns green when blocking is active. After that, startup is instant.

## Tabs and bookmarks

| Action | Key |
|---|---|
| New tab (opens the bookmarks page) | Ctrl+T, or double click the tab strip |
| Close tab | Ctrl+W, or middle click the tab |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab (or Ctrl+PageDown / PageUp) |
| Jump to tab 1 to 8, last tab | Ctrl+1 … Ctrl+8, Ctrl+9 |
| Open link in background tab | Ctrl+click or middle click |
| Bookmark this page | Ctrl+D or the star |
| Bookmarks page | Ctrl+Shift+O or the bookmark button |

Links that ask for a new window open as a new tab. Popups that scripts open on
their own are still blocked. Your tabs come back when you restart; tabs you
haven't looked at yet only load when you click them.

The bookmarks page is also the new tab page. Type to filter, Enter to search
the web, arrow keys to pick a bookmark. Each bookmark can be renamed or put in a
folder. **Import** reads the HTML bookmarks file that Firefox, Chrome and Brave
export, and **Export** writes one back out.

## HTTPS and connection safety

- **HTTPS-only mode** (on by default; toggle it with the lock button in the toolbar): http links are
  upgraded to https automatically. If a site has no https, you get a warning page
  and can choose to continue unencrypted for that site until you close the browser.
- **Address bar indicator:** green lock = encrypted with a valid certificate;
  red "Not secure" = unencrypted; "Check" = encrypted but with a certificate
  issue; house = device on your home network. Click it for details.
- **Bad certificates** on public sites are blocked with no bypass. Devices on
  your home network (NAS, router) with self-signed certificates can be trusted
  for the session from the warning page.
- Home network addresses (`192.168.x.x`, `10.x.x.x`, `*.local`, `*.lan`,
  `localhost`) are never upgraded.
- Web content runs in WebKit's bubblewrap sandbox, which is always on.

## Toolbar

| Button | Key | What it does |
|---|---|---|
| Reader | F9 | Extracts the story with Mozilla Readability. Text size, column width, serif/sans and light/sepia/dark are adjustable and remembered per site. |
| Unstick | | Removes full-screen overlays and popups, unlocks scrolling, undoes blur. Use it when a page "freezes". |
| Zap | | Click any element to remove it. **Shift+click** removes it on every future visit (saved to your filters). Esc to finish. |
| Save | | Saves the page's video or audio with yt-dlp. Choose **1080p MP4** (H.264/AAC, plays everywhere), **up to 4K** (original WebM/MKV), **audio, best quality** (original stream, no re-encoding) or **MP3 320 kbps** (with title and cover art). Video goes to `~/Videos/SquatchBrowse`, audio to `~/Music/SquatchBrowse`. The badge counts videos found on the page. Needs `yt-dlp` and `ffmpeg`. |
| `<>` | | Turns JavaScript off and on. Very light pages, but Reader, Zap and Unstick need scripts. |
| Folder | | Opens the saved videos folder. |
| Pencil | | Opens `my-filters.txt` for hand-written rules. |
| Shield | | Re-downloads the filter lists now. |
| Wrench | F12 | Web inspector (developer tools). |

Also: Ctrl+L or F6 for the address bar, Alt+arrows for back/forward, F5 to reload.
Popups are blocked; a chip appears so you can open one on purpose.

## How blocking works

`adblock-rust` (Brave's engine) converts EasyList, EasyPrivacy, Fanboy
Annoyances and uBlock filters into WebKit's native content-blocker format.
WebKit compiles that into bytecode, so there is no per-request cost in Rust.
Both network rules (ads, trackers) and cosmetic rules (hiding ad boxes) apply.
Lists refresh every 4 days in the background.

## Files

| Path | Purpose |
|---|---|
| `~/.config/squatchbrowse/config.toml` | Homepage, search engine, folders, filter list URLs, user agent |
| `~/.config/squatchbrowse/my-filters.txt` | Your own rules, uBlock/EasyList syntax |
| `~/.local/share/squatchbrowse/bookmarks.json` | Bookmarks (each has an id and timestamps, ready for syncing later) |
| `~/.local/share/squatchbrowse/session.json` | Open tabs |
| `~/.local/share/squatchbrowse/` | Cached lists, compiled filters, cookies and logins |

## Code map

| File | Role |
|---|---|
| `src/main.rs` | Window, tabs, session restore, event routing |
| `src/bookmarks.rs` | Bookmark storage, import |
| `src/filters.rs` | Download, cache and convert filter lists |
| `src/content_filter.rs` | FFI to WebKit's content filter store |
| `src/video.rs` | yt-dlp runner |
| `src/config.rs` | Settings and address bar logic |
| `assets/page.js` | Reader, Unstick, Zap, video discovery, shortcuts |
| `assets/toolbar.html` | Tab strip and toolbar |
| `assets/home.html` | Bookmarks and new tab page (`squatch://home`) |
| `assets/Readability.js` | Mozilla Readability 0.6.0 (Apache 2.0) |

## Limits

- DRM video (Netflix and similar) can't be saved.
- Not built to get past real paywalls.
