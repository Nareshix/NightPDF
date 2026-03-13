use eframe::egui::{self, Color32, ColorImage, CursorIcon, Key, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use pdfium_render::prelude::*;
use arboard::Clipboard;
use rfd::FileDialog;

const THEMES: &[(&str, u8, u8, u8)] = &[
    ("Classic Dark",   0,   0,   0  ),
    ("Claude Warm",    42,  37,  34 ),
    ("ChatGPT Cool",   52,  53,  65 ),
    ("Sepia Dark",     40,  35,  25 ),
    ("Midnight Blue",  25,  30,  45 ),
    ("Forest Green",   25,  35,  30 ),
];

fn apply_theme(rgba: &mut RgbaImage, idx: usize) {
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

struct PdfViewer {
    pdf_bytes: Option<Vec<u8>>,
    current_page: usize,
    total_pages: usize,
    theme_idx: usize,
    zoom: f32,
    texture: Option<TextureHandle>,
    page_pts: Option<(f32, f32)>,
    image_rect: Option<Rect>,
    needs_render: bool,

    char_cache: Vec<CachedChar>,

    drag_start: Option<Pos2>,
    drag_end: Option<Pos2>,
    selected_text: String,
    selected_rects: Vec<PdfRect>,

    // Click tracking: double-click = word, triple-click = line
    last_click_pos: Option<Pos2>,
    last_click_time: f64,
    click_count: u8,

    show_search: bool,
    search_input: String,
    search_query: String,
    search_bounds: Vec<PdfRect>,
    search_match_count: usize,

    clipboard: Option<Clipboard>,
}

impl PdfViewer {
    fn new() -> Self {
        Self {
            pdf_bytes: None,
            current_page: 0,
            total_pages: 0,
            theme_idx: 1,
            zoom: 1.0,
            texture: None,
            page_pts: None,
            image_rect: None,
            needs_render: false,
            char_cache: Vec::new(),
            drag_start: None,
            drag_end: None,
            selected_text: String::new(),
            selected_rects: Vec::new(),
            last_click_pos: None,
            last_click_time: 0.0,
            click_count: 0,
            show_search: false,
            search_input: String::new(),
            search_query: String::new(),
            search_bounds: Vec::new(),
            search_match_count: 0,
            clipboard: Clipboard::new().ok(),
        }
    }

    fn load_pdf(&mut self, path: &std::path::Path) {
        let Ok(bytes) = std::fs::read(path) else { eprintln!("Cannot read file"); return };
        let pdfium = Pdfium::default();
        if let Ok(doc) = pdfium.load_pdf_from_byte_slice(&bytes, None) {
            self.total_pages = doc.pages().len() as usize;
        }
        self.pdf_bytes = Some(bytes);
        self.current_page = 0;
        self.texture = None;
        self.needs_render = true;
        self.clear_selection();
        self.search_bounds.clear();
        self.search_match_count = 0;
    }

    fn render_page(&mut self, ctx: &egui::Context) {
        let Some(bytes) = &self.pdf_bytes else { return };
        let pdfium = Pdfium::default();
        let Ok(doc) = pdfium.load_pdf_from_byte_slice(bytes, None) else { return };
        let Ok(page) = doc.pages().get(self.current_page as u16) else { return };
        self.page_pts = Some((page.width().value, page.height().value));

        // Build char cache once per page
        self.char_cache.clear();
        if let Ok(text_page) = page.text() {
            for object in page.objects().iter() {
                if let Some(text_obj) = object.as_text_object() {
                    if let Ok(chars) = text_page.chars_for_object(text_obj) {
                        for ch in chars.iter() {
                            if let (Ok(bounds), Some(s)) = (ch.loose_bounds(), ch.unicode_string()) {
                                self.char_cache.push(CachedChar { bounds, text: s });
                            }
                        }
                    }
                }
            }
        }

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
        self.texture = Some(ctx.load_texture(
            "pdf-page",
            ColorImage { size: [w as usize, h as usize], pixels },
            TextureOptions::LINEAR,
        ));
        self.needs_render = false;
    }

    fn screen_to_pdf(&self, pos: Pos2) -> Option<(f32, f32)> {
        let r = self.image_rect?;
        let (pw, ph) = self.page_pts?;
        let px = ((pos.x - r.min.x) / r.width() * pw).clamp(0.0, pw);
        let py = (ph - (pos.y - r.min.y) / r.height() * ph).clamp(0.0, ph);
        Some((px, py))
    }

    fn pdf_rect_to_screen(&self, pr: &PdfRect) -> Option<Rect> {
        let r = self.image_rect?;
        let (pw, ph) = self.page_pts?;
        let sx = |x: f32| r.min.x + (x / pw) * r.width();
        let sy = |y: f32| r.min.y + ((ph - y) / ph) * r.height();
        Some(Rect::from_min_max(
            Pos2::new(sx(pr.left().value),  sy(pr.top().value)),
            Pos2::new(sx(pr.right().value), sy(pr.bottom().value)),
        ))
    }

    // Live drag selection — flow selection like Chrome.
    // Groups chars into lines by vertical proximity, then:
    //   first line:  from drag_start x → end of line
    //   middle lines: entire line
    //   last line:   start of line → drag_end x
    //   (flipped when dragging upward)
    fn update_selection(&mut self) {
        let (Some(s), Some(e)) = (self.drag_start, self.drag_end) else { return };
        let Some((sx, sy)) = self.screen_to_pdf(s) else { return };
        let Some((ex, ey)) = self.screen_to_pdf(e) else { return };

        if self.char_cache.is_empty() { return }

        // ── Group chars into lines ────────────────────────────────────────────
        // Sort a scratch index by vertical center descending (top of page first
        // in PDF coords where y increases upward).
        let mut indices: Vec<usize> = (0..self.char_cache.len()).collect();
        indices.sort_by(|&a, &b| {
            let cy = |i: usize| {
                let b = &self.char_cache[i].bounds;
                (b.bottom().value + b.top().value) * 0.5
            };
            cy(b).partial_cmp(&cy(a)).unwrap()
        });

        // Cluster into lines: new line when vertical gap > half char height
        let mut lines: Vec<Vec<usize>> = Vec::new();
        for idx in indices {
            let b = &self.char_cache[idx].bounds;
            let cy = (b.bottom().value + b.top().value) * 0.5;
            let ch = (b.top().value - b.bottom().value).abs();
            let thresh = ch * 0.6;
            if let Some(last) = lines.last_mut() {
                let last_cy = {
                    let lb = &self.char_cache[last[0]].bounds;
                    (lb.bottom().value + lb.top().value) * 0.5
                };
                if (cy - last_cy).abs() <= thresh {
                    last.push(idx);
                    continue;
                }
            }
            lines.push(vec![idx]);
        }

        // Sort chars within each line left-to-right
        for line in &mut lines {
            line.sort_by(|&a, &b| {
                let lx = |i: usize| self.char_cache[i].bounds.left().value;
                lx(a).partial_cmp(&lx(b)).unwrap()
            });
        }

        // ── Determine drag direction ──────────────────────────────────────────
        // In PDF coords y increases upward, so dragging downward = decreasing y
        let (start_x, start_y, end_x, end_y) = if sy >= ey {
            (sx, sy, ex, ey) // dragging downward
        } else {
            (ex, ey, sx, sy) // dragging upward — swap so start is always higher
        };

        // Find which line index contains start_y and end_y
        let line_cy = |line: &Vec<usize>| -> f32 {
            let b = &self.char_cache[line[0]].bounds;
            (b.bottom().value + b.top().value) * 0.5
        };

        let start_line = lines.iter().enumerate()
            .min_by(|(_, a), (_, b)| {
                (line_cy(a) - start_y).abs().partial_cmp(&(line_cy(b) - start_y).abs()).unwrap()
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        let end_line = lines.iter().enumerate()
            .min_by(|(_, a), (_, b)| {
                (line_cy(a) - end_y).abs().partial_cmp(&(line_cy(b) - end_y).abs()).unwrap()
            })
            .map(|(i, _)| i)
            .unwrap_or(lines.len().saturating_sub(1));

        // ── Select chars per line ─────────────────────────────────────────────
        let mut text = String::new();
        let mut rects = Vec::new();

        for (li, line) in lines.iter().enumerate() {
            if li < start_line || li > end_line { continue; }

            for &ci in line {
                let b = &self.char_cache[ci].bounds;
                let cx = (b.left().value + b.right().value) * 0.5;

                let selected = if start_line == end_line {
                    // Single line — use x range directly
                    let (x0, x1) = (start_x.min(end_x), start_x.max(end_x));
                    cx >= x0 && cx <= x1
                } else if li == start_line {
                    cx >= start_x  // first line: from cursor rightward
                } else if li == end_line {
                    cx <= end_x    // last line: up to cursor
                } else {
                    true           // middle lines: fully selected
                };

                if selected {
                    text.push_str(&self.char_cache[ci].text);
                    rects.push(self.char_cache[ci].bounds);
                }
            }
        }

        self.selected_text = text.trim().to_string();
        self.selected_rects = rects;
    }

    // Find which char index the click landed on
    fn char_at(&self, pos: Pos2) -> Option<usize> {
        let (px, py) = self.screen_to_pdf(pos)?;
        for (i, ch) in self.char_cache.iter().enumerate() {
            let b = &ch.bounds;
            if px >= b.left().value && px <= b.right().value
            && py >= b.bottom().value && py <= b.top().value {
                return Some(i);
            }
        }
        // Fallback: nearest char by center distance
        let mut best = None;
        let mut best_dist = f32::MAX;
        for (i, ch) in self.char_cache.iter().enumerate() {
            let b = &ch.bounds;
            let cx = (b.left().value + b.right().value) * 0.5;
            let cy = (b.bottom().value + b.top().value) * 0.5;
            let d = (cx - px).powi(2) + (cy - py).powi(2);
            if d < best_dist { best_dist = d; best = Some(i); }
        }
        best
    }

    // Double-click: select the word the clicked char belongs to.
    // A word boundary is a space or the start/end of the cache.
    fn select_word_at(&mut self, pos: Pos2) {
        let Some(idx) = self.char_at(pos) else { return };
        let is_boundary = |i: usize| self.char_cache[i].text.trim().is_empty();

        if is_boundary(idx) { return; }

        // Walk left to word start
        let mut start = idx;
        while start > 0 && !is_boundary(start - 1) { start -= 1; }

        // Walk right to word end
        let mut end = idx;
        while end + 1 < self.char_cache.len() && !is_boundary(end + 1) { end += 1; }

        let mut text = String::new();
        let mut rects = Vec::new();
        for ch in &self.char_cache[start..=end] {
            text.push_str(&ch.text);
            rects.push(ch.bounds);
        }
        self.selected_text = text.trim().to_string();
        self.selected_rects = rects;
    }

    // Triple-click: select the whole line the clicked char is on.
    // "Same line" = chars whose vertical center is within ~half a line-height of the clicked char.
    fn select_line_at(&mut self, pos: Pos2) {
        let Some(idx) = self.char_at(pos) else { return };
        let b = &self.char_cache[idx].bounds;
        let line_cy = (b.bottom().value + b.top().value) * 0.5;
        let line_h  = (b.top().value - b.bottom().value).abs();
        let thresh  = line_h * 0.7; // same line if within 70% of char height

        let mut text = String::new();
        let mut rects = Vec::new();
        for ch in &self.char_cache {
            let cb = &ch.bounds;
            let cy = (cb.bottom().value + cb.top().value) * 0.5;
            if (cy - line_cy).abs() <= thresh {
                text.push_str(&ch.text);
                rects.push(ch.bounds);
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
        let Ok(page) = doc.pages().get(self.current_page as u16) else { return };
        let Ok(text) = page.text() else { return };
        let options = PdfSearchOptions::new();
        let Ok(search) = text.search(&self.search_query, &options) else { return };
        for segments in search.iter(PdfSearchDirection::SearchForward) {
            for seg in segments.iter() {
                self.search_bounds.push(seg.bounds());
            }
        }
        self.search_match_count = self.search_bounds.len();
    }

    fn clear_selection(&mut self) {
        self.drag_start = None;
        self.drag_end = None;
        self.selected_text.clear();
        self.selected_rects.clear();
    }

    fn go_to_page(&mut self, page: usize) {
        self.current_page = page;
        self.needs_render = true;
        self.clear_selection();
        self.search_bounds.clear();
        self.search_match_count = 0;
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
}

impl eframe::App for PdfViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (open, ctrl_f, ctrl_c, pg_next, pg_prev, esc, enter) = ctx.input(|i| (
            i.key_pressed(Key::O) && i.modifiers.ctrl,
            i.key_pressed(Key::F) && i.modifiers.ctrl,
            i.key_pressed(Key::C) && i.modifiers.ctrl,
            i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::PageDown),
            i.key_pressed(Key::ArrowLeft)  || i.key_pressed(Key::PageUp),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Enter),
        ));
        if open { if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() { self.load_pdf(&p); } }
        if ctrl_f { self.show_search = !self.show_search; }
        if ctrl_c { self.copy_selection(); }
        if pg_next && self.current_page + 1 < self.total_pages { self.go_to_page(self.current_page + 1); }
        if pg_prev && self.current_page > 0                    { self.go_to_page(self.current_page - 1); }
        if esc    { self.show_search = false; self.search_bounds.clear(); }

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
                                self.needs_render = true;
                            }
                        }
                    });
                ui.separator();
                ui.add_enabled_ui(self.current_page > 0, |ui| {
                    if ui.button("◀").clicked() { self.go_to_page(self.current_page - 1); }
                });
                ui.label(format!("{} / {}",
                    if self.total_pages > 0 { self.current_page + 1 } else { 0 },
                    self.total_pages));
                ui.add_enabled_ui(self.current_page + 1 < self.total_pages, |ui| {
                    if ui.button("▶").clicked() { self.go_to_page(self.current_page + 1); }
                });
                ui.separator();
                if ui.button("−").clicked() { self.zoom = (self.zoom - 0.15).max(0.3); self.needs_render = true; }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").clicked() { self.zoom = (self.zoom + 0.15).min(3.0); self.needs_render = true; }
                ui.separator();
                if ui.button("🔍  Ctrl+F").clicked() { self.show_search = !self.show_search; }
                if !self.selected_text.is_empty() {
                    if ui.button("📋 Copy  Ctrl+C").clicked() { self.copy_selection(); }
                }
            });
            ui.add_space(4.0);
        });

        if self.show_search {
            egui::TopBottomPanel::top("search").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    let resp = ui.add_sized(
                        [280.0, 24.0],
                        egui::TextEdit::singleline(&mut self.search_input).hint_text("Search in document…"),
                    );
                    resp.request_focus();
                    let triggered = (resp.lost_focus() && enter) || ui.button("Find").clicked();
                    if triggered {
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

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if !self.selected_text.is_empty() {
                    let preview = if self.selected_text.len() > 70 {
                        format!("{}…", &self.selected_text[..70])
                    } else {
                        self.selected_text.clone()
                    };
                    ui.colored_label(Color32::from_rgb(100, 200, 255), format!("\"{}\"", preview));
                    ui.weak("— Ctrl+C to copy");
                } else {
                    ui.weak("Drag to select  •  Double-click: word  •  Triple-click: line  •  Ctrl+F: search");
                }
            });
            ui.add_space(3.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, self.theme_bg());

            if self.pdf_bytes.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new("📄  Open a PDF to get started")
                        .size(22.0).color(Color32::from_gray(140)));
                });
                return;
            }

            if self.needs_render || self.texture.is_none() {
                self.render_page(ctx);
            }

            let (tex_id, tex_w, tex_h) = {
                let Some(texture) = &self.texture else { return };
                (texture.id(), texture.size()[0], texture.size()[1])
            };
            let avail = ui.available_size();
            let display_w = (tex_w as f32).min(avail.x - 24.0);
            let display_h = tex_h as f32 * display_w / tex_w as f32;
            let display_size = Vec2::new(display_w, display_h);

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        let (rect, response) = ui.allocate_exact_size(display_size, Sense::click_and_drag());
                        self.image_rect = Some(rect);

                        if ui.is_rect_visible(rect) {
                            let painter = ui.painter();

                            painter.image(tex_id, rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE);

                            // Search highlights
                            let bounds = self.search_bounds.clone();
                            for pdf_rect in &bounds {
                                if let Some(sr) = self.pdf_rect_to_screen(pdf_rect) {
                                    painter.rect_filled(sr, 2.0, Color32::from_rgba_premultiplied(255, 215, 0, 100));
                                    painter.rect_stroke(sr, 2.0, Stroke::new(1.5, Color32::from_rgb(255, 180, 0)), egui::StrokeKind::Outside);
                                }
                            }

                            // Selection highlights
                            let sel_rects = self.selected_rects.clone();
                            for pdf_rect in &sel_rects {
                                if let Some(sr) = self.pdf_rect_to_screen(pdf_rect) {
                                    painter.rect_filled(sr, 0.0, Color32::from_rgba_premultiplied(80, 140, 255, 110));
                                }
                            }
                        }

                        if response.hovered() {
                            ctx.set_cursor_icon(CursorIcon::Text);
                        }

                        // ── Click handling ────────────────────────────────────
                        if response.clicked() {
                            let now = ctx.input(|i| i.time);
                            let pos = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(Pos2::ZERO);

                            // Count rapid clicks in the same area (within 5px, within 0.4s)
                            let same_spot = self.last_click_pos
                                .map(|p| p.distance(pos) < 5.0)
                                .unwrap_or(false);
                            let rapid = (now - self.last_click_time) < 0.4;

                            if same_spot && rapid {
                                self.click_count = (self.click_count + 1).min(3);
                            } else {
                                self.click_count = 1;
                                self.clear_selection();
                            }

                            self.last_click_pos = Some(pos);
                            self.last_click_time = now;

                            match self.click_count {
                                2 => self.select_word_at(pos),
                                3 => self.select_line_at(pos),
                                _ => {}
                            }

                            ctx.request_repaint();
                        }

                        // ── Drag handling ─────────────────────────────────────
                        if response.drag_started() {
                            self.click_count = 0; // drag cancels click-count
                            self.clear_selection();
                            self.drag_start = ctx.input(|i| i.pointer.interact_pos());
                        }
                        if response.dragged() {
                            self.drag_end = ctx.input(|i| i.pointer.interact_pos());
                            self.update_selection();
                            ctx.request_repaint();
                        }
                    });
                    ui.add_space(12.0);
                });
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
