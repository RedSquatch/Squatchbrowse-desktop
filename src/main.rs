//! SquatchBrowse: a small, privacy-first reading browser.
//!
//! Layout: one window holding
//!   - the toolbar webview (tab strip + controls, local HTML, no network)
//!   - one content webview per tab; only the active one is visible.
//! Every callback forwards a `UserEvent` through the tao event loop, so all
//! state lives in `App` and is touched only on the main thread.
//!
//! Linux runs on WebKitGTK (blocking via WebKit's compiled content blocker).
//! Windows runs on WebView2 (blocking via Brave's engine per request, see win.rs).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod bookmarks;
mod config;
#[cfg(target_os = "linux")]
mod content_filter;
mod filters;
mod tools;
mod video;
#[cfg(windows)]
mod win;

use bookmarks::Bookmarks;
use config::Config;
use filters::{Manifest, Paths};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Instant;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Window, WindowBuilder},
};
use wry::{http::Response, NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder};

#[cfg(target_os = "linux")]
use {
    content_filter::FilterStore,
    gtk::prelude::*,
    std::{cell::RefCell, collections::HashMap, rc::Rc},
    tao::platform::unix::WindowExtUnix,
    webkit2gtk::{gio, SettingsExt, UserContentManagerExt, WebContextExt, WebViewExt},
    wry::{WebViewBuilderExtUnix, WebViewExtUnix},
};

#[cfg(windows)]
use {
    std::{cell::RefCell, rc::Rc},
    wry::{dpi::LogicalPosition, dpi::LogicalSize, Rect},
};

const TOOLBAR_HTML: &str = include_str!("../assets/toolbar.html");
const HOME_HTML: &str = include_str!("../assets/home.html");
const ERROR_HTML: &str = include_str!("../assets/error.html");
const PAGE_JS: &str = include_str!("../assets/page.js");
const READABILITY_JS: &str = include_str!("../assets/Readability.js");
#[cfg(target_os = "linux")]
const TOOLBAR_HEIGHT: i32 = 84;
#[cfg(windows)]
const TOOLBAR_HEIGHT: f64 = 84.0;
const HOME_URL: &str = "squatch://home";

pub type TabId = u32;

/// What a filter build hands back: WebKit rule chunks on Linux, a serialized engine on Windows.
#[cfg(target_os = "linux")]
type Built = (Vec<String>, usize);
#[cfg(windows)]
type Built = (Vec<u8>, usize);

pub enum UserEvent {
    Toolbar(Value),
    /// Message from page.js: (tab, page URL as reported by the engine, body)
    Page(TabId, String, Value),
    Load(TabId, PageLoadEvent, String),
    Popup(TabId, String),
    /// A load failed for a reason other than being cancelled: (tab, uri)
    LoadFailed(TabId, String),
    /// The certificate for (tab, uri) was rejected.
    TlsError(TabId, String),
    /// HTTPS-only stopped an http:// navigation before it left the machine (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Upgrade(TabId, String),
    FiltersBuilt(Result<Built, String>),
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    FilterSaved(Result<String, String>),
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    FilterLoaded(Result<String, String>),
    Toast(String),
    VideoDone(String),
}

