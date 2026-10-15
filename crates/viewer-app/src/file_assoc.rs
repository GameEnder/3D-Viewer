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
    pub const EXTENSIONS: [&str; 7] = ["glb", "gltf", "obj", "stl", "ply", "3mf", "fbx"];

    /// Absolute path to the running executable (for the open command).
    pub fn exe_path() -> String {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Write the ProgID key: description, icon, and open command.
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

        let (shell, _) = progid
            .create_subkey(r"shell\open\command")
            .map_err(|e| format!("create open command key: {e}"))?;
        shell
            .set_value("", &format!("\"{}\" \"%1\"", exe_path()))
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
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu
            .open_subkey_with_flags("Software\\Classes", KEY_SET_VALUE | KEY_CREATE_SUB_KEY)
            .map_err(|e| format!("open Classes key: {e}"))?;

        write_progid(&classes)?;

        let mut count = 0;
        for ext in EXTENSIONS {
            let key_path = format!(".{ext}\\OpenWithProgids");
            let (key, _) = classes
                .create_subkey(&key_path)
                .map_err(|e| format!("create .{ext}: {e}"))?;
            key.set_value(PROG_ID, &"")
                .map_err(|e| format!("set .{ext}: {e}"))?;
            count += 1;
        }
        Ok(count)
    }

    /// Remove all registrations (ProgID + extension references).
    /// Returns the number of extensions successfully unregistered.
    pub fn unregister() -> Result<usize, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu
            .open_subkey_with_flags("Software\\Classes", KEY_SET_VALUE | KEY_CREATE_SUB_KEY)
            .map_err(|e| format!("open Classes key: {e}"))?;

        let mut count = 0;
        for ext in EXTENSIONS {
            let key_path = format!(".{ext}\\OpenWithProgids");
            if let Ok(key) = classes.open_subkey_with_flags(&key_path, KEY_SET_VALUE) {
                let _ = key.delete_value(PROG_ID);
                count += 1;
            }
        }
        delete_progid(&classes)?;
        Ok(count)
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
    pub const EXTENSIONS: [&str; 0] = [];
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
    pub fn exe_path() -> String {
        String::new()
    }
}

pub use imp::{EXTENSIONS, PROG_ID, default_handler, is_registered, register, unregister};
