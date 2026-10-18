//! Proxy configuration from GNOME 3 GSettings, loaded dynamically at run-time.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use libloading::Library;

use super::ConfigBackend;

const PROXY_SCHEMA: &str = "org.gnome.system.proxy";

type GFree = unsafe extern "C" fn(*mut c_void);
type GStrfreev = unsafe extern "C" fn(*mut *mut c_char);
type GObjectUnref = unsafe extern "C" fn(*mut c_void);
type GSettingsNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GSettingsGetString = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
type GSettingsGetInt = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type GSettingsGetStrv = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut *mut c_char;
type GSettingsGetBoolean = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type GSettingsSchemaSourceGetDefault = unsafe extern "C" fn() -> *mut c_void;
type GSettingsSchemaSourceLookup = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> *mut c_void;
type GSettingsSchemaUnref = unsafe extern "C" fn(*mut c_void);

pub(super) struct Gnome3Config {
    // Libraries must outlive the function pointers resolved from them
    _glib: Library,
    _gio: Library,
    g_free: GFree,
    g_strfreev: GStrfreev,
    g_object_unref: GObjectUnref,
    g_settings_new: GSettingsNew,
    g_settings_get_string: GSettingsGetString,
    g_settings_get_int: GSettingsGetInt,
    g_settings_get_strv: GSettingsGetStrv,
    g_settings_get_boolean: GSettingsGetBoolean,
    g_settings_schema_source_get_default: GSettingsSchemaSourceGetDefault,
    g_settings_schema_source_lookup: GSettingsSchemaSourceLookup,
    g_settings_schema_unref: GSettingsSchemaUnref,
}

// Owned GSettings object
struct Settings<'a> {
    api: &'a Gnome3Config,
    settings: *mut c_void,
}

impl Settings<'_> {
    fn string(&self, key: &str) -> Option<String> {
        let key = CString::new(key).ok()?;
        // SAFETY: The settings object is valid and the returned string is owned by the caller.
        unsafe {
            let value = (self.api.g_settings_get_string)(self.settings, key.as_ptr());
            if value.is_null() {
                return None;
            }
            let string = CStr::from_ptr(value).to_string_lossy().into_owned();
            (self.api.g_free)(value as *mut c_void);
            Some(string)
        }
    }

    fn int(&self, key: &str) -> i32 {
        let Ok(key) = CString::new(key) else {
            return 0;
        };
        // SAFETY: The settings object is valid.
        unsafe { (self.api.g_settings_get_int)(self.settings, key.as_ptr()) }
    }

    fn boolean(&self, key: &str) -> bool {
        let Ok(key) = CString::new(key) else {
            return false;
        };
        // SAFETY: The settings object is valid.
        unsafe { (self.api.g_settings_get_boolean)(self.settings, key.as_ptr()) != 0 }
    }

    fn strings(&self, key: &str) -> Vec<String> {
        let Ok(key) = CString::new(key) else {
            return Vec::new();
        };
        let mut strings = Vec::new();
        // SAFETY: The settings object is valid and the returned array is null-terminated and owned
        // by the caller.
        unsafe {
            let values = (self.api.g_settings_get_strv)(self.settings, key.as_ptr());
            if values.is_null() {
                return strings;
            }
            let mut current = values;
            while !(*current).is_null() {
                strings.push(CStr::from_ptr(*current).to_string_lossy().into_owned());
                current = current.add(1);
            }
            (self.api.g_strfreev)(values);
        }
        strings
    }
}

impl Drop for Settings<'_> {
    fn drop(&mut self) {
        // SAFETY: The settings object is owned by this wrapper.
        unsafe { (self.api.g_object_unref)(self.settings) };
    }
}

