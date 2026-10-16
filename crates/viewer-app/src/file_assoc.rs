//! Windows file association support (registry-based, `HKCU` only — no admin
//! rights required). Registers the app as the default handler for 3D model
//! file types so double-clicking a .glb / .obj / etc. opens it in 3D Viewer.
//!
//! Registry layout:
//! ```text
//! HKCU\Software\Classes\3DViewer.Rust           (ProgID)
//!   (Default)                                   = "3D Viewer Model"
//!   DefaultIcon                                 = "<exe>",0
//!   shell\open\command                           = "<exe>" "%1"
//!
//! HKCU\Software\Classes\.glb\OpenWithProgids    = "3DViewer.Rust"
//! HKCU\Software\Classes\.gltf\OpenWithProgids  = "3DViewer.Rust"
//! ... (one per supported extension)
//! ```

#[cfg(windows)]
pub mod imp {
    use winreg::RegKey;
    use winreg::enums::*;

    /// Our ProgID — the registry identity of this application.
    pub const PROG_ID: &str = "3DViewer.Rust";
    pub const PROG_DESCRIPTION: &str = "3D Viewer Model";

    /// The extensions we support (without the leading dot).
    pub const EXTENSIONS: [&str; 8] = ["glb", "gltf", "obj", "stl", "ply", "3mf", "fbx", "nif"];

    /// Absolute path to the running executable (for the open command).
    pub fn exe_path() -> String {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Write the ProgID key: description, icon, friendly app name, and the
    /// open command. The `FriendlyAppName` is what Windows shows in Settings
    /// → Default Apps and in file Properties → "Opens with"; without it
    /// Windows falls back to the exe filename (viewer.exe).
    fn write_progid(classes: &RegKey) -> Result<(), String> {
        let (progid, _) = classes
            .create_subkey(PROG_ID)
            .map_err(|e| format!("create ProgID key: {e}"))?;
        progid
            .set_value("", &PROG_DESCRIPTION)
            .map_err(|e| format!("set description: {e}"))?;
        progid
            .set_value("DefaultIcon", &format!("{},0", exe_path()))
            .map_err(|e| format!("set icon: {e}"))?;
        progid
            .set_value("FriendlyAppName", &"3D Viewer")
            .map_err(|e| format!("set friendly name: {e}"))?;

        let (open, _) = progid
            .create_subkey(r"shell\open")
            .map_err(|e| format!("create open key: {e}"))?;
        open.set_value("", &"&Open with 3D Viewer")
            .map_err(|e| format!("set open label: {e}"))?;

        let (cmd, _) = progid
            .create_subkey(r"shell\open\command")
            .map_err(|e| format!("create open command key: {e}"))?;
        cmd.set_value("", &format!("\"{}\" \"%1\"", exe_path()))
            .map_err(|e| format!("set open command: {e}"))?;
        Ok(())
    }

    /// Delete the ProgID key and all extension references.
    fn delete_progid(classes: &RegKey) -> Result<(), String> {
        let _ = classes.delete_subkey_all(PROG_ID);
        Ok(())
    }

    /// Register all extensions to open with our ProgID.
    /// Returns the number of extensions successfully registered.
    pub fn register() -> Result<usize, String> {
        clear_userchoice_overrides();
        let count = write_all_extensions()?;
        notify_explorer();
        Ok(count)
    }

    /// Remove all registrations (ProggID + extension references).
    /// Returns the number of extensions successfully unregistered.
    pub fn unregister() -> Result<usize, String> {
        let count = clear_all_extensions()?;
        notify_explorer();
        Ok(count)
    }

    /// Delete the UserChoice override for each extension. Windows writes
    /// UserChoice when the user picks a default app in Settings; it takes
    /// priority over `Classes\.ext\Default`, so we must clear it to become
    /// the default handler.
    fn clear_userchoice_overrides() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let base = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts";
        for ext in EXTENSIONS {
            let uc_path = format!("{base}\\.{ext}\\UserChoice");
            if let Ok(key) = hkcu.open_subkey_with_flags(&uc_path, KEY_SET_VALUE) {
                let _ = key.delete_value("ProgId");
                let _ = key.delete_value("Hash");
            }
        }
    }

    /// Write the Classes entries for all extensions (Default + OpenWithProgids).
    fn write_all_extensions() -> Result<usize, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu
            .open_subkey_with_flags("Software\\Classes", KEY_SET_VALUE | KEY_CREATE_SUB_KEY)
            .map_err(|e| format!("open Classes key: {e}"))?;

        write_progid(&classes)?;