struct Tab {
    id: TabId,
    view: WebView,
    url: String,
    title: String,
    loading: bool,
    videos: u64,
    /// Restored tabs load lazily, the first time they're shown.
    pending_url: Option<String>,
    /// Simple rate limit for page-requested new tabs.
    opened: Vec<Instant>,
    /// HTTPS with a valid certificate, checked when the page finishes loading.
    secure: bool,
    /// The http:// URL we're trying to reach over https://, while that attempt is in flight.
    upgrading: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Session {
    tabs: Vec<SessionTab>,
    active: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionTab {
    url: String,
    title: String,
}

struct App {
    cfg: Config,
    paths: Paths,
    window: Window,
    ctx: WebContext,
    toolbar: WebView,
    proxy: EventLoopProxy<UserEvent>,
    tabs: Vec<Tab>,
    active: TabId,
    next_id: TabId,
    bookmarks: Bookmarks,
    // filter state
    filter_ids: Vec<String>,
    rule_count: usize,
    pending_loads: usize,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pending_saves: Vec<String>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    saved_ok: Vec<String>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    auto_rebuilt: bool,
    building: bool,
    load_failed: bool,
    js_enabled: bool,
    /// Hosts the user chose to visit without encryption (this run only).
    http_allowed: HashSet<String>,
    video_busy: bool,

    #[cfg(target_os = "linux")]
    store: FilterStore,
    #[cfg(target_os = "linux")]
    vbox: gtk::Box,
    #[cfg(target_os = "linux")]
    protocol_registered: bool,
    /// Certificates WebKit rejected, keyed by URL, so a local device can be trusted on request.
    #[cfg(target_os = "linux")]
    certs: Rc<RefCell<HashMap<String, gio::TlsCertificate>>>,

    #[cfg(windows)]
    engine: win::SharedEngine,
    #[cfg(windows)]
    policy: win::SharedPolicy,
}

fn main() -> anyhow::Result<()> {
    let cfg = Config::load_or_create()?;
    let data_dir = config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let paths = Paths::new(&data_dir, &config::config_dir());
    filters::ensure_my_filters_file(&paths);
    // A link opened from another app (the OS passes it as an argument).
    let open_arg = std::env::args().nth(1).filter(|a| a.starts_with("http://") || a.starts_with("https://"));

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("SquatchBrowse")
        .with_inner_size(tao::dpi::LogicalSize::new(1240.0, 840.0))
        .build(&event_loop)?;

    let p = proxy.clone();
    let toolbar_builder = WebViewBuilder::new().with_html(TOOLBAR_HTML).with_ipc_handler(move |req| {
        if let Ok(v) = serde_json::from_str(req.body()) {
            let _ = p.send_event(UserEvent::Toolbar(v));
        }
    });

    #[cfg(target_os = "linux")]
    let vbox = window.default_vbox().expect("tao default vbox").clone();
    #[cfg(target_os = "linux")]
    let toolbar = {
        let t = toolbar_builder.build_gtk(&vbox)?;
        let w = t.webview();
        vbox.set_child_packing(&w, false, false, 0, gtk::PackType::Start);
        w.set_size_request(-1, TOOLBAR_HEIGHT);
        t
    };
    #[cfg(windows)]
    let toolbar = toolbar_builder.with_bounds(toolbar_bounds(&window)).build_as_child(&window)?;

    let mut app = App {
        bookmarks: Bookmarks::load(data_dir.join("bookmarks.json")),
        ctx: WebContext::new(Some(data_dir.join("profile"))),
        cfg,
        paths,
        window,
        toolbar,
        proxy,
        tabs: vec![],
        active: 0,
        next_id: 1,
        filter_ids: vec![],
        rule_count: 0,
        pending_loads: 0,
        pending_saves: vec![],
        saved_ok: vec![],
        auto_rebuilt: false,
        building: false,
        load_failed: false,
        js_enabled: true,
        http_allowed: HashSet::new(),
        video_busy: false,
        #[cfg(target_os = "linux")]
        store: FilterStore::new(&Paths::new(&data_dir, &config::config_dir()).store_dir),
        #[cfg(target_os = "linux")]
        vbox,
        #[cfg(target_os = "linux")]
        protocol_registered: false,
        #[cfg(target_os = "linux")]
        certs: Rc::new(RefCell::new(HashMap::new())),
        #[cfg(windows)]
        engine: Rc::new(RefCell::new(None)),
        #[cfg(windows)]
        policy: Rc::new(RefCell::new(win::Policy::default())),
    };
    #[cfg(windows)]
    {
        app.policy.borrow_mut().https_only = app.cfg.https_only;
    }

    app.restore_session()?;
    if let Some(url) = open_arg {
        let _ = app.new_tab(&url, true, None);
    }
    app.start_filters();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.save_session();
                *control_flow = ControlFlow::Exit
            }
            #[cfg(windows)]
            Event::WindowEvent { event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }, .. } => {
                app.layout()
            }
            Event::UserEvent(ev) => app.handle(ev),
            _ => {}
        }
    });
}

// ============================================================ URLs

/// The address the engine uses for one of our own pages. On Windows wry serves
/// `squatch://home` as `http://squatch.home`; everywhere else it's unchanged.
fn nav_url(url: &str) -> Cow<'_, str> {
    #[cfg(windows)]
    if let Some(rest) = url.strip_prefix("squatch://") {
        return Cow::Owned(format!("{}{rest}", win::INTERNAL_PREFIX));
    }
    Cow::Borrowed(url)
}

/// The reverse of `nav_url`: what the engine reports, as the app thinks of it.
fn app_url(url: &str) -> String {
    #[cfg(windows)]
    if let Some(rest) = url.strip_prefix(win::INTERNAL_PREFIX) {
        return format!("squatch://{rest}");
    }
    url.to_string()
}

fn squatch_page(req: wry::http::Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let page = req.uri().host().unwrap_or("home");
    // On Windows the host arrives as "squatch.home" style; take the last part.
    let page = page.rsplit('.').next().unwrap_or(page);
    let body: Cow<'static, [u8]> = match page {
        "home" | "bookmarks" | "newtab" => Cow::Borrowed(HOME_HTML.as_bytes()),
        "error" => Cow::Borrowed(ERROR_HTML.as_bytes()),
        _ => Cow::Borrowed(b"Not found".as_slice()),
    };
    Response::builder()
        .header("Content-Type", "text/html; charset=utf-8")
        .body(body)
        .unwrap()
}

#[cfg(windows)]
fn toolbar_bounds(window: &Window) -> Rect {
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    Rect { position: LogicalPosition::new(0.0, 0.0).into(), size: LogicalSize::new(size.width, TOOLBAR_HEIGHT).into() }
}

#[cfg(windows)]
fn content_bounds(window: &Window) -> Rect {
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    Rect {
        position: LogicalPosition::new(0.0, TOOLBAR_HEIGHT).into(),
        size: LogicalSize::new(size.width, (size.height - TOOLBAR_HEIGHT).max(0.0)).into(),
    }
}

impl App {
    // ================================================================ tabs

    fn new_tab(&mut self, url: &str, activate: bool, lazy_title: Option<&str>) -> anyhow::Result<TabId> {
        let id = self.next_id;
        self.next_id += 1;
        let (p1, p2, p3) = (self.proxy.clone(), self.proxy.clone(), self.proxy.clone());
        let block_popups = self.cfg.block_popups;
        let lazy = lazy_title.is_some();
        let start = if lazy { "about:blank".to_string() } else { nav_url(url).into_owned() };
        #[cfg(windows)]
        let page_js = format!("window.__sqCosmetic=1;\n{PAGE_JS}");
        #[cfg(not(windows))]
        let page_js = PAGE_JS.to_string();

        let mut builder = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url(&start)
            .with_devtools(true)
            .with_visible(false)
            .with_initialization_script_for_main_only(&page_js, true)
            .with_ipc_handler(move |req| {
                if let Ok(v) = serde_json::from_str(req.body()) {
                    let _ = p1.send_event(UserEvent::Page(id, app_url(&req.uri().to_string()), v));
                }
            })
            .with_on_page_load_handler(move |ev, url| {
                let _ = p2.send_event(UserEvent::Load(id, ev, app_url(&url)));
            })
            .with_new_window_req_handler(move |url, _features| {
                if block_popups {
                    let _ = p3.send_event(UserEvent::Popup(id, app_url(&url)));
                    NewWindowResponse::Deny
                } else {
                    NewWindowResponse::Allow
                }
            });
        if let Some(ua) = self.cfg.user_agent.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.with_user_agent(ua);
        }