impl Gnome3Config {
    pub(super) fn new() -> Option<Self> {
        // SAFETY: The libraries are loaded by soname and the symbols are resolved with the
        // signatures documented by GLib and GIO.
        let config = unsafe {
            let glib = Library::new("libglib-2.0.so.0").ok()?;
            let gio = Library::new("libgio-2.0.so.0").ok()?;
            Gnome3Config {
                g_free: *glib.get(b"g_free\0").ok()?,
                g_strfreev: *glib.get(b"g_strfreev\0").ok()?,
                g_object_unref: *gio.get(b"g_object_unref\0").ok()?,
                g_settings_new: *gio.get(b"g_settings_new\0").ok()?,
                g_settings_get_string: *gio.get(b"g_settings_get_string\0").ok()?,
                g_settings_get_int: *gio.get(b"g_settings_get_int\0").ok()?,
                g_settings_get_strv: *gio.get(b"g_settings_get_strv\0").ok()?,
                g_settings_get_boolean: *gio.get(b"g_settings_get_boolean\0").ok()?,
                g_settings_schema_source_get_default: *gio.get(b"g_settings_schema_source_get_default\0").ok()?,
                g_settings_schema_source_lookup: *gio.get(b"g_settings_schema_source_lookup\0").ok()?,
                g_settings_schema_unref: *gio.get(b"g_settings_schema_unref\0").ok()?,
                _glib: glib,
                _gio: gio,
            }
        };
        if !config.has_schema(PROXY_SCHEMA) {
            log::debug!("GSettings schema {PROXY_SCHEMA} is not installed");
            return None;
        }
        Some(config)
    }

    // g_settings_new aborts the process if the schema is not installed
    fn has_schema(&self, schema_id: &str) -> bool {
        let Ok(schema_id) = CString::new(schema_id) else {
            return false;
        };
        // SAFETY: The default schema source is owned by GIO and the looked up schema is owned by
        // the caller.
        unsafe {
            let source = (self.g_settings_schema_source_get_default)();
            if source.is_null() {
                return false;
            }
            let schema = (self.g_settings_schema_source_lookup)(source, schema_id.as_ptr(), 1);
            if schema.is_null() {
                return false;
            }
            (self.g_settings_schema_unref)(schema);
        }
        true
    }

    fn settings(&self, schema_id: &str) -> Option<Settings<'_>> {
        if !self.has_schema(schema_id) {
            return None;
        }
        let schema_id = CString::new(schema_id).ok()?;
        // SAFETY: The schema exists so g_settings_new returns a new settings object.
        let settings = unsafe { (self.g_settings_new)(schema_id.as_ptr()) };
        (!settings.is_null()).then_some(Settings { api: self, settings })
    }

    fn is_mode(&self, mode: &str) -> bool {
        self.settings(PROXY_SCHEMA)
            .and_then(|settings| settings.string("mode"))
            .is_some_and(|system_mode| system_mode == mode)
    }
}

impl ConfigBackend for Gnome3Config {
    fn auto_discover(&self) -> bool {
        self.is_mode("auto")
    }

    fn auto_config_url(&self) -> Option<String> {
        if !self.is_mode("auto") {
            return None;
        }
        self.settings(PROXY_SCHEMA)?
            .string("autoconfig-url")
            .filter(|url| !url.is_empty())
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        if !self.is_mode("manual") {
            return None;
        }
        let use_same_proxy = self.settings(PROXY_SCHEMA)?.boolean("use-same-proxy");
        let scheme = if use_same_proxy { "http" } else { scheme };
        let settings = self.settings(&format!("{PROXY_SCHEMA}.{scheme}"))?;
        let host = settings.string("host").filter(|host| !host.is_empty())?;
        match settings.int("port") {
            0 => Some(host),
            port => Some(format!("{host}:{port}")),
        }
    }

    fn bypass_list(&self) -> Option<String> {
        if !self.is_mode("manual") {
            return None;
        }
        let hosts: Vec<String> = self
            .settings(PROXY_SCHEMA)?
            .strings("ignore-hosts")
            .into_iter()
            .filter(|host| !host.is_empty())
            .collect();
        (!hosts.is_empty()).then(|| hosts.join(","))
    }
}
