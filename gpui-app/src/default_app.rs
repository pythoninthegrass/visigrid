//! Default application handler detection and registration.
//!
//! On macOS, this module provides functionality to:
//! - Check if VisiGrid is the default handler for spreadsheet file types
//! - Request to be set as the default handler via Launch Services
//!
//! On other platforms, these functions are no-ops.

use std::sync::atomic::{AtomicBool, Ordering};

/// Session-level flag: have we shown the prompt this session?
/// Prevents spamming if user opens multiple files without dismissing.
static SHOWN_THIS_SESSION: AtomicBool = AtomicBool::new(false);

/// File types that VisiGrid can be the default handler for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpreadsheetFileType {
    /// Excel spreadsheets (.xlsx, .xls, .xlsm, .xlsb)
    Excel,
    /// CSV files (.csv)
    Csv,
    /// Tab-separated values (.tsv)
    Tsv,
    /// Native VisiGrid format (.sheet)
    Native,
    /// Native VisiGrid format (.vgrid)
    NativeVgrid,
}

impl SpreadsheetFileType {
    /// Get the file type from a file extension.
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "xlsx" | "xls" | "xlsm" | "xlsb" => Some(Self::Excel),
            "csv" => Some(Self::Csv),
            "tsv" | "tab" => Some(Self::Tsv),
            "sheet" => Some(Self::Native),
            "vgrid" => Some(Self::NativeVgrid),
            _ => None,
        }
    }

    /// Get the UTI (Uniform Type Identifier) for this file type on macOS.
    #[cfg(target_os = "macos")]
    pub fn uti(&self) -> &'static str {
        match self {
            Self::Excel => "org.openxmlformats.spreadsheetml.sheet",
            Self::Csv => "public.comma-separated-values-text",
            Self::Tsv => "public.tab-separated-values-text",
            Self::Native => "com.visigrid.sheet",
            Self::NativeVgrid => "com.visigrid.vgrid",
        }
    }

    /// Short name for the prompt (e.g., "CSV files")
    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Excel => "Excel files",
            Self::Csv => "CSV files",
            Self::Tsv => "TSV files",
            Self::Native | Self::NativeVgrid => "VisiGrid files",
        }
    }

    /// Human-readable name for display.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Excel => "Excel spreadsheets",
            Self::Csv => "CSV files",
            Self::Tsv => "TSV files",
            Self::Native | Self::NativeVgrid => "VisiGrid files",
        }
    }
}

/// Check if we've already shown the prompt this session.
pub fn shown_this_session() -> bool {
    SHOWN_THIS_SESSION.load(Ordering::Relaxed)
}

/// Mark that we've shown the prompt this session.
pub fn mark_shown_this_session() {
    SHOWN_THIS_SESSION.store(true, Ordering::Relaxed);
}

/// Reset session state (for testing).
#[cfg(test)]
pub fn reset_session_state() {
    SHOWN_THIS_SESSION.store(false, Ordering::Relaxed);
}

/// Minimal Launch Services FFI. Replaces shelling out to third-party
/// `duti` (absent on most machines, hostile to App Sandbox) and the
/// /tmp probe heuristic. The bundle ID is read from the RUNNING app,
/// so Homebrew (com.visigrid.app) and Mac App Store (com.visigrid.mac)
/// builds both register themselves correctly.
#[cfg(target_os = "macos")]
mod ls {
    use std::ffi::{c_char, c_void, CStr, CString};

    pub type CFStringRef = *const c_void;
    type CFBundleRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    pub const ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            s: CFStringRef,
            buf: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> bool;
        pub fn CFRelease(v: *const c_void);
        fn CFBundleGetMainBundle() -> CFBundleRef;
        fn CFBundleGetIdentifier(bundle: CFBundleRef) -> CFStringRef;
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        pub fn LSCopyDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            roles: u32,
        ) -> CFStringRef;
        pub fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            roles: u32,
            handler_bundle_id: CFStringRef,
        ) -> i32;
    }

    pub fn cfstr(s: &str) -> Option<CFStringRef> {
        let c = CString::new(s).ok()?;
        let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
        (!r.is_null()).then_some(r)
    }

    pub fn to_string(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 512];
        let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
        ok.then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// Bundle ID of the running app. None when running as a bare binary
    /// (cargo run), which has no identity Launch Services would accept.
    pub fn running_bundle_id() -> Option<String> {
        unsafe {
            let bundle = CFBundleGetMainBundle();
            if bundle.is_null() {
                return None;
            }
            // Get rule: CFBundleGetIdentifier is not owned; do not release.
            to_string(CFBundleGetIdentifier(bundle))
        }
    }
}

