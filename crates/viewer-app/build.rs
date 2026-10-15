fn main() {
    // Embed the .ico into viewer.exe on Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::path::Path::new("resources/app.ico").exists()
        && let Err(e) = winresource::WindowsResource::new()
            .set_icon("resources/app.ico")
            .set("FileDescription", "3D Viewer")
            .compile()
    {
        eprintln!("warning: failed to embed exe icon: {e}");
    }
}
