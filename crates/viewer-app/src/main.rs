// crates/viewer-app/src/main.rs
// Windows GUI subsystem in release builds — no console window appears
// when the exe is launched from Explorer or a shortcut. Debug builds
// keep the console so stderr/stdout stay visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod file_assoc;
mod paint;
mod panels;
mod themes;
mod user_choice;

fn main() -> eframe::Result<()> {
    // Command-line file argument: when Explorer double-click-opens a model
    // file with this app, the path arrives as argv[1].
    let initial_file = std::env::args()
        .nth(1)
        .filter(|s| {
            // Only accept an argument that is an existing file (not a flag).
            let p = std::path::Path::new(s);
            p.is_file()
        })
        .map(std::path::PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("3D Viewer")
            .with_inner_size([1280.0, 800.0])
            // Title-bar / taskbar icon: the same app.ico that build.rs
            // embeds into the exe, so the window always matches the file.
            .with_icon(load_window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "3D Viewer",
        options,
        Box::new(|cc| {
            Ok(Box::new(
                app::ViewerApp::new(cc).with_initial_file(initial_file),
            ))
        }),
    )
}

/// Decode the embedded app.ico into egui RGBA icon data. Failures degrade
/// to the platform default icon (with a console note in debug builds)
/// rather than blocking startup.
fn load_window_icon() -> eframe::egui::IconData {
    match app_icon_rgba() {
        Ok((rgba, width, height)) => eframe::egui::IconData {
            rgba,
            width,
            height,
        },
        Err(e) => {
            eprintln!("warning: cannot decode app.ico for the window icon: {e}");
            eframe::egui::IconData {
                rgba: vec![0; 4],
                width: 1,
                height: 1,
            }
        }
    }
}

/// Shared decode path (also exercised by the unit test below): the ICO
/// embedded at compile time, decoded to RGBA8.
fn app_icon_rgba() -> Result<(Vec<u8>, u32, u32), image::ImageError> {
    let bytes = include_bytes!("../resources/app.ico");
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok((img.into_raw(), w, h))
}

#[cfg(test)]
mod tests {
    /// The embedded ico must decode to real pixels (title bar + exe icon
    /// stay in sync because both come from the same file).
    #[test]
    fn embedded_app_ico_decodes() {
        let (rgba, w, h) = super::app_icon_rgba().expect("app.ico decodes");
        assert!(w > 0 && h > 0, "decoded icon has size {w}x{h}");
        assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA stride mismatch");
        // Not fully transparent (a 0-alpha decode would show a blank icon).
        assert!(
            rgba.iter().skip(3).any(|&a| a != 0),
            "icon is fully transparent"
        );
    }
}
