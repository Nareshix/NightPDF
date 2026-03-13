use eframe::egui::{self, Color32, ColorImage, CursorIcon, Key, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use pdfium_render::prelude::*;
use arboard::Clipboard;
use rfd::FileDialog;
use std::collections::HashMap;

const THEMES: &[(&str, u8, u8, u8)] = &[
    ("Classic Dark",   0,   0,   0  ),
    ("Claude Warm",    42,  37,  34 ),
    ("ChatGPT Cool",   52,  53,  65 ),
    ("Sepia Dark",     40,  35,  25 ),
    ("Midnight Blue",  25,  30,  45 ),
    ("Forest Green",   25,  35,  30 ),
];

// Sample average brightness of the image using a grid of pixels (fast, not every pixel)
fn average_brightness(rgba: &RgbaImage) -> f32 {
    let (w, h) = rgba.dimensions();
    let step_x = (w / 20).max(1);
    let step_y = (h / 20).max(1);
    let mut sum = 0.0f32;
    let mut count = 0u32;
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let px = rgba.get_pixel(x, y).0;
            sum += 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
            count += 1;
            x += step_x;
        }
        y += step_y;
    }
    if count == 0 { 128.0 } else { sum / count as f32 }
}

fn apply_theme(rgba: &mut RgbaImage, idx: usize) {
    let avg = average_brightness(rgba);
    if avg < 80.0 { return; }

    let (_, r, g, b) = THEMES[idx];
    let (br, bg, bb) = (r as f32, g as f32, b as f32);
    for px in rgba.pixels_mut() {
        let [pr, pg, pb, a] = px.0;
        let lum = 0.299 * pr as f32 + 0.587 * pg as f32 + 0.114 * pb as f32;
        let f = 1.0 - lum / 255.0;
        px.0 = [
            (br + (255.0 - br) * f) as u8,
            (bg + (255.0 - bg) * f) as u8,
            (bb + (255.0 - bb) * f) as u8,
            a,
        ];
    }
}

struct CachedChar {
    bounds: PdfRect,
    text: String,
}

// Per-page size info — fetched once on load, no render needed
struct PageInfo {
    width_pts: f32,
    height_pts: f32,
}

struct PdfViewer {
    pdf_bytes: Option<Vec<u8>>,
    total_pages: usize,
    page_infos: Vec<PageInfo>,
    page_cache: HashMap<usize, TextureHandle>,
    char_cache: Vec<CachedChar>,
    char_cache_page: Option<usize>,

    theme_idx: usize,
    zoom: f32,

    drag_start: Option<Pos2>,
    drag_end: Option<Pos2>,
    drag_page: Option<usize>,
    selected_text: String,
    selected_rects: Vec<(usize, PdfRect)>,

    last_click_pos: Option<Pos2>,
    last_click_time: f64,
    click_count: u8,
    click_page: Option<usize>,

    show_search: bool,
    search_input: String,
    search_query: String,
    search_bounds: Vec<(usize, PdfRect)>,
    search_match_count: usize,

    page_screen_rects: Vec<Rect>,

    clipboard: Option<Clipboard>,

    // Smooth scrolling — we own the offset and drive it with velocity+friction
    scroll_offset: f32,
    scroll_velocity: f32,
}

