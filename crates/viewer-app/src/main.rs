// crates/viewer-app/src/main.rs
mod app;
mod camera;
mod paint;
mod panels;
mod themes;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("3D Viewer")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "3D Viewer",
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc)))),
    )
}
