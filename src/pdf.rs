use pdfium_render::prelude::*;

/// One character with its bounding box stored as fractions of the page (0..1).
/// Y is top-down (0 = top of page), matching egui's coordinate system.
#[derive(Clone)]
pub struct CharInfo {
    pub ch: char,
    pub x: f32, // left edge, 0..1
    pub y: f32, // top edge, 0..1 (top-down)
    pub w: f32,
    pub h: f32,
}

pub struct Page {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub chars: Vec<CharInfo>,
}

pub struct Doc {
    pub path: String,
    pub pages: Vec<Page>,
}

const SCALE: f32 = 1.5;

pub fn load(path: &str) -> Result<Doc, PdfiumError> {
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
            .or_else(|_| Pdfium::bind_to_system_library())?,
    );

    let doc = pdfium.load_pdf_from_file(path, None)?;

    let render_config = PdfRenderConfig::new()
        .set_target_width((800.0 * SCALE) as i32)
        .set_maximum_height((1200.0 * SCALE) as i32);

    let mut pages = Vec::new();

    for page in doc.pages().iter() {
        // Render
        let bitmap = page.render_with_config(&render_config)?;
        let img = bitmap.as_image().into_rgba8();
        let (w, h) = (img.width(), img.height());
        let rgba = img.into_raw();

        // Page dimensions in PDF points
        let pw = page.width().value;
        let ph = page.height().value;

        // Extract chars + bounding boxes
        let mut chars: Vec<CharInfo> = Vec::new();
        let mut text = String::new();

        if let Ok(page_text) = page.text() {
            for c in page_text.chars().iter() {
                let ch = match c.unicode_char() {
                    Some(ch) => ch,
                    None => continue,
                };
                text.push(ch);

                if let Ok(bounds) = c.loose_bounds() {
                    // PDF coords: bottom-left origin → flip Y to top-down
                    let x = bounds.left().value / pw;
                    let y = 1.0 - (bounds.top().value / ph);
                    let cw = (bounds.right().value - bounds.left().value).abs() / pw;
                    let ch_h = (bounds.top().value - bounds.bottom().value).abs() / ph;
                    chars.push(CharInfo { ch, x, y, w: cw, h: ch_h });
                }
            }
        }

        pages.push(Page { rgba, width: w, height: h, text, chars });
    }

    Ok(Doc { path: path.to_string(), pages })
}