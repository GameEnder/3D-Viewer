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
            .with_inner_size([1280.0, 800.0]),
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
