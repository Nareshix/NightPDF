mod theme;
mod types;
mod viewer;
mod app;

use viewer::PdfViewer;

fn main() -> eframe::Result<()> {
    env_logger::init();
    eframe::run_native(
        "PDF Dark Reader",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1000.0, 860.0])
                .with_min_inner_size([600.0, 400.0])
                .with_title("PDF Dark Reader"),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(PdfViewer::new()))),
    )
}