        // On Linux the scheme and the download handler belong to the shared WebContext,
        // so they're registered once, with the first tab. WebView2 needs them on every tab.
        #[cfg(target_os = "linux")]
        let register = !std::mem::replace(&mut self.protocol_registered, true);
        #[cfg(windows)]
        let register = true;
        if register {
            let download_dir = self.cfg.download_dir();
            builder = builder
                .with_custom_protocol("squatch".into(), |_id, req| squatch_page(req))
                .with_download_started_handler(move |url, dest| {
                    let name = dest
                        .file_name()
                        .map(|n| n.to_owned())
                        .unwrap_or_else(|| url.rsplit('/').next().unwrap_or("download").into());
                    let _ = std::fs::create_dir_all(&download_dir);
                    *dest = download_dir.join(name);
                    true
                });
        }

        #[cfg(target_os = "linux")]
        let view = {
            let view = builder.build_gtk(&self.vbox)?;
            self.setup_linux_tab(&view, id);
            view
        };
        #[cfg(windows)]
        let view = {
            let view = builder.with_bounds(content_bounds(&self.window)).build_as_child(&self.window)?;
            win::attach(&view, id, self.engine.clone(), self.policy.clone(), self.proxy.clone());
            if !self.js_enabled {
                win::set_javascript(&view, false);
            }
            view
        };

