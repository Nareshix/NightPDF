use eframe::egui::Color32;

pub const THEMES: &[(&str, u8, u8, u8)] = &[
    ("Original", 255, 255, 255),
    ("Classic Dark", 0, 0, 0),
    ("Claude Warm", 42, 37, 34),
    ("ChatGPT Cool", 52, 53, 65),
    ("Sepia Dark", 40, 35, 25),
    ("Midnight Blue", 25, 30, 45),
    ("Forest Green", 25, 35, 30),
    ("Mocha Blue", 30, 30, 46),
];
pub fn average_brightness_bgra(pixels: &[u8], width: usize, height: usize) -> f32 {
    let step_x = (width / 20).max(1);
    let step_y = (height / 20).max(1);
    let mut sum = 0.0f32;
    let mut count = 0u32;
    let mut y = 0;

    while y < height {
        let mut x = 0;
        while x < width {
            let idx = (y * width + x) * 4;
            if idx + 2 < pixels.len() {
                // PDFium native format is BGRA
                let b = pixels[idx] as f32;
                let g = pixels[idx + 1] as f32;
                let r = pixels[idx + 2] as f32;
                sum += 0.299 * r + 0.587 * g + 0.114 * b;
                count += 1;
            }
            x += step_x;
        }
        y += step_y;
    }
    if count == 0 {
        128.0
    } else {
        sum / count as f32
    }
}

pub fn apply_theme_and_convert_bgra_to_rgba(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    theme_idx: usize,
) {
    if theme_idx == 0 {
        // Original Theme: Just swap Blue and Red to make it RGBA for egui
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        return;
    }

    let avg = average_brightness_bgra(pixels, width, height);
    if avg < 128.0 {
        // PDF is already dark, just convert BGRA -> RGBA and skip color math
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        return;
    }

    let (_, tr, tg, tb) = THEMES[theme_idx];
    let (br, bg, bb) = (tr as f32, tg as f32, tb as f32);

    for chunk in pixels.chunks_exact_mut(4) {
        let b = chunk[0];
        let g = chunk[1];
        let r = chunk[2];
        // chunk[3] is Alpha, leave it alone

        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let f = 1.0 - lum / 255.0;

        chunk[0] = (br + (255.0 - br) * f) as u8;
        chunk[1] = (bg + (255.0 - bg) * f) as u8;
        chunk[2] = (bb + (255.0 - bb) * f) as u8;
    }
}

pub fn theme_bg(idx: usize) -> Color32 {
    let (_, r, g, b) = THEMES[idx];
    Color32::from_rgb(r, g, b)
}
