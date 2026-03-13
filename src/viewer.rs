use eframe::egui::{self, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use pdfium_render::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use crate::theme::{self};
use crate::types::PageInfo;

// Keep the C++ PDFium engine alive globally so we don't reload it.
// REQUIRES the "sync" feature on the pdfium-render crate!
pub static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

pub struct PdfViewer {
    pub document: Option<PdfDocument<'static>>,
    pub total_pages: usize,
    pub current_page: usize,
    pub page_infos: Vec<PageInfo>,

    pub page_cache: HashMap<usize, TextureHandle>,
    pub page_cache_order: VecDeque<usize>,

    pub theme_idx: usize,
    pub zoom: f32,

    pub drag_start: Option<(usize, Pos2)>,
    pub drag_end: Option<(usize, Pos2)>,

    pub selected_text: String,
    pub selected_rects: Vec<(usize, PdfRect)>,


    pub show_search: bool,
    pub search_input: String,
    pub search_query: String,
    pub search_bounds: Vec<(usize, PdfRect)>,
    pub search_match_count: usize,
    pub search_current_match: usize,
    pub jump_to_match: bool,

    // Jump to page feature
    pub show_jump: bool,
    pub jump_input: String,
    pub jump_error: bool,
    pub target_scroll_page: Option<usize>,

    pub page_screen_rects: Vec<Rect>,

    pub scroll_offset: f32,

    // Bookmark / restore position
    pub current_file_path: Option<String>,
    pub last_save_time: f64,
}

impl PdfViewer {
    pub fn new() -> Self {
        PDFIUM.get_or_init(|| Pdfium::default());

        Self {
            document: None,
            total_pages: 0,
            current_page: 0,
            page_infos: Vec::new(),
            page_cache: HashMap::new(),
            page_cache_order: VecDeque::new(),
            theme_idx: 2,
            zoom: 1.0,
            drag_start: None,
            drag_end: None,
            selected_text: String::new(),
            selected_rects: Vec::new(),
            show_search: false,
            search_input: String::new(),
            search_query: String::new(),
            search_bounds: Vec::new(),
            search_match_count: 0,
            search_current_match: 0,
            jump_to_match: false,

            show_jump: false,
            jump_input: String::new(),
            jump_error: false,
            target_scroll_page: None,

            page_screen_rects: Vec::new(),
            scroll_offset: 0.0,

            current_file_path: None,
            last_save_time: 0.0,
        }
    }

    fn bookmarks_path() -> std::path::PathBuf {
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop();
        p.push("pdf_bookmarks.txt");
        p
    }

    pub fn save_bookmark(&self) {
        let Some(path) = &self.current_file_path else { return };
        let bm_path = Self::bookmarks_path();

        let mut lines: Vec<String> = std::fs::read_to_string(&bm_path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with(path.as_str()))
            .map(|l| l.to_string())
            .collect();

        lines.push(format!("{}|{}", path, self.scroll_offset));
        let _ = std::fs::write(&bm_path, lines.join("\n"));
    }

    fn load_bookmark(&self) -> Option<f32> {
        let path = self.current_file_path.as_ref()?;
        std::fs::read_to_string(Self::bookmarks_path())
            .ok()?
            .lines()
            .find(|l| l.starts_with(path.as_str()))?
            .split('|')
            .nth(1)?
            .parse()
            .ok()
    }

    pub fn load_pdf(&mut self, path: &std::path::Path) {
        let pdfium = PDFIUM.get().unwrap();

        // BUG FIX: Prevent panic if path contains invalid UTF-8 bytes
        let Some(path_str) = path.to_str() else {
            eprintln!("Invalid file path encoding.");
            return;
        };

        let Ok(doc) = pdfium.load_pdf_from_file(path_str, None) else {
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
        self.current_page = 0;
        self.page_infos = infos;
        self.document = Some(doc);

        self.page_cache.clear();
        self.page_cache_order.clear();
        self.clear_selection();

        self.search_bounds.clear();
        self.search_match_count = 0;
        self.search_current_match = 0;
        self.jump_to_match = false;

        self.show_jump = false;
        self.jump_input.clear();
        self.jump_error = false;
        self.target_scroll_page = None;

        self.page_screen_rects = vec![Rect::ZERO; self.total_pages];
        self.scroll_offset = 0.0;

        // Restore last position for this file
        self.current_file_path = path.to_str().map(|s| s.to_string());
        if let Some(saved) = self.load_bookmark() {
            self.scroll_offset = saved;
        }
    }

    pub fn ensure_page_rendered(&mut self, page_idx: usize, ctx: &egui::Context) {
        if self.page_cache.contains_key(&page_idx) {
            if let Some(pos) = self.page_cache_order.iter().position(|&p| p == page_idx) {
                self.page_cache_order.remove(pos);
                self.page_cache_order.push_back(page_idx);
            }
            return;
        }

        let Some(doc) = &self.document else { return };
        let Ok(page) = doc.pages().get(page_idx as u16) else {
            return;
        };

        let scale_factor = ctx.pixels_per_point();
        let render_w = (900.0 * self.zoom * scale_factor) as i32;
        let config = PdfRenderConfig::new()
            .set_target_width(render_w)
            .set_clear_color(PdfColor::WHITE);

        let Ok(bitmap) = page.render_with_config(&config) else {
            return;
        };

        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let mut pixels = bitmap.as_raw_bytes().to_vec();

        theme::apply_theme_and_convert_bgra_to_rgba(&mut pixels, width, height, self.theme_idx);

        let texture = ctx.load_texture(
            format!("pdf-page-{}", page_idx),
            ColorImage::from_rgba_unmultiplied([width, height], &pixels),
            TextureOptions::LINEAR,
        );

        if self.page_cache.len() >= 15 {
            if let Some(oldest) = self.page_cache_order.pop_front() {
                self.page_cache.remove(&oldest);
            }
        }
        self.page_cache.insert(page_idx, texture);
        self.page_cache_order.push_back(page_idx);
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

    pub fn nearest_page_to_pos(&self, pos: Pos2) -> Option<usize> {
        self.page_screen_rects
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let dist_a = a.distance_sq_to_pos(pos);
                let dist_b = b.distance_sq_to_pos(pos);
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    fn get_char_index_at(&self, px: f32, py: f32, chars: &[PdfPageTextChar]) -> Option<usize> {
        for (i, ch) in chars.iter().enumerate() {
            if let Ok(b) = ch.loose_bounds() {
                if px >= b.left().value
                    && px <= b.right().value
                    && py >= b.bottom().value
                    && py <= b.top().value
                {
                    return Some(i);
                }
            }
        }

        chars
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let dist = |ch: &PdfPageTextChar| {
                    if let Ok(b) = ch.loose_bounds() {
                        let cx = (b.left().value + b.right().value) * 0.5;
                        let cy = (b.bottom().value + b.top().value) * 0.5;
                        (cx - px).powi(2) + (cy - py).powi(2)
                    } else {
                        f32::MAX
                    }
                };
                dist(a)
                    .partial_cmp(&dist(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    pub fn update_selection(&mut self) {
        let (Some((start_page, s_pos)), Some((end_page, e_pos))) = (self.drag_start, self.drag_end)
        else {
            return;
        };
        let Some(doc) = &self.document else {
            return;
        };

        let mut selected_text = String::new();
        self.selected_rects.clear();

        let (top_page, top_pos, bottom_page, bottom_pos) = if start_page < end_page {
            (start_page, s_pos, end_page, e_pos)
        } else if start_page > end_page {
            (end_page, e_pos, start_page, s_pos)
        } else {
            (start_page, s_pos, end_page, e_pos)
        };

        for page_idx in top_page..=bottom_page {
            let Ok(page) = doc.pages().get(page_idx as u16) else {
                continue;
            };
            let Ok(text) = page.text() else { continue };

            let text_chars = text.chars();
            let chars: Vec<_> = text_chars.iter().collect();
            if chars.is_empty() {
                continue;
            }

            let (p_start, p_end) = if top_page == bottom_page {
                let i1 = self
                    .get_char_index_at(top_pos.x, top_pos.y, &chars)
                    .unwrap_or(0);
                let i2 = self
                    .get_char_index_at(bottom_pos.x, bottom_pos.y, &chars)
                    .unwrap_or(chars.len().saturating_sub(1));
                (i1.min(i2), i1.max(i2))
            } else if page_idx == top_page {
                let i = self
                    .get_char_index_at(top_pos.x, top_pos.y, &chars)
                    .unwrap_or(0);
                (i, chars.len().saturating_sub(1))
            } else if page_idx == bottom_page {
                let i = self
                    .get_char_index_at(bottom_pos.x, bottom_pos.y, &chars)
                    .unwrap_or(chars.len().saturating_sub(1));
                (0, i)
            } else {
                (0, chars.len().saturating_sub(1))
            };

            for i in p_start..=p_end {
                if i >= chars.len() {
                    continue;
                }
                let ch = &chars[i];
                if let Some(s) = ch.unicode_string() {
                    selected_text.push_str(&s);
                }
                if let Ok(bounds) = ch.loose_bounds() {
                    self.selected_rects.push((page_idx, bounds));
                }
            }

            if page_idx < bottom_page {
                selected_text.push('\n');
            }
        }
        self.selected_text = selected_text;
    }

    pub fn select_all(&mut self) {
        if self.document.is_none() {
            return;
        }

        self.clear_selection();
        let mut selected_text = String::new();

        let doc = self.document.as_ref().unwrap();

        for page_idx in 0..self.total_pages {
            let Ok(page) = doc.pages().get(page_idx as u16) else {
                continue;
            };
            let Ok(text) = page.text() else { continue };

            let text_chars = text.chars();
            let chars: Vec<_> = text_chars.iter().collect();

            for ch in &chars {
                if let Some(s) = ch.unicode_string() {
                    selected_text.push_str(&s);
                }
                if let Ok(b) = ch.loose_bounds() {
                    self.selected_rects.push((page_idx, b));
                }
            }
            if page_idx < self.total_pages.saturating_sub(1) {
                selected_text.push('\n');
            }
        }
        self.selected_text = selected_text;
    }

    pub fn select_word_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) else {
            return;
        };
        let Some(doc) = &self.document else {
            return;
        };
        let Ok(page) = doc.pages().get(page_idx as u16) else {
            return;
        };
        let Ok(text) = page.text() else {
            return;
        };

        let text_chars = text.chars();
        let chars: Vec<_> = text_chars.iter().collect();
        let Some(idx) = self.get_char_index_at(px, py, &chars) else {
            return;
        };

        let is_boundary = |i: usize| -> bool {
            chars[i]
                .unicode_string()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        };

        if is_boundary(idx) {
            return;
        }

        let mut start = idx;
        while start > 0 && !is_boundary(start - 1) {
            start -= 1;
        }

        let mut end = idx;
        let len = chars.len();
        while end + 1 < len && !is_boundary(end + 1) {
            end += 1;
        }

        let mut selected_text = String::new();
        self.selected_rects.clear();

        for i in start..=end {
            let ch = &chars[i];
            if let Some(s) = ch.unicode_string() {
                selected_text.push_str(&s);
            }
            if let Ok(b) = ch.loose_bounds() {
                self.selected_rects.push((page_idx, b));
            }
        }
        self.selected_text = selected_text;
    }

    pub fn select_line_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) else {
            return;
        };
        let Some(doc) = &self.document else {
            return;
        };
        let Ok(page) = doc.pages().get(page_idx as u16) else {
            return;
        };
        let Ok(text) = page.text() else {
            return;
        };

        let text_chars = text.chars();
        let chars: Vec<_> = text_chars.iter().collect();
        let Some(idx) = self.get_char_index_at(px, py, &chars) else {
            return;
        };

        let clicked_ch = &chars[idx];
        let Ok(bounds) = clicked_ch.loose_bounds() else {
            return;
        };

        let line_cy = (bounds.bottom().value + bounds.top().value) * 0.5;
        let thresh = (bounds.top().value - bounds.bottom().value).abs() * 0.7;

        let mut selected_text = String::new();
        self.selected_rects.clear();

        let mut line_chars = Vec::new();
        for ch in &chars {
            if let Ok(b) = ch.loose_bounds() {
                let cy = (b.bottom().value + b.top().value) * 0.5;
                if (cy - line_cy).abs() <= thresh {
                    line_chars.push((b, ch.unicode_string().unwrap_or_default()));
                }
            }
        }

        line_chars.sort_by(|(b1, _), (b2, _)| {
            b1.left()
                .value
                .partial_cmp(&b2.left().value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (b, s) in line_chars {
            selected_text.push_str(&s);
            self.selected_rects.push((page_idx, b));
        }
        self.selected_text = selected_text;
    }

    pub fn do_search(&mut self) {
        self.search_bounds.clear();
        self.search_match_count = 0;
        self.search_current_match = 0;
        self.jump_to_match = true;

        if self.search_query.is_empty() {
            return;
        }

        let Some(doc) = &self.document else {
            return;
        };

        for page_idx in 0..self.total_pages {
            let Ok(page) = doc.pages().get(page_idx as u16) else {
                continue;
            };
            let Ok(text) = page.text() else {
                continue;
            };

            let options = PdfSearchOptions::new();
            let Ok(search) = text.search(&self.search_query, &options) else {
                continue;
            };

            for segments in search.iter(PdfSearchDirection::SearchForward) {
                for seg in segments.iter() {
                    self.search_bounds.push((page_idx, seg.bounds()));
                }
            }
        }
        self.search_match_count = self.search_bounds.len();
        if self.search_match_count == 0 {
            self.jump_to_match = false;
        }
    }

    pub fn next_search_match(&mut self) {
        if self.search_match_count > 0 {
            self.search_current_match = (self.search_current_match + 1) % self.search_match_count;
            self.jump_to_match = true;
        }
    }

    pub fn prev_search_match(&mut self) {
        if self.search_match_count > 0 {
            if self.search_current_match == 0 {
                self.search_current_match = self.search_match_count - 1;
            } else {
                self.search_current_match -= 1;
            }
            self.jump_to_match = true;
        }
    }

    pub fn clear_selection(&mut self) {
        self.drag_start = None;
        self.drag_end = None;
        self.selected_text.clear();
        self.selected_rects.clear();
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