        let mut count = 0;
        for ext in EXTENSIONS {
            // Set the extension's Default to our ProgID — this is what
            // makes Windows treat us as the DEFAULT handler.
            let (ext_key, _) = classes
                .create_subkey(format!(".{ext}"))
                .map_err(|e| format!("create .{ext} key: {e}"))?;
            ext_key
                .set_value("", &PROG_ID)
                .map_err(|e| format!("set .{ext} default: {e}"))?;

            // Also register in OpenWithProgids (adds us to the context-menu
            // "Open with" list, independent of the default assignment).
            let (key, _) = classes
                .create_subkey(format!(".{ext}\\OpenWithProgids"))
                .map_err(|e| format!("create .{ext}\\OpenWithProgids: {e}"))?;
            key.set_value(PROG_ID, &"")
                .map_err(|e| format!("set .{ext}\\OpenWithProgids: {e}"))?;
            count += 1;
        }
        Ok(count)
    }

    /// Clear all extension entries (Default + OpenWithProgids) and the ProgID.
    fn clear_all_extensions() -> Result<usize, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu
            .open_subkey_with_flags("Software\\Classes", KEY_SET_VALUE | KEY_CREATE_SUB_KEY)
            .map_err(|e| format!("open Classes key: {e}"))?;

        let mut count = 0;
        for ext in EXTENSIONS {
            // Clear the extension's Default (removes us as default handler).
            if let Ok(key) = classes.open_subkey_with_flags(format!(".{ext}"), KEY_SET_VALUE)
                && key.get_value::<String, _>("").is_ok_and(|v| v == PROG_ID)
            {
                let _ = key.delete_value("");
            }
            // Remove the OpenWithProgids entry.
            let key_path = format!(".{ext}\\OpenWithProgids");
            if let Ok(key) = classes.open_subkey_with_flags(&key_path, KEY_SET_VALUE) {
                let _ = key.delete_value(PROG_ID);
                count += 1;
            }
        }
        delete_progid(&classes)?;
        Ok(count)
    }

    /// Tell Explorer that file associations have changed (SHCNE_ASSOCCHANGED).
    /// Without this, Explorer keeps using the cached association until it is
    /// restarted.
    ///
    /// Direct FFI: `SHChangeNotify(LONG, UINT, LPCITEMIDLIST, LPCITEMIDLIST)`
    /// is not a valid rundll32 entry point (rundll32 requires
    /// `CALLBACK(HWND, HINSTANCE, LPSTR, int)`), so the old
    /// `rundll32 shell32.dll,SHChangeNotify ...` trick silently did nothing.
    /// The signature has been frozen since Windows 95.
    fn notify_explorer() {
        const SHCNE_ASSOCCHANGED: u32 = 0x0800_0000;
        const SHCNF_FLUSH: u32 = 0x0000_1000;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHChangeNotify(wEventId: u32, uFlags: u32, dwItem1: usize, dwItem2: usize);
        }
        // SAFETY: passing null pidls (required for ASSOCCHANGED) to a
        // thread-safe Shell API. The `usize` pair encodes the null pointers
        // without materializing raw pointer types in the extern signature.
        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_FLUSH, 0, 0);
        }
    }

    /// Which application does Windows' per-user "UserChoice" protect as the
    /// default for this extension? Windows writes this when the user picks a
    /// default (Settings → Default Apps or "Open with → Always") and it takes
    /// priority over everything in `Software\Classes`. It is hash-protected:
    /// we cannot set or clear it programmatically (by design, and attempts
    /// are denied for keys Windows signed).
    pub fn user_choice(ext: &str) -> Option<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let base = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts";
        let key = hkcu
            .open_subkey(format!(r"{base}\.{ext}\UserChoice"))
            .ok()?;
        let prog_id: String = key.get_value("ProgId").ok()?;
        if prog_id.is_empty() {
            None
        } else {
            Some(prog_id)
        }
    }

    /// The handler Windows will ACTUALLY use when double-clicking `.{ext}`:
    /// the UserChoice override if present, otherwise the Classes default.
    /// `default_handler` alone overstates our position when a UserChoice
    /// points elsewhere.
    pub fn effective_handler(ext: &str) -> Option<String> {
        user_choice(ext).or_else(|| default_handler(ext))
    }

    /// Check whether our ProgID is currently registered.
    pub fn is_registered() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey(format!("Software\\Classes\\{PROG_ID}"))
            .is_ok()
    }

    /// Which application is currently the default for the given extension?
    /// Returns the ProgID or class name, or None if unregistered.
    pub fn default_handler(ext: &str) -> Option<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey(format!("Software\\Classes\\.{ext}"))
            .ok()?;
        let default: String = key.get_value("").ok()?;
        if default.is_empty() {
            None
        } else {
            Some(default)
        }
    }
}

#[cfg(not(windows))]
pub mod imp {
    /// File association is not supported on non-Windows platforms.
    pub const PROG_ID: &str = "3DViewer.Rust";
    pub const EXTENSIONS: [&str; 8] = ["glb", "gltf", "obj", "stl", "ply", "3mf", "fbx", "nif"];
    pub fn register() -> Result<usize, String> {
        Err("File associations are only supported on Windows".into())
    }
    pub fn unregister() -> Result<usize, String> {
        Err("File associations are only supported on Windows".into())
    }
    pub fn is_registered() -> bool {
        false
    }
    pub fn default_handler(_ext: &str) -> Option<String> {
        None
    }
    pub fn user_choice(_ext: &str) -> Option<String> {
        None
    }
    pub fn effective_handler(_ext: &str) -> Option<String> {
        None
    }
    pub fn exe_path() -> String {
        String::new()
    }
}

pub use imp::{
    EXTENSIONS, PROG_ID, effective_handler, is_registered, register, unregister, user_choice,
};