impl PdfViewer {
    fn new() -> Self {
        Self {
            pdf_bytes: None,
            total_pages: 0,
            page_infos: Vec::new(),
            page_cache: HashMap::new(),
            char_cache: Vec::new(),
            char_cache_page: None,
            theme_idx: 1,
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

    fn load_pdf(&mut self, path: &std::path::Path) {
        let Ok(bytes) = std::fs::read(path) else { eprintln!("Cannot read file"); return };
        let pdfium = Pdfium::default();

        let (total_pages, page_infos) = {
            let Ok(doc) = pdfium.load_pdf_from_byte_slice(&bytes, None) else { return };
            let total = doc.pages().len() as usize;
            let infos = (0..total)
                .filter_map(|i| doc.pages().get(i as u16).ok())
                .map(|p| PageInfo { width_pts: p.width().value, height_pts: p.height().value })
                .collect();
            (total, infos)
        };

        self.total_pages = total_pages;
        self.page_infos = page_infos;
        self.pdf_bytes = Some(bytes);
        self.page_cache.clear();
        self.char_cache.clear();
        self.char_cache_page = None;
        self.clear_selection();
        self.search_bounds.clear();
        self.search_match_count = 0;
        self.page_screen_rects = vec![Rect::ZERO; self.total_pages];
        self.scroll_offset = 0.0;
        self.scroll_velocity = 0.0;
    }

    fn ensure_page_rendered(&mut self, page_idx: usize, ctx: &egui::Context) {
        if self.page_cache.contains_key(&page_idx) { return }
        let Some(bytes) = &self.pdf_bytes else { return };
        let pdfium = Pdfium::default();
        let Ok(doc) = pdfium.load_pdf_from_byte_slice(bytes, None) else { return };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return };

        let render_w = (900.0 * self.zoom) as i32;
        let config = PdfRenderConfig::new().set_target_width(render_w);
        let Ok(bitmap) = page.render_with_config(&config) else { return };
        let image = bitmap.as_image();
        let mut rgba = image.to_rgba8();
        apply_theme(&mut rgba, self.theme_idx);
        let (w, h) = rgba.dimensions();
        let pixels: Vec<Color32> = rgba.pixels()
            .map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
            .collect();
        let texture = ctx.load_texture(
            format!("pdf-page-{}", page_idx),
            ColorImage { size: [w as usize, h as usize], pixels },
            TextureOptions::LINEAR,
        );
        if self.page_cache.len() >= 8 {
            if let Some(&oldest) = self.page_cache.keys().next() {
                self.page_cache.remove(&oldest);
            }
        }
        self.page_cache.insert(page_idx, texture);
    }

    fn ensure_char_cache(&mut self, page_idx: usize) {
        if self.char_cache_page == Some(page_idx) { return }
        let Some(bytes) = &self.pdf_bytes else { return };
        let pdfium = Pdfium::default();
        let Ok(doc) = pdfium.load_pdf_from_byte_slice(bytes, None) else { return };
        let Ok(page) = doc.pages().get(page_idx as u16) else { return };
        self.char_cache.clear();
        if let Ok(text_page) = page.text() {
            for ch in text_page.chars().iter() {
                if let (Ok(bounds), Some(s)) = (ch.loose_bounds(), ch.unicode_string()) {
                    self.char_cache.push(CachedChar { bounds, text: s });
                }
            }
        }
        self.char_cache_page = Some(page_idx);
    }

    fn screen_to_pdf_page(&self, pos: Pos2, page_idx: usize) -> Option<(f32, f32)> {
        let r = self.page_screen_rects.get(page_idx)?;
        let info = self.page_infos.get(page_idx)?;
        let px = ((pos.x - r.min.x) / r.width() * info.width_pts).clamp(0.0, info.width_pts);
        let py = (info.height_pts - (pos.y - r.min.y) / r.height() * info.height_pts)
            .clamp(0.0, info.height_pts);
        Some((px, py))
    }

    fn pdf_rect_to_screen_page(&self, pr: &PdfRect, page_idx: usize) -> Option<Rect> {
        let r = self.page_screen_rects.get(page_idx)?;
        let info = self.page_infos.get(page_idx)?;
        let (pw, ph) = (info.width_pts, info.height_pts);
        let sx = |x: f32| r.min.x + (x / pw) * r.width();
        let sy = |y: f32| r.min.y + ((ph - y) / ph) * r.height();
        Some(Rect::from_min_max(
            Pos2::new(sx(pr.left().value),  sy(pr.top().value)),
            Pos2::new(sx(pr.right().value), sy(pr.bottom().value)),
        ))
    }

    fn page_at_pos(&self, pos: Pos2) -> Option<usize> {
        self.page_screen_rects.iter().position(|r| r.contains(pos))
    }

    fn update_selection(&mut self) {
        let (Some(s), Some(e), Some(page_idx)) = (self.drag_start, self.drag_end, self.drag_page)
            else { return };

        self.ensure_char_cache(page_idx);

        let Some((sx, sy)) = self.screen_to_pdf_page(s, page_idx) else { return };
        let Some((ex, ey)) = self.screen_to_pdf_page(e, page_idx) else { return };

        if self.char_cache.is_empty() { return }

        let mut indices: Vec<usize> = (0..self.char_cache.len()).collect();
        indices.sort_by(|&a, &b| {
            let cy = |i: usize| {
                let b = &self.char_cache[i].bounds;
                (b.bottom().value + b.top().value) * 0.5
            };
            cy(b).partial_cmp(&cy(a)).unwrap()
        });

        let mut lines: Vec<Vec<usize>> = Vec::new();
        for idx in indices {
            let b = &self.char_cache[idx].bounds;
            let cy = (b.bottom().value + b.top().value) * 0.5;
            let ch = (b.top().value - b.bottom().value).abs().max(1.0);
            let thresh = ch * 0.6;
            if let Some(last) = lines.last_mut() {
                let last_cy = {
                    let lb = &self.char_cache[last[0]].bounds;
                    (lb.bottom().value + lb.top().value) * 0.5
                };
                if (cy - last_cy).abs() <= thresh { last.push(idx); continue; }
            }
            lines.push(vec![idx]);
        }

        let (start_x, start_y, end_x, end_y) =
            if sy >= ey { (sx, sy, ex, ey) } else { (ex, ey, sx, sy) };

        let line_cy = |line: &Vec<usize>| -> f32 {
            let b = &self.char_cache[line[0]].bounds;
            (b.bottom().value + b.top().value) * 0.5
        };
        let start_line = lines.iter().enumerate()
            .min_by(|(_, a), (_, b)| {
                (line_cy(a) - start_y).abs().partial_cmp(&(line_cy(b) - start_y).abs()).unwrap()
            }).map(|(i, _)| i).unwrap_or(0);
        let end_line = lines.iter().enumerate()
            .min_by(|(_, a), (_, b)| {
                (line_cy(a) - end_y).abs().partial_cmp(&(line_cy(b) - end_y).abs()).unwrap()
            }).map(|(i, _)| i).unwrap_or(lines.len().saturating_sub(1));

        let mut text = String::new();
        let mut rects = Vec::new();

        for (li, line) in lines.iter().enumerate() {
            if li < start_line || li > end_line { continue; }
            for &ci in line {
                let b = &self.char_cache[ci].bounds;
                let cx = (b.left().value + b.right().value) * 0.5;
                let selected = if start_line == end_line {
                    let (x0, x1) = (start_x.min(end_x), start_x.max(end_x));
                    cx >= x0 && cx <= x1
                } else if li == start_line { cx >= start_x }
                  else if li == end_line   { cx <= end_x   }
                  else                     { true           };
                if selected {
                    text.push_str(&self.char_cache[ci].text);
                    rects.push((page_idx, self.char_cache[ci].bounds));
                }
            }
        }
        self.selected_text = text.trim().to_string();
        self.selected_rects = rects;
    }

    fn char_at_page(&mut self, pos: Pos2, page_idx: usize) -> Option<usize> {
        self.ensure_char_cache(page_idx);
        let (px, py) = self.screen_to_pdf_page(pos, page_idx)?;
        for (i, ch) in self.char_cache.iter().enumerate() {
            let b = &ch.bounds;
            if px >= b.left().value && px <= b.right().value
            && py >= b.bottom().value && py <= b.top().value {
                return Some(i);
            }
        }
        self.char_cache.iter().enumerate()
            .min_by(|(_, a), (_, b)| {
                let dist = |ch: &CachedChar| {
                    let cx = (ch.bounds.left().value + ch.bounds.right().value) * 0.5;
                    let cy = (ch.bounds.bottom().value + ch.bounds.top().value) * 0.5;
                    (cx - px).powi(2) + (cy - py).powi(2)
                };
                dist(a).partial_cmp(&dist(b)).unwrap()
            })
            .map(|(i, _)| i)
    }

    fn select_word_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some(idx) = self.char_at_page(pos, page_idx) else { return };
        let is_boundary = |i: usize| self.char_cache[i].text.trim().is_empty();
        if is_boundary(idx) { return; }
        let mut start = idx;
        while start > 0 && !is_boundary(start - 1) { start -= 1; }
        let mut end = idx;
        while end + 1 < self.char_cache.len() && !is_boundary(end + 1) { end += 1; }
        let mut text = String::new();
        let mut rects = Vec::new();
        for ch in &self.char_cache[start..=end] {
            text.push_str(&ch.text);
            rects.push((page_idx, ch.bounds));
        }
        self.selected_text = text.trim().to_string();
        self.selected_rects = rects;
    }

