mod app;
mod theme;
mod types;
mod viewer;

use viewer::PdfViewer;

fn main() -> eframe::Result<()> {
    // env_logger::init();
    eframe::run_native(
        "NightPDF",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1000.0, 860.0])
                .with_min_inner_size([600.0, 400.0])
                .with_title("NightPDF"),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(PdfViewer::new()))),
    )
}
