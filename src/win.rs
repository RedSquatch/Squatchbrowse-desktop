//! Windows (WebView2) glue.
//!
//! WebView2 has no content-blocker like WebKit's, so every request is checked with
//! Brave's engine in a WebResourceRequested handler and answered with a 403 when
//! it's an ad or tracker. Cosmetic hiding goes through page.js (see `cosmetic_css`).
//! This module also reports failed loads and certificate errors, and does the
//! HTTPS-only upgrade before an http:// request leaves the machine.

use crate::{TabId, UserEvent};
use adblock::{request::Request, Engine};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use tao::event_loop::EventLoopProxy;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    take_pwstr, NavigationCompletedEventHandler, NavigationStartingEventHandler, WebResourceRequestedEventHandler,
};
use windows::core::{Interface, BOOL, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

/// The filter engine, shared by every tab. `None` until the lists are built.
pub type SharedEngine = Rc<RefCell<Option<Engine>>>;

/// HTTPS-only settings the navigation hook reads.
#[derive(Default)]
pub struct Policy {
    pub https_only: bool,
    pub http_allowed: HashSet<String>,
}
pub type SharedPolicy = Rc<RefCell<Policy>>;

/// wry serves `squatch://x` as `http://squatch.x` on Windows.
pub const INTERNAL_PREFIX: &str = "http://squatch.";

pub fn engine_from_bytes(bytes: &[u8]) -> Option<Engine> {
    let mut e = Engine::default();
    e.deserialize(bytes).ok()?;
    Some(e)
}

fn request_type(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "subdocument",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "csp_report",
        _ => "other",
    }
}

fn host(url: &str) -> String {
    crate::host_of(url)
}

/// Wire up one tab's WebView2: blocking, errors, HTTPS upgrade.
pub fn attach(view: &WebView, id: TabId, engine: SharedEngine, policy: SharedPolicy, proxy: EventLoopProxy<UserEvent>) {
    if let Err(e) = unsafe { attach_inner(view, id, engine, policy, proxy) } {
        eprintln!("[win] couldn't attach handlers: {e}");
    }
}

unsafe fn attach_inner(
    view: &WebView,
    id: TabId,
    engine: SharedEngine,
    policy: SharedPolicy,
    proxy: EventLoopProxy<UserEvent>,
) -> windows::core::Result<()> {
    let wv: ICoreWebView2 = view.webview();
    let env = wv.cast::<ICoreWebView2_2>()?.Environment()?;
    // The page the tab is on (or heading to). Main-frame loads to it are never blocked.
    let main_url: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let mut token = 0i64;

    unsafe {
        // ---- HTTPS-only upgrade, and remember where the main frame is going
        let main = main_url.clone();
        let p = proxy.clone();
        wv.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if uri.starts_with("http://") && !uri.starts_with(INTERNAL_PREFIX) {
                    let pol = policy.borrow();
                    let h = host(&uri);
                    if pol.https_only && !crate::config::is_local_host(&h) && !pol.http_allowed.contains(&h) {
                        args.SetCancel(true)?;
                        let _ = p.send_event(UserEvent::Upgrade(id, uri));
                        return Ok(());
                    }
                }
                *main.borrow_mut() = uri;
                Ok(())
            })),
            &mut token,
        )?;

        // ---- failed loads and bad certificates
        let main = main_url.clone();
        let p = proxy.clone();
        wv.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut ok = BOOL::default();
                args.IsSuccess(&mut ok)?;
                if ok.as_bool() {
                    return Ok(());
                }
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                args.WebErrorStatus(&mut status)?;
                let uri = main.borrow().clone();
                if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED || uri.is_empty() {
                    return Ok(());
                }
                let cert = [
                    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS,
                ]
                .contains(&status);
                let _ = p.send_event(if cert { UserEvent::TlsError(id, uri) } else { UserEvent::LoadFailed(id, uri) });
                Ok(())
            })),
            &mut token,
        )?;

        // ---- ad and tracker blocking
        wv.AddWebResourceRequestedFilter(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let main = main_url.clone();
        let blocked_reason = HSTRING::from("Blocked");
        let no_headers = HSTRING::new();
        wv.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let engine = engine.borrow();
                let Some(engine) = engine.as_ref() else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if !(uri.starts_with("http://") || uri.starts_with("https://")) || uri.starts_with(INTERNAL_PREFIX) {
                    return Ok(());
                }
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
                args.ResourceContext(&mut ctx)?;
                let source = main.borrow();
                // The page itself: the user asked for it.
                if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && uri == *source {
                    return Ok(());
                }
                let Ok(req) = Request::new(&uri, &source, request_type(ctx), "GET") else { return Ok(()) };
                if engine.check_network_request(&req).should_block() {
                    let resp = env.CreateWebResourceResponse(None, 403, &blocked_reason, &no_headers)?;
                    args.SetResponse(&resp)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Turn scripts on or off for one tab (takes effect on the next load).
pub fn set_javascript(view: &WebView, on: bool) {
    unsafe {
        if let Ok(s) = view.webview().Settings() {
            let _ = s.SetIsScriptEnabled(on);
        }
    }
}

/// Element-hiding rules for a page: the site-specific ones, plus generic rules that
/// match the classes and ids page.js found. Returns CSS selectors.
pub fn cosmetic_selectors(engine: &Engine, url: &str, classes: &[String], ids: &[String]) -> Vec<String> {
    let res = engine.url_cosmetic_resources(url);
    let mut out: Vec<String> = res.hide_selectors.into_iter().collect();
    if !res.generichide {
        out.extend(engine.hidden_class_id_selectors(classes, ids, &res.exceptions));
    }
    out
}