    fn select_line_at(&mut self, pos: Pos2, page_idx: usize) {
        let Some(idx) = self.char_at_page(pos, page_idx) else { return };
        let b = &self.char_cache[idx].bounds;
        let line_cy = (b.bottom().value + b.top().value) * 0.5;
        let line_h  = (b.top().value - b.bottom().value).abs().max(1.0);
        let thresh  = line_h * 0.7;
        let mut text = String::new();
        let mut rects = Vec::new();
        for ch in &self.char_cache {
            let cb = &ch.bounds;
            let cy = (cb.bottom().value + cb.top().value) * 0.5;
            if (cy - line_cy).abs() <= thresh {
                text.push_str(&ch.text);
                rects.push((page_idx, ch.bounds));
            }
        }
        self.selected_text = text.trim().to_string();
        self.selected_rects = rects;
    }

    fn do_search(&mut self) {
        self.search_bounds.clear();
        self.search_match_count = 0;
        if self.search_query.is_empty() { return }
        let Some(bytes) = self.pdf_bytes.clone() else { return };
        let pdfium = Pdfium::default();
        let Ok(doc) = pdfium.load_pdf_from_byte_slice(&bytes, None) else { return };
        for page_idx in 0..self.total_pages {
            let Ok(page) = doc.pages().get(page_idx as u16) else { continue };
            let Ok(text) = page.text() else { continue };
            let options = PdfSearchOptions::new();
            let Ok(search) = text.search(&self.search_query, &options) else { continue };
            for segments in search.iter(PdfSearchDirection::SearchForward) {
                for seg in segments.iter() {
                    self.search_bounds.push((page_idx, seg.bounds()));
                }
            }
        }
        self.search_match_count = self.search_bounds.len();
    }

