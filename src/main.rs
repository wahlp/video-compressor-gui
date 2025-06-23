mod app;
use app::MyApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Video Compressor",
        native_options,
        Box::new(|creation_context| {
            creation_context.egui_ctx.set_theme(egui::Theme::Dark);
            Ok(Box::new(MyApp::load().unwrap()))
        })
    )
}
