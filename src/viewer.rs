use arboard::Clipboard;
use eframe::egui::{self, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::theme::{self};
use crate::types::PageInfo;

// Keep the C++ PDFium engine alive globally so we don't reload it.
// REQUIRES the "sync" feature on the pdfium-render crate!
pub static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

pub struct PdfViewer {
    // We now store the parsed document natively, avoiding reloading!
    pub document: Option<PdfDocument<'static>>,
    pub total_pages: usize,
    pub page_infos: Vec<PageInfo>,
    pub page_cache: HashMap<usize, TextureHandle>,

    pub theme_idx: usize,
    pub zoom: f32,

    pub drag_start: Option<Pos2>,
    pub drag_end: Option<Pos2>,
    pub drag_page: Option<usize>,
    pub selected_text: String,
    pub selected_rects: Vec<(usize, PdfRect)>,

    pub last_click_pos: Option<Pos2>,
    pub last_click_time: f64,
    pub click_count: u8,
    pub click_page: Option<usize>,

    pub show_search: bool,
    pub search_input: String,
    pub search_query: String,
    pub search_bounds: Vec<(usize, PdfRect)>,
    pub search_match_count: usize,

    pub page_screen_rects: Vec<Rect>,

    pub clipboard: Option<Clipboard>,

    pub scroll_offset: f32,
    pub scroll_velocity: f32,
}

impl PdfViewer {
    pub fn new() -> Self {
        // Initialize the C++ library once on startup
        PDFIUM.get_or_init(Pdfium::default);

        Self {
            document: None,
            total_pages: 0,
            page_infos: Vec::new(),
            page_cache: HashMap::new(),
            theme_idx: 2,
            zoom: 1.0,
            drag_start: None,
            drag_end: None,
            drag_page: None,
            selected_text: String::new(),
            selected_rects: Vec::new(),
            last_click_pos: None,
            last_click_time: 0.0,
            click_count: 0,
            click_page: None,
            show_search: false,
            search_input: String::new(),
            search_query: String::new(),
            search_bounds: Vec::new(),
            search_match_count: 0,
            page_screen_rects: Vec::new(),
            clipboard: Clipboard::new().ok(),
            scroll_offset: 0.0,
            scroll_velocity: 0.0,
        }
    }

    pub fn load_pdf(&mut self, path: &std::path::Path) {
        let pdfium = PDFIUM.get().unwrap();

        // Load file via the C++ engine (handles locking and memory safely)
        let Ok(doc) = pdfium.load_pdf_from_file(path.to_str().unwrap(), None) else {
            eprintln!("Cannot read file");
            return;
        };

        let total_pages = doc.pages().len() as usize;
        let infos = (0..total_pages)
            .filter_map(|i| doc.pages().get(i as u16).ok())
            .map(|p| PageInfo {
                width_pts: p.width().value,
                height_pts: p.height().value,
            })
            .collect();

        self.total_pages = total_pages;
        self.page_infos = infos;
        self.document = Some(doc); // Store the parsed document!

        self.page_cache.clear();
        self.clear_selection();
        self.search_bounds.clear();
        self.search_match_count = 0;
        self.page_screen_rects = vec![Rect::ZERO; self.total_pages];
        self.scroll_offset = 0.0;
        self.scroll_velocity = 0.0;
    }

    pub fn ensure_page_rendered(&mut self, page_idx: usize, ctx: &egui::Context) {
        if self.page_cache.contains_key(&page_idx) { return; }

        let Some(doc) = &self.document else { return };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return; };

        let render_w = (900.0 * self.zoom) as i32;
        let config = PdfRenderConfig::new()
            .set_target_width(render_w)
            .set_clear_color(PdfColor::WHITE);

        let Ok(bitmap) = page.render_with_config(&config) else { return; };

        // Ask PDFium for raw bytes (BGRA) - bypasses the image crate!
        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let mut pixels = bitmap.as_raw_bytes().to_vec();

        theme::apply_theme_and_convert_bgra_to_rgba(&mut pixels, width, height, self.theme_idx);

        let texture = ctx.load_texture(
            format!("pdf-page-{}", page_idx),
            ColorImage::from_rgba_unmultiplied([width, height], &pixels),
            TextureOptions::LINEAR,
        );

        if self.page_cache.len() >= 8 {
            if let Some(&oldest) = self.page_cache.keys().next() {
                self.page_cache.remove(&oldest);
            }
        }
        self.page_cache.insert(page_idx, texture);
    }

    pub fn screen_to_pdf_page(&self, pos: Pos2, page_idx: usize) -> Option<(f32, f32)> {
        let r = self.page_screen_rects.get(page_idx)?;
        let info = self.page_infos.get(page_idx)?;
        let px = ((pos.x - r.min.x) / r.width() * info.width_pts).clamp(0.0, info.width_pts);
        let py = (info.height_pts - (pos.y - r.min.y) / r.height() * info.height_pts)
            .clamp(0.0, info.height_pts);
        Some((px, py))
    }

    pub fn pdf_rect_to_screen_page(&self, pr: &PdfRect, page_idx: usize) -> Option<Rect> {
        let r = self.page_screen_rects.get(page_idx)?;
        let info = self.page_infos.get(page_idx)?;
        let (pw, ph) = (info.width_pts, info.height_pts);
        let sx = |x: f32| r.min.x + (x / pw) * r.width();
        let sy = |y: f32| r.min.y + ((ph - y) / ph) * r.height();
        Some(Rect::from_min_max(
            Pos2::new(sx(pr.left().value), sy(pr.top().value)),
            Pos2::new(sx(pr.right().value), sy(pr.bottom().value)),
        ))
    }

    pub fn page_at_pos(&self, pos: Pos2) -> Option<usize> {
        self.page_screen_rects.iter().position(|r| r.contains(pos))
    }

    /// Helper to find the character index nearest to a coordinate
    fn get_char_index_at(&self, px: f32, py: f32, chars: &[PdfPageTextChar]) -> Option<usize> {
        // First check for a direct hit inside the bounding box
        for (i, ch) in chars.iter().enumerate() {
            if let Ok(b) = ch.loose_bounds() {
                if px >= b.left().value && px <= b.right().value &&
                   py >= b.bottom().value && py <= b.top().value {
                    return Some(i);
                }
            }
        }

        // If not directly on a character, fallback to the nearest one
        chars.iter().enumerate().min_by(|(_, a), (_, b)| {
            let dist = |ch: &PdfPageTextChar| {
                if let Ok(b) = ch.loose_bounds() {
                    let cx = (b.left().value + b.right().value) * 0.5;
                    let cy = (b.bottom().value + b.top().value) * 0.5;
                    (cx - px).powi(2) + (cy - py).powi(2)
                } else {
                    f32::MAX
                }
            };
            dist(a).partial_cmp(&dist(b)).unwrap_or(std::cmp::Ordering::Equal)
        }).map(|(i, _)| i)
    }

    pub fn update_selection(&mut self) {
        let (Some(s), Some(e), Some(page_idx)) = (self.drag_start, self.drag_end, self.drag_page) else { return; };
        let Some((sx, sy)) = self.screen_to_pdf_page(s, page_idx) else { return; };
        let Some((ex, ey)) = self.screen_to_pdf_page(e, page_idx) else { return; };

        let Some(doc) = &self.document else { return; };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return; };
        let Ok(text) = page.text() else { return; };

        // Bind text.chars() to a variable so it lives long enough for the Vec
        let text_chars = text.chars();
        let chars: Vec<_> = text_chars.iter().collect();
        if chars.is_empty() { return; }

        let start_idx = self.get_char_index_at(sx, sy, &chars).unwrap_or(0);
        let end_idx = self.get_char_index_at(ex, ey, &chars).unwrap_or(chars.len().saturating_sub(1));

        let min_idx = start_idx.min(end_idx);
        let max_idx = start_idx.max(end_idx);

        let mut selected_text = String::new();
        self.selected_rects.clear();

        for i in min_idx..=max_idx {
            let ch = &chars[i];
            if let Some(s) = ch.unicode_string() {
                selected_text.push_str(&s);
            }
            if let Ok(bounds) = ch.loose_bounds() {
                self.selected_rects.push((page_idx, bounds));
            }
        }
        self.selected_text = selected_text;
    }

    pub fn select_word_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) else { return; };
        let Some(doc) = &self.document else { return; };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return; };
        let Ok(text) = page.text() else { return; };

        // Bind text.chars() to a variable so it lives long enough for the Vec
        let text_chars = text.chars();
        let chars: Vec<_> = text_chars.iter().collect();
        let Some(idx) = self.get_char_index_at(px, py, &chars) else { return; };

        let is_boundary = |i: usize| -> bool {
            chars[i].unicode_string().map(|s| s.trim().is_empty()).unwrap_or(true)
        };

        if is_boundary(idx) { return; }

        let mut start = idx;
        while start > 0 && !is_boundary(start - 1) { start -= 1; }

        let mut end = idx;
        let len = chars.len();
        while end + 1 < len && !is_boundary(end + 1) { end += 1; }

        let mut selected_text = String::new();
        self.selected_rects.clear();

        for i in start..=end {
            let ch = &chars[i];
            if let Some(s) = ch.unicode_string() { selected_text.push_str(&s); }
            if let Ok(b) = ch.loose_bounds() { self.selected_rects.push((page_idx, b)); }
        }
        self.selected_text = selected_text;
    }

    pub fn select_line_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) else { return; };
        let Some(doc) = &self.document else { return; };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return; };
        let Ok(text) = page.text() else { return; };

        // Bind text.chars() to a variable so it lives long enough for the Vec
        let text_chars = text.chars();
        let chars: Vec<_> = text_chars.iter().collect();
        let Some(idx) = self.get_char_index_at(px, py, &chars) else { return; };

        let clicked_ch = &chars[idx];
        let Ok(bounds) = clicked_ch.loose_bounds() else { return; };

        let line_cy = (bounds.bottom().value + bounds.top().value) * 0.5;
        let thresh = (bounds.top().value - bounds.bottom().value).abs() * 0.7;

        let mut selected_text = String::new();
        self.selected_rects.clear();

        // Grab all characters that share roughly the same Y-axis
        for ch in &chars {
            if let Ok(b) = ch.loose_bounds() {
                let cy = (b.bottom().value + b.top().value) * 0.5;
                if (cy - line_cy).abs() <= thresh {
                    if let Some(s) = ch.unicode_string() { selected_text.push_str(&s); }
                    self.selected_rects.push((page_idx, b));
                }
            }
        }
        self.selected_text = selected_text;
    }

    pub fn do_search(&mut self) {
        self.search_bounds.clear();
        self.search_match_count = 0;
        if self.search_query.is_empty() { return; }

        let Some(doc) = &self.document else { return; };

        for page_idx in 0..self.total_pages {
            let Ok(page) = doc.pages().get(page_idx as u16) else { continue; };
            let Ok(text) = page.text() else { continue; };

            let options = PdfSearchOptions::new();
            let Ok(search) = text.search(&self.search_query, &options) else { continue; };

            for segments in search.iter(PdfSearchDirection::SearchForward) {
                for seg in segments.iter() {
                    self.search_bounds.push((page_idx, seg.bounds()));
                }
            }
        }
        self.search_match_count = self.search_bounds.len();
    }

    pub fn clear_selection(&mut self) {
        self.drag_start = None;
        self.drag_end = None;
        self.drag_page = None;
        self.selected_text.clear();
        self.selected_rects.clear();
    }

    pub fn copy_selection(&mut self) {
        if !self.selected_text.is_empty() {
            if let Some(cb) = &mut self.clipboard {
                let _ = cb.set_text(self.selected_text.clone());
            }
        }
    }

    pub fn page_display_w(&self, _page_idx: usize, avail_w: f32) -> f32 {
        let base = 900.0 * self.zoom;
        base.min(avail_w - 24.0).max(100.0)
    }

    pub fn page_display_size(&self, page_idx: usize, avail_w: f32) -> Vec2 {
        let info = &self.page_infos[page_idx];
        let w = self.page_display_w(page_idx, avail_w);
        let h = w * info.height_pts / info.width_pts;
        Vec2::new(w, h)
    }
}