/// Check if VisiGrid is the default handler for a file type.
///
/// Returns `true` if VisiGrid is already the default, `false` otherwise.
/// On non-macOS platforms, always returns `true` (suppresses the prompt).
#[cfg(target_os = "macos")]
pub fn is_default_handler(file_type: SpreadsheetFileType) -> bool {
    let Some(me) = ls::running_bundle_id() else {
        return true; // bare binary: suppress the prompt
    };
    let Some(uti) = ls::cfstr(file_type.uti()) else {
        return true;
    };
    let handler = unsafe { ls::LSCopyDefaultRoleHandlerForContentType(uti, ls::ROLES_ALL) };
    unsafe { ls::CFRelease(uti) };
    let current = ls::to_string(handler);
    if !handler.is_null() {
        unsafe { ls::CFRelease(handler) };
    }
    match current {
        Some(id) => id.eq_ignore_ascii_case(&me),
        // No handler registered at all: treat as "not us" so the prompt
        // can offer to claim the type.
        None => false,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn is_default_handler(_file_type: SpreadsheetFileType) -> bool {
    // On non-macOS, always return true to suppress the prompt
    true
}

/// Request to set VisiGrid as the default handler for a file type,
/// using the running app's own bundle ID via Launch Services.
///
/// Falls back to opening System Settings if Launch Services refuses.
#[cfg(target_os = "macos")]
pub fn set_as_default_handler(file_type: SpreadsheetFileType) -> Result<(), String> {
    let me = ls::running_bundle_id()
        .ok_or_else(|| "Not running from an app bundle".to_string())?;
    let uti = ls::cfstr(file_type.uti()).ok_or_else(|| "Bad UTI".to_string())?;
    let Some(bundle_id) = ls::cfstr(&me) else {
        unsafe { ls::CFRelease(uti) };
        return Err("Bad bundle id".to_string());
    };
    let status =
        unsafe { ls::LSSetDefaultRoleHandlerForContentType(uti, ls::ROLES_ALL, bundle_id) };
    unsafe {
        ls::CFRelease(uti);
        ls::CFRelease(bundle_id);
    }
    if status == 0 {
        return Ok(());
    }
    // Launch Services refused (rare): send the user to System Settings.
    let _ = std::process::Command::new("open")
        .args(["x-apple.systempreferences:com.apple.ExtensionsPreferences"])
        .spawn();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set_as_default_handler(_file_type: SpreadsheetFileType) -> Result<(), String> {
    // On non-macOS, this is a no-op
    Ok(())
}

/// Check if a file path looks like a temporary file (skip prompts for these).
pub fn is_temporary_file(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();

    // Common temp directories
    if path_str.contains("/tmp/")
        || path_str.contains("/var/folders/")
        || path_str.contains("/.Trash/")
        || path_str.contains("/Temp/")
        || path_str.contains("\\Temp\\")
    {
        return true;
    }

    // Files with temp-like names
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with("~$")  // Excel temp files
            || name.starts_with("._")  // macOS resource forks
            || name.ends_with(".tmp")
            || name.ends_with(".temp")
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_from_ext() {
        assert_eq!(SpreadsheetFileType::from_ext("xlsx"), Some(SpreadsheetFileType::Excel));
        assert_eq!(SpreadsheetFileType::from_ext("XLSX"), Some(SpreadsheetFileType::Excel));
        assert_eq!(SpreadsheetFileType::from_ext("csv"), Some(SpreadsheetFileType::Csv));
        assert_eq!(SpreadsheetFileType::from_ext("vgrid"), Some(SpreadsheetFileType::NativeVgrid));
        assert_eq!(SpreadsheetFileType::from_ext("sheet"), Some(SpreadsheetFileType::Native));
        assert_eq!(SpreadsheetFileType::from_ext("txt"), None);
    }

    #[test]
    fn test_is_temporary_file() {
        use std::path::Path;

        assert!(is_temporary_file(Path::new("/tmp/test.xlsx")));
        assert!(is_temporary_file(Path::new("/var/folders/ab/cd/T/test.csv")));
        assert!(is_temporary_file(Path::new("~$Budget.xlsx")));
        assert!(is_temporary_file(Path::new("/Users/me/.Trash/old.xlsx")));

        assert!(!is_temporary_file(Path::new("/Users/me/Documents/Budget.xlsx")));
        assert!(!is_temporary_file(Path::new("/home/user/data.csv")));
    }
}
