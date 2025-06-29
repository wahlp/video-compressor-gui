mod app;
mod utils;
mod types;
use app::MyApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Video Compressor",
        native_options,
        Box::new(|_creation_context| {
            Ok(Box::new(MyApp::load().unwrap()))
        })
    )
}
