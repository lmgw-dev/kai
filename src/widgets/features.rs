//! WebKit features the widget views turn off that have no setter of their own (§10.1): looked up
//! at run time by identifier in WebKit's own feature list (`webkit_settings_get_all_features`,
//! WebKitGTK 2.42), never guessed. The Rust bindings stop at 2.40, so the calls are declared here.
//!
//! **Fail closed:** a feature of [`OFF`] that this WebKit does not list, or that stays on after it
//! was turned off, stops the widget from showing, and the error names it (a WebKit update that
//! renames one is seen, not silently ignored).

use std::ffi::{CString, c_char, c_void};

use gtk::glib::translate::ToGlibPtr;
use webkit2gtk::Settings;

/// `(identifier, why)`: each feature turned off for every widget view.
pub const OFF: &[(&str, &str)] = &[
    (
        "SpeechSynthesisAPI",
        "speechSynthesis would speak through the speakers (flite in the web process), into Kai's \
         own microphone",
    ),
    (
        "LinkPreconnect",
        "<link rel=preconnect> opens a connection without a resource load, past the content rules",
    ),
];

#[repr(C)]
struct FeatureList(c_void);
#[repr(C)]
struct Feature(c_void);

#[link(name = "webkit2gtk-4.1")]
unsafe extern "C" {
    fn webkit_settings_get_all_features() -> *mut FeatureList;
    fn webkit_feature_list_unref(list: *mut FeatureList);
    fn webkit_feature_list_find(list: *mut FeatureList, identifier: *const c_char) -> *mut Feature;
    #[cfg(test)]
    fn webkit_feature_list_get_length(list: *mut FeatureList) -> usize;
    #[cfg(test)]
    fn webkit_feature_list_get(list: *mut FeatureList, index: usize) -> *mut Feature;
    #[cfg(test)]
    fn webkit_feature_get_identifier(feature: *mut Feature) -> *const c_char;
    fn webkit_settings_set_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut Feature,
        enabled: i32,
    );
    fn webkit_settings_get_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut Feature,
    ) -> i32;
}

/// WebKit's feature list (a reference of its own).
struct List(*mut FeatureList);

impl List {
    fn all() -> Result<List, String> {
        // SAFETY: returns a new reference (or NULL on a WebKit without the API, never seen here).
        let p = unsafe { webkit_settings_get_all_features() };
        if p.is_null() {
            return Err("this WebKit lists no features (webkit_settings_get_all_features)".into());
        }
        Ok(List(p))
    }

    /// The feature named `identifier` (borrowed from the list).
    fn find(&self, identifier: &str) -> Option<*mut Feature> {
        let c = CString::new(identifier).ok()?;
        // SAFETY: a valid list and a NUL-terminated string; the result is owned by the list.
        let f = unsafe { webkit_feature_list_find(self.0, c.as_ptr()) };
        (!f.is_null()).then_some(f)
    }

    #[cfg(test)]
    fn identifiers(&self) -> Vec<String> {
        use std::ffi::CStr;
        // SAFETY: indexes below the list's length; the identifiers are owned by the features.
        unsafe {
            (0..webkit_feature_list_get_length(self.0))
                .map(|i| webkit_feature_list_get(self.0, i))
                .filter(|f| !f.is_null())
                .map(|f| {
                    CStr::from_ptr(webkit_feature_get_identifier(f))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        }
    }
}

impl Drop for List {
    fn drop(&mut self) {
        // SAFETY: the reference `all` took.
        unsafe { webkit_feature_list_unref(self.0) };
    }
}

/// Turns every feature of [`OFF`] off in `settings`, and checks that each is off. An error names
/// the feature and why the widgets need it off.
pub fn turn_off(settings: &Settings) -> Result<(), String> {
    turn_off_these(settings, OFF)
}

fn turn_off_these(settings: &Settings, off: &[(&str, &str)]) -> Result<(), String> {
    let list = List::all()?;
    let s: *mut webkit2gtk::ffi::WebKitSettings = settings.to_glib_none().0;
    for (id, why) in off {
        let Some(f) = list.find(id) else {
            return Err(format!(
                "this WebKit has no feature {id}, which the widgets turn off ({why}); Kai shows no \
                 overlay with it unknown"
            ));
        };
        // SAFETY: valid settings and a feature of the live list.
        let on = unsafe {
            webkit_settings_set_feature_enabled(s, f, 0);
            webkit_settings_get_feature_enabled(s, f) != 0
        };
        if on {
            return Err(format!(
                "WebKit's feature {id} stays on after it was turned off ({why}); Kai shows no \
                 overlay with it on"
            ));
        }
    }
    Ok(())
}

/// A `WebKitSettings` for the unit tests, made without `gtk::init` (the binding's constructor
/// insists on it; WebKit itself does not need a display for its settings).
#[cfg(test)]
pub fn bare_settings() -> Settings {
    // SAFETY: a new full reference.
    unsafe { gtk::glib::translate::from_glib_full(webkit2gtk::ffi::webkit_settings_new()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webkit2gtk::SettingsExt;

    /// One test for everything that touches WebKit: it is not thread-safe, and the test harness
    /// runs tests on threads of their own.
    #[test]
    fn a_widget_views_settings_turn_off_sound_media_webgl_fullscreen_and_the_features() {
        let ids = List::all().unwrap().identifiers();
        assert!(ids.len() > 100, "WebKit's feature list: {}", ids.len());
        for (id, _) in OFF {
            assert!(ids.iter().any(|i| i == id), "{id} is not in {ids:?}");
        }
        let s = bare_settings();
        assert!(s.enables_webaudio() && s.enables_media(), "on by default");
        crate::widgets::view::harden(&s).unwrap();
        assert!(!s.enables_webaudio() && !s.enables_media() && !s.enables_media_stream());
        assert!(s.is_media_playback_requires_user_gesture());
        assert!(!s.enables_webgl() && !s.enables_fullscreen() && !s.enables_webrtc());
        let list = List::all().unwrap();
        for (id, _) in OFF {
            let f = list.find(id).unwrap();
            // SAFETY: valid settings and a feature of the live list.
            let on = unsafe { webkit_settings_get_feature_enabled(s.to_glib_none().0, f) != 0 };
            assert!(!on, "{id} is still on");
        }
        // Fail closed: a feature this WebKit does not list stops the view.
        let e = turn_off_these(&bare_settings(), &[("NoSuchFeatureKai", "a test")]).unwrap_err();
        assert!(
            e.starts_with(
                "this WebKit has no feature NoSuchFeatureKai, which the widgets turn off"
            ),
            "{e}"
        );
    }
}
