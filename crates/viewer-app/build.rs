//! Build script for the viewer binary: on Windows, embeds the app icon and
//! version resources into viewer.exe (no-op elsewhere). A build script (not
//! a post-build step) because winresource must run before the exe is
//! finalized by the linker.

fn main() {
    // Embed the .ico + version info into viewer.exe on Windows. The version
    // resources (ProductName, FileDescription, etc.) are what Windows shows
    // in file Properties and in Settings → Default Apps when resolving the
    // app behind a ProgID.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if !std::path::Path::new("resources/app.ico").exists() {
            return;
        }
        let mut res = winresource::WindowsResource::new();
        res.set_icon("resources/app.ico")
            .set("FileDescription", "3D Viewer")
            .set("ProductName", "3D Viewer")
            .set("OriginalFilename", "viewer.exe")
            .set("LegalCopyright", "")
            .set("ProductVersion", "1.0.0")
            .set("FileVersion", "1.0.0.0");
        if let Err(e) = res.compile() {
            eprintln!("warning: failed to embed exe resources: {e}");
        }
    }
}
