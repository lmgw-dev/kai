//! The widgets' content filter (§10.1's load refusals, [`super::sandbox::FILTER_RULES`]): WebKit's
//! content blocker, compiled once per run into a store under `$XDG_RUNTIME_DIR/kai/`, and added to
//! every widget view's content manager. It works in the web process, before a load leaves it,
//! whatever the page's CSP says. The Rust bindings lack the filter API, so it is called through
//! `webkit2gtk::ffi`.

use std::path::{Path, PathBuf};

use gtk::glib;
use gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use webkit2gtk::UserContentManager;
use webkit2gtk::ffi;

use super::sandbox::{FILTER_RULES, filter_id};

/// A compiled filter (a reference of its own).
pub struct Filter(*mut ffi::WebKitUserContentFilter);

impl Filter {
    /// Adds it to `ucm`: every load of that manager's views goes through it.
    pub fn add_to(&self, ucm: &UserContentManager) {
        // SAFETY: both pointers are valid; the manager takes its own reference to the filter.
        unsafe { ffi::webkit_user_content_manager_add_filter(ucm.to_glib_none().0, self.0) };
    }
}

impl Drop for Filter {
    fn drop(&mut self) {
        // SAFETY: the reference `compile` handed over (`save_finish` returns a full one).
        unsafe { ffi::webkit_user_content_filter_unref(self.0) };
    }
}

/// Where the compiled filter is kept: Kai's runtime directory (RAM, gone at logout).
pub fn store_dir() -> PathBuf {
    glib::user_runtime_dir().join("kai").join("widget-filter")
}

type Done = Box<dyn FnOnce(Result<Filter, String>)>;

/// Compiles [`FILTER_RULES`] into a store in `dir` and calls `done` on the GTK thread with the
/// filter, or why it failed.
pub fn compile(dir: &Path, done: impl FnOnce(Result<Filter, String>) + 'static) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        done(Err(format!("cannot make {}: {e}", dir.display())));
        return;
    }
    let path = dir.to_string_lossy().into_owned();
    let id = filter_id();
    let bytes = glib::Bytes::from_static(FILTER_RULES.as_bytes());
    let data: Box<(Done, *mut ffi::WebKitUserContentFilterStore)>;
    // SAFETY: `store_new` returns a new reference, kept in `data` until the callback releases it;
    // `save` copies the identifier and takes its own reference to the bytes; `data` is handed to
    // the callback exactly once.
    unsafe {
        let path_ptr: glib::translate::Stash<*const std::ffi::c_char, str> =
            path.as_str().to_glib_none();
        let store = ffi::webkit_user_content_filter_store_new(path_ptr.0);
        data = Box::new((Box::new(done), store));
        let id_ptr: glib::translate::Stash<*const std::ffi::c_char, str> =
            id.as_str().to_glib_none();
        ffi::webkit_user_content_filter_store_save(
            store,
            id_ptr.0,
            bytes.to_glib_none().0,
            std::ptr::null_mut(),
            Some(saved),
            Box::into_raw(data) as glib::ffi::gpointer,
        );
    }
}

unsafe extern "C" fn saved(
    source: *mut glib::gobject_ffi::GObject,
    result: *mut gtk::gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    // SAFETY: `data` is the box `compile` leaked, handed here once; `source` is the store.
    unsafe {
        let data = Box::from_raw(data as *mut (Done, *mut ffi::WebKitUserContentFilterStore));
        let (done, store) = *data;
        let mut error = std::ptr::null_mut();
        let filter = ffi::webkit_user_content_filter_store_save_finish(
            source as *mut ffi::WebKitUserContentFilterStore,
            result,
            &mut error,
        );
        glib::gobject_ffi::g_object_unref(store as *mut glib::gobject_ffi::GObject);
        if filter.is_null() {
            let e = if error.is_null() {
                "no error given".to_owned()
            } else {
                glib::Error::from_glib_full(error).to_string()
            };
            done(Err(e));
        } else {
            done(Ok(Filter(filter)));
        }
    }
}
