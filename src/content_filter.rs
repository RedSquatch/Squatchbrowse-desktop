//! Thin FFI wrapper around WebKitUserContentFilterStore.
//!
//! The safe webkit2gtk-rs bindings skip this API, so we call webkit2gtk-sys
//! directly. Everything here runs on the GTK main thread; the async callbacks
//! are dispatched by the GLib main loop that tao already runs.

use glib::translate::ToGlibPtr;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::rc::Rc;
use webkit2gtk::UserContentManager;
use webkit2gtk_sys as ffi;

pub type Callback = Rc<dyn Fn(Result<String, String>)>;

pub struct FilterStore {
    store: *mut ffi::WebKitUserContentFilterStore,
}

struct CbData {
    manager: Option<UserContentManager>,
    id: String,
    on_done: Callback,
}

impl FilterStore {
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let c = CString::new(dir.to_string_lossy().as_bytes()).expect("path has NUL");
        // The store lives for the whole program; it is intentionally never freed.
        let store = unsafe { ffi::webkit_user_content_filter_store_new(c.as_ptr()) };
        FilterStore { store }
    }

    /// Load a previously compiled filter and attach it to `manager`.
    pub fn load(&self, id: &str, manager: &UserContentManager, on_done: Callback) {
        let cid = CString::new(id).unwrap();
        let data = Box::new(CbData { manager: Some(manager.clone()), id: id.to_string(), on_done });
        unsafe {
            ffi::webkit_user_content_filter_store_load(
                self.store,
                cid.as_ptr(),
                std::ptr::null_mut(),
                Some(load_ready),
                Box::into_raw(data) as glib_sys::gpointer,
            );
        }
    }

    /// Compile `json` (Safari content-blocker rules) and store it under `id`.
    /// Attach it to webviews afterwards with `load`.
    pub fn save(&self, id: &str, json: String, on_done: Callback) {
        let cid = CString::new(id).unwrap();
        let bytes = glib::Bytes::from_owned(json.into_bytes());
        let data = Box::new(CbData { manager: None, id: id.to_string(), on_done });
        unsafe {
            ffi::webkit_user_content_filter_store_save(
                self.store,
                cid.as_ptr(),
                bytes.to_glib_none().0,
                std::ptr::null_mut(),
                Some(save_ready),
                Box::into_raw(data) as glib_sys::gpointer,
            );
        }
    }
}

unsafe extern "C" fn load_ready(
    source: *mut gobject_sys::GObject,
    res: *mut gio_sys::GAsyncResult,
    user_data: glib_sys::gpointer,
) {
    let mut err: *mut glib_sys::GError = std::ptr::null_mut();
    unsafe {
        let filter = ffi::webkit_user_content_filter_store_load_finish(source as *mut _, res, &mut err);
        finish(filter, err, user_data);
    }
}

unsafe extern "C" fn save_ready(
    source: *mut gobject_sys::GObject,
    res: *mut gio_sys::GAsyncResult,
    user_data: glib_sys::gpointer,
) {
    let mut err: *mut glib_sys::GError = std::ptr::null_mut();
    unsafe {
        let filter = ffi::webkit_user_content_filter_store_save_finish(source as *mut _, res, &mut err);
        finish(filter, err, user_data);
    }
}

unsafe fn finish(
    filter: *mut ffi::WebKitUserContentFilter,
    err: *mut glib_sys::GError,
    user_data: glib_sys::gpointer,
) {
    unsafe {
        let data = Box::from_raw(user_data as *mut CbData);
        if filter.is_null() {
            let msg = if err.is_null() {
                "unknown error".to_string()
            } else {
                let m = CStr::from_ptr((*err).message).to_string_lossy().into_owned();
                glib_sys::g_error_free(err);
                m
            };
            (data.on_done)(Err(format!("{}: {}", data.id, msg)));
            return;
        }
        if let Some(manager) = &data.manager {
            let mgr: *mut ffi::WebKitUserContentManager = manager.to_glib_none().0;
            ffi::webkit_user_content_manager_add_filter(mgr, filter);
        }
        ffi::webkit_user_content_filter_unref(filter);
        (data.on_done)(Ok(data.id.clone()));
    }
}