    fn clear_selection(&mut self) {
        self.drag_start = None;
        self.drag_end = None;
        self.drag_page = None;
        self.selected_text.clear();
        self.selected_rects.clear();
    }

    fn copy_selection(&mut self) {
        if !self.selected_text.is_empty() {
            if let Some(cb) = &mut self.clipboard {
                let _ = cb.set_text(self.selected_text.clone());
            }
        }
    }

    fn theme_bg(&self) -> Color32 {
        let (_, r, g, b) = THEMES[self.theme_idx];
        Color32::from_rgb(r, g, b)
    }

    fn page_display_w(&self, _page_idx: usize, avail_w: f32) -> f32 {
        let base = 900.0 * self.zoom;
        base.min(avail_w - 24.0).max(100.0)
    }

    fn page_display_size(&self, page_idx: usize, avail_w: f32) -> Vec2 {
        let info = &self.page_infos[page_idx];
        let w = self.page_display_w(page_idx, avail_w);
        let h = w * info.height_pts / info.width_pts;
        Vec2::new(w, h)
    }
}

impl eframe::App for PdfViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (open, ctrl_f, ctrl_c, esc, enter) = ctx.input(|i| (
            i.key_pressed(Key::O) && i.modifiers.ctrl,
            i.key_pressed(Key::F) && i.modifiers.ctrl,
            i.key_pressed(Key::C) && i.modifiers.ctrl,
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Enter),
        ));
        if open { if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() { self.load_pdf(&p); } }
        if ctrl_f { self.show_search = !self.show_search; }
        if ctrl_c { self.copy_selection(); }
        if esc    { self.show_search = false; self.search_bounds.clear(); }

        // ── Smooth scroll physics ─────────────────────────────────────────────
        // Read raw scroll delta (unsmoothed) and amplify it, then apply
        // an exponential friction so it glides to a stop naturally.
        let (raw_scroll, dt) = ctx.input(|i| (i.raw_scroll_delta.y, i.predicted_dt));

        if raw_scroll.abs() > 0.1 {
            // Amplify: multiply by ~25× to match Chrome-style trackpad feel
            self.scroll_velocity += raw_scroll * 100.0;
            // Higher cap so fast swipes can actually cover many pages
            self.scroll_velocity = self.scroll_velocity.clamp(-8000.0, 8000.0);
        }

        // Exponential decay — tune the 12.0 constant to taste (higher = stops faster)
        let friction = (-8.0 * dt).exp();
        self.scroll_velocity *= friction;

        // Apply velocity to offset (scroll down = positive offset)
        self.scroll_offset -= self.scroll_velocity * dt;
        self.scroll_offset = self.scroll_offset.max(0.0);

        // Keep repainting while still moving
        if self.scroll_velocity.abs() > 1.0 {
            ctx.request_repaint();
        }

        // ── Toolbar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("📂 Open  Ctrl+O").clicked() {
                    if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() { self.load_pdf(&p); }
                }
                ui.separator();
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(THEMES[self.theme_idx].0)
                    .show_ui(ui, |ui| {
                        for (i, (name, _, _, _)) in THEMES.iter().enumerate() {
                            if ui.selectable_label(self.theme_idx == i, *name).clicked() {
                                self.theme_idx = i;
                                self.page_cache.clear();
                            }
                        }
                    });
                ui.separator();
                ui.label(format!("{} pages", self.total_pages));
                ui.separator();
                if ui.button("−").clicked() { self.zoom = (self.zoom - 0.15).max(0.3); self.page_cache.clear(); }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").clicked() { self.zoom = (self.zoom + 0.15).min(3.0); self.page_cache.clear(); }
                ui.separator();
                if ui.button("🔍  Ctrl+F").clicked() { self.show_search = !self.show_search; }
                if !self.selected_text.is_empty() {
                    if ui.button("📋 Copy  Ctrl+C").clicked() { self.copy_selection(); }
                }
            });
            ui.add_space(4.0);
        });

        // ── Search bar ────────────────────────────────────────────────────────
        if self.show_search {
            egui::TopBottomPanel::top("search").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    let resp = ui.add_sized(
                        [280.0, 24.0],
                        egui::TextEdit::singleline(&mut self.search_input).hint_text("Search all pages…"),
                    );
                    resp.request_focus();
                    if (resp.lost_focus() && enter) || ui.button("Find").clicked() {
                        self.search_query = self.search_input.clone();
                        self.do_search();
                    }
                    if self.search_match_count > 0 {
                        ui.colored_label(Color32::from_rgb(100, 220, 120),
                            format!("{} match{}", self.search_match_count,
                                if self.search_match_count == 1 { "" } else { "es" }));
                    } else if !self.search_query.is_empty() {
                        ui.colored_label(Color32::from_rgb(255, 100, 100), "No matches");
                    }
                    if ui.button("✕").clicked() {
                        self.show_search = false;
                        self.search_bounds.clear();
                        self.search_match_count = 0;
                    }
                });
                ui.add_space(4.0);
            });
        }

        // ── Status bar ────────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if !self.selected_text.is_empty() {
                    let preview = if self.selected_text.len() > 70 {
                        format!("{}…", &self.selected_text[..70])
                    } else { self.selected_text.clone() };
                    ui.colored_label(Color32::from_rgb(100, 200, 255), format!("\"{}\"", preview));
                    ui.weak("— Ctrl+C to copy");
                } else {
                    ui.weak("Drag to select  •  Double-click: word  •  Triple-click: line  •  Ctrl+F: search all pages");
                }
            });
            ui.add_space(3.0);
        });

        // ── Main scroll area ──────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            let bg = self.theme_bg();
            ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, bg);

            if self.pdf_bytes.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new("📄  Open a PDF to get started")
                        .size(22.0).color(Color32::from_gray(140)));
                });
                return;
            }

            let avail_w = ui.available_width();
            let viewport_rect = ui.clip_rect();

            // Drive the ScrollArea with our physics-based offset.
            // egui will clamp it to the valid range automatically.
            let mut offset = self.scroll_offset;
            let scroll_output = egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .vertical_scroll_offset(offset)
                .show(ui, |ui| {
                    ui.add_space(12.0);

                    for page_idx in 0..self.total_pages {
                        let size = self.page_display_size(page_idx, avail_w);
                        let side_pad = ((avail_w - size.x) / 2.0).max(0.0);

                        ui.horizontal(|ui| {
                            ui.add_space(side_pad);

                            let (page_rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

                            if self.page_screen_rects.len() > page_idx {
                                self.page_screen_rects[page_idx] = page_rect;
                            }

                            let is_visible = viewport_rect.intersects(page_rect);

                            if is_visible {
                                self.ensure_page_rendered(page_idx, ctx);

                                if let Some(texture) = self.page_cache.get(&page_idx) {
                                    let tex_id = texture.id();
                                    let painter = ui.painter();

                                    painter.image(tex_id, page_rect,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE);

                                    let search_rects: Vec<PdfRect> = self.search_bounds.iter()
                                        .filter(|(pi, _)| *pi == page_idx)
                                        .map(|(_, r)| *r)
                                        .collect();
                                    for pr in &search_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx) {
                                            painter.rect_filled(sr, 2.0, Color32::from_rgba_premultiplied(255, 215, 0, 100));
                                            painter.rect_stroke(sr, 2.0, Stroke::new(1.5, Color32::from_rgb(255, 180, 0)), egui::StrokeKind::Outside);
                                        }
                                    }

                                    let sel_rects: Vec<PdfRect> = self.selected_rects.iter()
                                        .filter(|(pi, _)| *pi == page_idx)
                                        .map(|(_, r)| *r)
                                        .collect();
                                    for pr in &sel_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx) {
                                            painter.rect_filled(sr, 0.0, Color32::from_rgba_premultiplied(80, 140, 255, 110));
                                        }
                                    }
                                }
                            } else {
                                let painter = ui.painter();
                                painter.rect_filled(page_rect, 4.0, Color32::from_gray(40));
                                painter.text(
                                    page_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("{}", page_idx + 1),
                                    egui::FontId::proportional(14.0),
                                    Color32::from_gray(80),
                                );
                            }

                            if response.hovered() {
                                ctx.set_cursor_icon(CursorIcon::Text);
                            }

                            if response.clicked() {
                                let now = ctx.input(|i| i.time);
                                let pos = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(Pos2::ZERO);
                                let same_spot = self.last_click_pos.map(|p| p.distance(pos) < 5.0).unwrap_or(false);
                                let rapid = (now - self.last_click_time) < 0.4;
                                let same_page = self.click_page == Some(page_idx);
                                if same_spot && rapid && same_page {
                                    self.click_count = (self.click_count + 1).min(3);
                                } else {
                                    self.click_count = 1;
                                    self.clear_selection();
                                }
                                self.last_click_pos = Some(pos);
                                self.last_click_time = now;
                                self.click_page = Some(page_idx);
                                match self.click_count {
                                    2 => self.select_word_at(pos, page_idx),
                                    3 => self.select_line_at(pos, page_idx),
                                    _ => {}
                                }
                                ctx.request_repaint();
                            }

                            if response.drag_started() {
                                self.click_count = 0;
                                self.clear_selection();
                                self.drag_start = ctx.input(|i| i.pointer.interact_pos());
                                self.drag_page = Some(page_idx);
                            }
                            if response.dragged() {
                                self.drag_end = ctx.input(|i| i.pointer.interact_pos());
                                self.update_selection();
                                ctx.request_repaint();
                            }
                        });

                        ui.add_space(8.0);
                    }

                    ui.add_space(12.0);
                });

            // Sync our stored offset back from what egui actually rendered
            // (it clamps to valid range, so this prevents us drifting past the end)
            self.scroll_offset = scroll_output.state.offset.y;
        });
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    eframe::run_native(
        "PDF Dark Reader",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1000.0, 860.0])
                .with_min_inner_size([600.0, 400.0])
                .with_title("PDF Dark Reader"),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(PdfViewer::new()))),
    )
}