        self.tabs.push(Tab {
            id,
            view,
            url: if lazy { url.to_string() } else { String::new() },
            title: lazy_title.unwrap_or("").to_string(),
            loading: !lazy,
            videos: 0,
            pending_url: if lazy { Some(url.to_string()) } else { None },
            opened: vec![],
            secure: false,
            upgrading: None,
        });
        if activate || self.tabs.len() == 1 {
            self.activate(id);
        } else {
            self.push_tabs();
            self.save_session();
        }
        Ok(id)
    }

    /// WebKit specifics for a new tab: error hooks, settings, ad blocking rules.
    #[cfg(target_os = "linux")]
    fn setup_linux_tab(&mut self, view: &WebView, id: TabId) {
        let wv = view.webview();
        let p4 = self.proxy.clone();
        wv.connect_load_failed(move |_, _ev, uri, err| {
            let cancelled = err.matches(webkit2gtk::NetworkError::Cancelled)
                || err.matches(webkit2gtk::PolicyError::FrameLoadInterruptedByPolicyChange);
            if !cancelled {
                let _ = p4.send_event(UserEvent::LoadFailed(id, uri.to_string()));
            }
            false
        });
        let p5 = self.proxy.clone();
        let certs = self.certs.clone();
        wv.connect_load_failed_with_tls_errors(move |_, uri, cert, _flags| {
            certs.borrow_mut().insert(uri.to_string(), cert.clone());
            let _ = p5.send_event(UserEvent::TlsError(id, uri.to_string()));
            true // never show the page
        });

        if let Some(settings) = WebViewExt::settings(&wv) {
            settings.set_javascript_can_open_windows_automatically(false);
            settings.set_enable_developer_extras(true);
            settings.set_enable_javascript(self.js_enabled);
        }
        // Give the new tab the ad blocking rules everyone else has.
        let manager = wv.user_content_manager().unwrap();
        for fid in self.filter_ids.clone() {
            self.pending_loads += 1;
            self.store.load(&fid, &manager, self.loaded_cb());
        }
    }

    /// Windows: child webviews don't follow the window size on their own.
    #[cfg(windows)]
    fn layout(&self) {
        let _ = self.toolbar.set_bounds(toolbar_bounds(&self.window));
        let content = content_bounds(&self.window);
        for t in &self.tabs {
            let _ = t.view.set_bounds(content);
        }
    }

    fn load(view: &WebView, url: &str) {
        let _ = view.load_url(&nav_url(url));
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tab(self.active)
    }

    fn activate(&mut self, id: TabId) {
        if self.tab(id).is_none() {
            return;
        }
        self.active = id;
        for t in &self.tabs {
            let _ = t.view.set_visible(t.id == id);
        }
        if let Some(t) = self.tab_mut(id) {
            if let Some(url) = t.pending_url.take() {
                Self::load(&t.view, &url);
                t.loading = true;
            }
        }
        if let Some(t) = self.active_tab() {
            let _ = t.view.focus();
        }
        self.push_tabs();
        self.push_active();
        self.save_session();
    }

    fn close_tab(&mut self, id: TabId) {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.tabs.remove(pos);
        #[cfg(target_os = "linux")]
        self.vbox.remove(&tab.view.webview());
        drop(tab);
        if self.tabs.is_empty() {
            let home = self.cfg.homepage.clone();
            let _ = self.new_tab(&home, true, None);
            return;
        }
        if self.active == id {
            let next = self.tabs[pos.min(self.tabs.len() - 1)].id;
            self.activate(next);
        } else {
            self.push_tabs();
            self.save_session();
        }
    }

    fn cycle(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n < 2 {
            return;
        }
        let pos = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0) as isize;
        let next = self.tabs[((pos + step).rem_euclid(n)) as usize].id;
        self.activate(next);
    }

    fn open_bookmarks(&mut self) {
        if let Some(id) = self.tabs.iter().find(|t| t.url.starts_with(HOME_URL)).map(|t| t.id) {
            self.activate(id);
        } else {
            let _ = self.new_tab(HOME_URL, true, None);
        }
    }

    // ============================================================ session

    fn session_path(&self) -> std::path::PathBuf {
        config::data_dir().join("session.json")
    }

    fn restore_session(&mut self) -> anyhow::Result<()> {
        let session: Option<Session> = if self.cfg.restore_session {
            std::fs::read_to_string(self.session_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        } else {
            None
        };
        match session {
            Some(s) if !s.tabs.is_empty() => {
                let active = s.active.min(s.tabs.len() - 1);
                let mut active_id = 0;
                for (i, t) in s.tabs.iter().enumerate() {
                    let id = if i == active {
                        self.new_tab(&t.url, false, None)?
                    } else {
                        self.new_tab(&t.url, false, Some(&t.title))?
                    };
                    if i == active {
                        active_id = id;
                    }
                }
                self.activate(active_id);
            }
            _ => {
                let home = self.cfg.homepage.clone();
                self.new_tab(&home, true, None)?;
            }
        }
        Ok(())
    }

    fn save_session(&self) {
        let tabs: Vec<SessionTab> = self
            .tabs
            .iter()
            .filter(|t| !t.url.is_empty() && t.url != "about:blank")
            .map(|t| SessionTab { url: t.url.clone(), title: t.title.clone() })
            .collect();
        let active = self
            .tabs
            .iter()
            .filter(|t| !t.url.is_empty() && t.url != "about:blank")
            .position(|t| t.id == self.active)
            .unwrap_or(0);
        if let Ok(text) = serde_json::to_string(&Session { tabs, active }) {
            let _ = std::fs::write(self.session_path(), text);
        }
    }

    // ======================================================== toolbar sync

    fn ui(&self, v: Value) {
        let _ = self.toolbar.evaluate_script(&format!("window.ui && ui.set({v})"));
    }

    fn toast(&self, msg: &str) {
        self.ui(json!({ "toast": msg }));
    }

    fn push_tabs(&self) {
        let list: Vec<Value> = self
            .tabs
            .iter()
            .map(|t| {
                let title = if t.url.starts_with(HOME_URL) {
                    "Bookmarks".to_string()
                } else if t.title.is_empty() {
                    if t.url.is_empty() { "New tab".to_string() } else { t.url.clone() }
                } else {
                    t.title.clone()
                };
                json!({ "id": t.id, "title": title, "loading": t.loading, "active": t.id == self.active })
            })
            .collect();
        self.ui(json!({ "tabs": list }));
    }

    /// Address bar, star, video count and loading bar follow the active tab.
    fn push_active(&self) {
        if let Some(t) = self.active_tab() {
            let shown = if t.url.starts_with("squatch://") || t.url == "about:blank" { "" } else { t.url.as_str() };
            let security = if shown.is_empty() || t.loading {
                ""
            } else if t.url.starts_with("https://") {
                if t.secure { "secure" } else { "warning" }
            } else if t.url.starts_with("http://") {
                if config::is_local_host(&host_of(&t.url)) { "local" } else { "insecure" }
            } else {
                ""
            };
            self.ui(json!({
                "url": shown,
                "security": security,
                "loading": t.loading,
                "videos": t.videos,
                "starred": !shown.is_empty() && self.bookmarks.contains(&t.url),
            }));
        }
    }

    fn set_window_title(&self) {
        let title = self.active_tab().map(|t| t.title.as_str()).unwrap_or("");
        self.window
            .set_title(&if title.is_empty() { "SquatchBrowse".into() } else { format!("{title} · SquatchBrowse") });
    }

    /// Is the page that just finished loading encrypted with a valid certificate?
    fn page_is_secure(t: &Tab) -> bool {
        if !t.url.starts_with("https://") {
            return false;
        }
        #[cfg(target_os = "linux")]
        return t.view.webview().tls_info().map(|(_, flags)| flags.is_empty()).unwrap_or(false);
        // WebView2 refuses bad certificates outright (we're told through NavigationCompleted),
        // so an https page that finished loading had a valid one.
        #[cfg(windows)]
        return true;
    }

    // ============================================================ filters

    fn start_filters(&mut self) {
        let stale = filters::lists_are_stale(&self.paths, &self.cfg.filter_lists, self.cfg.list_max_age_days);
        match Manifest::read(&self.paths.manifest) {
            Some(m) if !m.ids.is_empty() => {
                self.rule_count = m.rule_count;
                #[cfg(target_os = "linux")]
                self.attach_everywhere(m.ids);
                #[cfg(windows)]
                {
                    let ok = std::fs::read(self.engine_path())
                        .ok()
                        .and_then(|b| win::engine_from_bytes(&b))
                        .map(|e| *self.engine.borrow_mut() = Some(e))
                        .is_some();
                    if !ok {
                        self.rebuild(stale);
                        return;
                    }
                    self.filter_ids = m.ids;
                    self.shield_status();
                }
                if stale {
                    self.rebuild(true);
                }
            }
            _ => self.rebuild(stale),
        }
    }

    #[cfg(windows)]
    fn engine_path(&self) -> std::path::PathBuf {
        config::data_dir().join("engine.dat")
    }

    /// Replace the filters on every tab with `ids` (already compiled in the store).
    #[cfg(target_os = "linux")]
    fn attach_everywhere(&mut self, ids: Vec<String>) {
        self.filter_ids = ids;
        self.load_failed = false;
        for t in &self.tabs {
            let manager = t.view.webview().user_content_manager().unwrap();
            manager.remove_all_filters();
            for fid in &self.filter_ids {
                self.pending_loads += 1;
                self.store.load(fid, &manager, self.loaded_cb());
            }
        }
        self.shield_status();
    }

    fn rebuild(&mut self, download: bool) {
        if self.building {
            return;
        }
        self.building = true;
        self.shield_status();
        let urls = self.cfg.filter_lists.clone();
        let paths = Paths::new(&config::data_dir(), &config::config_dir());
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            #[cfg(target_os = "linux")]
            let res = filters::build_rule_chunks(&paths, &urls, download);
            #[cfg(windows)]
            let res = filters::build_engine(&paths, &urls, download);
            let _ = proxy.send_event(UserEvent::FiltersBuilt(res.map_err(|e| format!("{e:#}"))));
        });
    }

    #[cfg(target_os = "linux")]
    fn loaded_cb(&self) -> content_filter::Callback {
        let p = self.proxy.clone();
        Rc::new(move |r| {
            let _ = p.send_event(UserEvent::FilterLoaded(r));
        })
    }

    #[cfg(target_os = "linux")]
    fn saved_cb(&self) -> content_filter::Callback {
        let p = self.proxy.clone();
        Rc::new(move |r| {
            let _ = p.send_event(UserEvent::FilterSaved(r));
        })
    }

    fn shield_status(&self) {
        let state = if self.building && self.pending_saves.is_empty() {
            "updating lists"
        } else if !self.pending_saves.is_empty() {
            "compiling"
        } else if self.pending_loads > 0 {
            "loading"
        } else if self.filter_ids.is_empty() || self.load_failed {
            "off"
        } else {
            "on"
        };
        self.ui(json!({ "shield": state, "rules": self.rule_count }));
    }

    #[cfg(target_os = "linux")]
    fn filters_built(&mut self, (chunks, total): Built) {
        self.rule_count = total;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let ids: Vec<String> = (0..chunks.len()).map(|i| format!("squatch-{stamp}-{i}")).collect();
        self.pending_saves = ids.clone();
        self.shield_status();
        for (id, json) in ids.iter().zip(chunks) {
            self.store.save(id, json, self.saved_cb());
        }
    }

    #[cfg(windows)]
    fn filters_built(&mut self, (bytes, total): Built) {
        self.building = false;
        match win::engine_from_bytes(&bytes) {
            Some(engine) => {
                *self.engine.borrow_mut() = Some(engine);
                self.rule_count = total;
                self.filter_ids = vec!["engine".into()];
                self.load_failed = false;
                if let Err(e) = std::fs::write(self.engine_path(), &bytes) {
                    eprintln!("[filters] couldn't cache engine: {e}");
                }
                let m = Manifest { ids: self.filter_ids.clone(), rule_count: total };
                if let Err(e) = m.write(&self.paths.manifest) {
                    eprintln!("[filters] manifest write failed: {e}");
                }
            }
            None => {
                self.load_failed = true;
                self.toast("Filter update failed");
            }
        }
        self.shield_status();
    }

    // ============================================================= events

    fn handle(&mut self, ev: UserEvent) {
        match ev {
            UserEvent::Toolbar(v) => self.toolbar_cmd(&v),
            UserEvent::Page(id, origin, v) => self.page_msg(id, &origin, &v),
            UserEvent::Load(id, state, url) => {
                let started = matches!(state, PageLoadEvent::Started);
                // Linux upgrades here; on Windows win.rs catches it before the request goes out.
                if cfg!(target_os = "linux") && started && self.should_upgrade(&url) {
                    if let Some(t) = self.tab_mut(id) {
                        if t.pending_url.is_none() {
                            t.upgrading = Some(url.clone());
                            let _ = t.view.load_url(&url.replacen("http://", "https://", 1));
                            return;
                        }
                    }
                }
                if let Some(t) = self.tab_mut(id) {
                    if t.pending_url.is_some() {
                        return; // the about:blank placeholder of a lazy tab
                    }
                    t.url = url;
                    t.loading = started;
                    if started {
                        t.videos = 0;
                        t.secure = false;
                    } else {
                        t.secure = Self::page_is_secure(t);
                        if t.secure {
                            t.upgrading = None;
                        }
                    }
                }
                self.push_tabs();
                if id == self.active {
                    self.push_active();
                }
                if !started {
                    self.save_session();
                }
            }
            UserEvent::Upgrade(id, url) => {
                if let Some(t) = self.tab_mut(id) {
                    t.upgrading = Some(url.clone());
                    t.pending_url = None;
                    let _ = t.view.load_url(&url.replacen("http://", "https://", 1));
                }
            }
            UserEvent::Popup(id, url) => {
                if id == self.active && !url.is_empty() && url != "about:blank" {
                    self.ui(json!({ "popup": url }));
                }
            }
            UserEvent::FiltersBuilt(Ok(built)) => self.filters_built(built),
            UserEvent::FiltersBuilt(Err(e)) => {
                self.building = false;
                eprintln!("[filters] {e}");
                self.toast(&format!("Filter update failed: {e}"));
                self.shield_status();
            }
            #[cfg(target_os = "linux")]
            UserEvent::FilterSaved(res) => {
                let id = match res {
                    Ok(id) => {
                        self.saved_ok.push(id.clone());
                        id
                    }
                    Err(e) => {
                        eprintln!("[filters] compile failed: {e}");
                        e.split(':').next().unwrap_or("").to_string()
                    }
                };
                self.pending_saves.retain(|x| *x != id);
                if self.pending_saves.is_empty() {
                    self.building = false;
                    let ids = std::mem::take(&mut self.saved_ok);
                    if ids.is_empty() {
                        self.toast("Filter compile failed; see terminal output");
                        self.shield_status();
                        return;
                    }
                    let m = Manifest { ids, rule_count: self.rule_count };
                    if let Err(e) = m.write(&self.paths.manifest) {
                        eprintln!("[filters] manifest write failed: {e}");
                    }
                    self.attach_everywhere(m.ids);
                }
            }
            #[cfg(target_os = "linux")]
            UserEvent::FilterLoaded(res) => {
                self.pending_loads = self.pending_loads.saturating_sub(1);
                if let Err(e) = res {
                    eprintln!("[filters] load failed: {e}");
                    self.load_failed = true;
                    // The compiled store is missing or stale: rebuild from cached lists, once.
                    if !self.auto_rebuilt {
                        self.auto_rebuilt = true;
                        self.rebuild(false);
                    }
                }
                if self.pending_loads == 0 {
                    self.shield_status();
                }
            }
            #[cfg(not(target_os = "linux"))]
            UserEvent::FilterSaved(_) | UserEvent::FilterLoaded(_) => {}
            UserEvent::LoadFailed(id, uri) => {
                let Some(t) = self.tab_mut(id) else { return };
                if let Some(orig) = t.upgrading.take() {
                    if uri.starts_with("https://") && host_of(&uri) == host_of(&orig) {
                        let page = format!("squatch://error?kind=insecure&url={}", config::url_encode(&orig));
                        Self::load(&t.view, &page);
                    }
                }
            }
            UserEvent::TlsError(id, uri) => {
                let Some(t) = self.tab_mut(id) else { return };
                // Trusting a home-network certificate is only wired up on Linux so far.
                let local = cfg!(target_os = "linux") && config::is_local_host(&host_of(&uri));
                let page = match t.upgrading.take() {
                    // We guessed https for an http link and the site has no proper certificate:
                    // offer the unencrypted page instead of a scary certificate warning.
                    Some(orig) if host_of(&orig) == host_of(&uri) => {
                        format!("squatch://error?kind=insecure&url={}", config::url_encode(&orig))
                    }
                    _ => format!(
                        "squatch://error?kind=cert&local={}&url={}",
                        if local { 1 } else { 0 },
                        config::url_encode(&uri)
                    ),
                };
                Self::load(&t.view, &page);
            }
            UserEvent::Toast(msg) => self.toast(&msg),
            UserEvent::VideoDone(msg) => {
                self.video_busy = false;
                self.toast(&msg);
            }
        }
    }

    fn should_upgrade(&self, url: &str) -> bool {
        if !self.cfg.https_only || !url.starts_with("http://") {
            return false;
        }
        let host = host_of(url);
        !config::is_local_host(&host) && !self.http_allowed.contains(&host)
    }

    fn eval_active(&self, js: &str) {
        if let Some(t) = self.active_tab() {
            let _ = t.view.evaluate_script(js);
        }
    }

    fn navigate_active(&mut self, url: &str) {
        let active = self.active;
        if let Some(t) = self.tab_mut(active) {
            t.pending_url = None;
            Self::load(&t.view, url);
            let _ = t.view.focus();
        }
    }

    fn set_javascript(&mut self, on: bool) {
        self.js_enabled = on;
        for t in &self.tabs {
            #[cfg(target_os = "linux")]
            if let Some(s) = WebViewExt::settings(&t.view.webview()) {
                s.set_enable_javascript(on);
            }
            #[cfg(windows)]
            win::set_javascript(&t.view, on);
        }
    }

    fn toolbar_cmd(&mut self, v: &Value) {
        let cmd = v["cmd"].as_str().unwrap_or("");
        match cmd {
            "ready" => {
                self.shield_status();
                self.push_tabs();
                self.push_active();
                self.ui(json!({ "js": self.js_enabled, "https": self.cfg.https_only }));
            }
            "toggle-https" => {
                self.cfg.https_only = !self.cfg.https_only;
                #[cfg(windows)]
                {
                    self.policy.borrow_mut().https_only = self.cfg.https_only;
                }
                if let Err(e) = self.cfg.save() {
                    eprintln!("[config] save failed: {e}");
                }
                self.ui(json!({ "https": self.cfg.https_only }));
                self.toast(if self.cfg.https_only {
                    "HTTPS-only on: http links are upgraded, and you're warned before unencrypted sites"
                } else {
                    "HTTPS-only off: http sites load normally (still marked Not secure)"
                });
            }
            "navigate" => {
                let input = v["input"].as_str().unwrap_or("").trim();
                if !input.is_empty() {
                    let url = self.cfg.resolve_input(input);
                    self.navigate_active(&url);
                }
            }
            "back" => { if let Some(t) = self.active_tab() { let _ = t.view.go_back(); } }
            "forward" => { if let Some(t) = self.active_tab() { let _ = t.view.go_forward(); } }
            "reload" => { if let Some(t) = self.active_tab() { let _ = t.view.reload(); } }
            "home" => {
                let home = self.cfg.homepage.clone();
                self.navigate_active(&home);
            }
            "new-tab" => {
                let home = self.cfg.homepage.clone();
                let _ = self.new_tab(&home, true, None);
                self.ui(json!({ "focusUrl": true }));
            }
            "close-tab" => {
                let id = v["id"].as_u64().map(|x| x as TabId).unwrap_or(self.active);
                self.close_tab(id);
            }
            "select-tab" => {
                if let Some(id) = v["id"].as_u64() {
                    self.activate(id as TabId);
                }
            }
            "tab-index" => {
                let n = v["n"].as_u64().unwrap_or(1) as usize;
                // Ctrl+9 always means the last tab, like other browsers.
                let idx = if n >= 9 { self.tabs.len() - 1 } else { n.saturating_sub(1).min(self.tabs.len() - 1) };
                let id = self.tabs[idx].id;
                self.activate(id);
            }
            "next-tab" => self.cycle(1),
            "prev-tab" => self.cycle(-1),
            "bookmark" => {
                let Some((url, title)) = self.active_tab().map(|t| (t.url.clone(), t.title.clone())) else { return };
                if url.is_empty() || url.starts_with("squatch://") || url == "about:blank" {
                    return;
                }
                match self.bookmarks.toggle(&url, &title) {
                    Ok(true) => self.toast("Bookmarked"),
                    Ok(false) => self.toast("Bookmark removed"),
                    Err(e) => self.toast(&format!("Couldn't save bookmark: {e}")),
                }
                self.push_active();
                self.refresh_home_tabs();
            }
            "bookmarks" => self.open_bookmarks(),
            "reader" => self.eval_active(&format!(
                "if(!window.Readability){{(function(){{{READABILITY_JS}\n;window.Readability=Readability;}})();}}window.__squatch&&__squatch.reader();"
            )),
            "unstick" => self.eval_active("window.__squatch&&__squatch.unstick()"),
            "zap" => self.eval_active("window.__squatch&&__squatch.zap()"),
            "video" => self.eval_active("window.__squatch&&__squatch.saveMenu()"),
            "devtools" => { if let Some(t) = self.active_tab() { t.view.open_devtools(); } }
            "open-popup" => {
                if let Some(url) = v["url"].as_str() {
                    let _ = self.new_tab(url, true, None);
                }
            }
            "toggle-js" => {
                self.set_javascript(!self.js_enabled);
                if let Some(t) = self.active_tab() {
                    let _ = t.view.reload();
                }
                self.ui(json!({ "js": self.js_enabled }));
                self.toast(if self.js_enabled {
                    "Scripts on"
                } else {
                    "Scripts off: lighter and faster, but Reader, Zap, Unstick and Bookmarks need scripts"
                });
            }
            "update-lists" => self.rebuild(true),
            "edit-filters" => open_path(&self.paths.my_filters),
            "open-videos" => {
                let dir = self.cfg.video_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
            _ => eprintln!("[toolbar] unknown command {cmd}"),
        }
    }

    fn page_msg(&mut self, id: TabId, origin: &str, v: &Value) {
        let trusted = origin.starts_with("squatch://");
        match v["evt"].as_str().unwrap_or("") {
            "page" => {
                let title = v["title"].as_str().unwrap_or("").to_string();
                let videos = v["videos"].as_u64().unwrap_or(0);
                if let Some(t) = self.tab_mut(id) {
                    t.title = title;
                    t.videos = videos;
                }
                self.push_tabs();
                if id == self.active {
                    self.push_active();
                    self.set_window_title();
                }
            }
            #[cfg(windows)]
            "cosmetic" => {
                let strings = |k: &str| -> Vec<String> {
                    v[k].as_array()
                        .map(|a| a.iter().take(5000).filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default()
                };
                let engine = self.engine.borrow();
                let Some(engine) = engine.as_ref() else { return };
                if origin.starts_with("squatch://") {
                    return;
                }
                let sels = win::cosmetic_selectors(engine, origin, &strings("classes"), &strings("ids"));
                if sels.is_empty() {
                    return;
                }
                if let Some(t) = self.tab(id) {
                    let data = serde_json::to_string(&sels).unwrap_or_else(|_| "[]".into());
                    let _ = t.view.evaluate_script(&format!("window.__squatch&&__squatch.hide({data})"));
                }
            }
            "key" => {
                let key = v["key"].as_str().unwrap_or("");
                match key {
                    "focus-url" => {
                        let _ = self.toolbar.focus();
                        self.ui(json!({ "focusUrl": true }));
                    }
                    k if k.starts_with("tab-") => {
                        let n: u64 = k[4..].parse().unwrap_or(1);
                        self.toolbar_cmd(&json!({ "cmd": "tab-index", "n": n }));
                    }
                    _ => self.toolbar_cmd(&json!({ "cmd": key })),
                }
            }
            "open-tab" => {
                let Some(url) = v["url"].as_str() else { return };
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return;
                }
                // Real clicks come one at a time; a burst means a script is abusing the bridge.
                let now = Instant::now();
                if let Some(t) = self.tab_mut(id) {
                    t.opened.retain(|i| now.duration_since(*i).as_secs() < 3);
                    if t.opened.len() >= 4 {
                        return;
                    }
                    t.opened.push(now);
                }
                let background = v["background"].as_bool().unwrap_or(false);
                let _ = self.new_tab(url, !background, None);
                if background {
                    self.toast("Opened in background tab");
                }
            }
            "toast" => {
                if id == self.active {
                    self.toast(v["msg"].as_str().unwrap_or(""));
                }
            }
            "remember" => {
                // The host comes from the engine, not from the page, so a site can only add rules for itself.
                let host = host_of(origin);
                let sel = v["selector"].as_str().unwrap_or("");
                if host.is_empty() || sel.is_empty() || sel.contains('\n') || sel.len() > 500 {
                    return;
                }
                match filters::remember_cosmetic(&self.paths, &host, sel) {
                    Ok(()) => {
                        self.toast(&format!("Remembered: {host}##{sel}"));
                        self.rebuild(false);
                    }
                    Err(e) => self.toast(&format!("Could not save filter: {e}")),
                }
            }
            "videos" => {
                if self.video_busy {
                    self.toast("Already saving a video, one at a time");
                    return;
                }
                self.video_busy = true;
                let page = origin.to_string();
                let mut candidates = vec![page.clone()];
                if let Some(list) = v["list"].as_array() {
                    candidates.extend(list.iter().filter_map(|x| x.as_str().map(String::from)));
                }
                candidates.dedup();
                let mode = video::Mode::parse(v["mode"].as_str().unwrap_or(""));
                let p = self.proxy.clone();
                let dir = if mode.is_audio() { self.cfg.audio_dir() } else { self.cfg.video_dir() };
                let ytdlp = self.cfg.ytdlp_path.clone();
                self.toast(if mode.is_audio() { "Saving audio…" } else { "Saving video…" });
                std::thread::spawn(move || {
                    let say = |m: &str| {
                        let _ = p.send_event(UserEvent::Toast(m.to_string()));
                    };
                    let msg = video::save(&ytdlp, &dir, &page, &candidates, mode, &say);
                    let _ = p.send_event(UserEvent::VideoDone(msg));
                });
            }
            // ---- bookmarks page (squatch://home only) ----
            "bm-list" if trusted => self.send_bookmarks(id),
            "bm-update" if trusted => {
                let r = self.bookmarks.update(
                    v["id"].as_str().unwrap_or(""),
                    v["title"].as_str().unwrap_or(""),
                    v["folder"].as_str().unwrap_or(""),
                );
                if let Err(e) = r {
                    self.toast(&format!("Couldn't save: {e}"));
                }
                self.refresh_home_tabs();
            }
            "bm-delete" if trusted => {
                if let Err(e) = self.bookmarks.remove(v["id"].as_str().unwrap_or("")) {
                    self.toast(&format!("Couldn't delete: {e}"));
                }
                self.refresh_home_tabs();
                self.push_active();
            }
            "bm-import" if trusted => {
                let entries: Vec<(String, String, String)> = v["items"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| {
                                (
                                    x["url"].as_str().unwrap_or("").to_string(),
                                    x["title"].as_str().unwrap_or("").to_string(),
                                    x["folder"].as_str().unwrap_or("").to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self.bookmarks.import(entries) {
                    Ok(n) => self.toast(&format!("Imported {n} bookmark{}", if n == 1 { "" } else { "s" })),
                    Err(e) => self.toast(&format!("Import failed: {e}")),
                }
                self.refresh_home_tabs();
            }
            "allow-http" if trusted => {
                let url = v["url"].as_str().unwrap_or("").to_string();
                if !url.starts_with("http://") {
                    return;
                }
                let host = host_of(&url);
                #[cfg(windows)]
                self.policy.borrow_mut().http_allowed.insert(host.clone());
                self.http_allowed.insert(host);
                if let Some(t) = self.tab(id) {
                    let _ = t.view.load_url(&url);
                }
            }
            #[cfg(target_os = "linux")]
            "trust-cert" if trusted => {
                let url = v["url"].as_str().unwrap_or("").to_string();
                let host = host_of(&url);
                // Only ever for devices on the home network, never public sites.
                if !config::is_local_host(&host) {
                    return;
                }
                let cert = self.certs.borrow().get(&url).cloned();
                if let (Some(cert), Some(t)) = (cert, self.tab(id)) {
                    if let Some(ctx) = t.view.webview().context() {
                        ctx.allow_tls_certificate_for_host(&cert, &raw_host(&url));
                    }
                    let _ = t.view.load_url(&url);
                }
            }
            "home-navigate" if trusted => {
                let input = v["input"].as_str().unwrap_or("").trim().to_string();
                if !input.is_empty() {
                    let url = self.cfg.resolve_input(&input);
                    if let Some(t) = self.tab(id) {
                        Self::load(&t.view, &url);
                    }
                }
            }
            _ => {}
        }
    }

    fn send_bookmarks(&self, id: TabId) {
        if let Some(t) = self.tab(id) {
            let data = serde_json::to_string(self.bookmarks.list()).unwrap_or_else(|_| "[]".into());
            let _ = t.view.evaluate_script(&format!("window.bm && bm.render({data})"));
        }
    }

    fn refresh_home_tabs(&self) {
        for t in self.tabs.iter().filter(|t| t.url.starts_with(HOME_URL)) {
            self.send_bookmarks(t.id);
        }
    }
}

pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host.trim_start_matches("www.").to_ascii_lowercase()
}

/// Host exactly as in the URL (no port), for WebKit's certificate exceptions.
#[cfg(target_os = "linux")]
fn raw_host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    host.split(':').next().unwrap_or(host).to_string()
}

fn open_path(path: &std::path::Path) {
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(not(windows))]
    let opener = "xdg-open";
    let _ = tools::command(opener).arg(path).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_parsing() {
        assert_eq!(super::host_of("https://www.Example.com:8443/a?b#c"), "example.com");
        assert_eq!(super::host_of("http://user@site.example/x"), "site.example");
    }

    #[test]
    fn own_pages_round_trip() {
        let u = "squatch://error?kind=cert";
        assert_eq!(super::app_url(&super::nav_url(u)), u);
    }
}
