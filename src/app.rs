use crate::pdf::{self, Doc};
use crate::search::{self, Hit};
use arboard::Clipboard;
use eframe::egui::{self, ColorImage, Context, Key, Rect, TextureHandle, TextureOptions, Vec2};

// ── Selection types ───────────────────────────────────────────────────────────

/// A position in the document: (page index, char index within that page)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DocPos {
    page: usize,
    char_idx: usize,
}

struct Selection {
    anchor: DocPos, // where drag started
    focus: DocPos,  // where drag currently ends
}

impl Selection {
    fn start(&self) -> DocPos { self.anchor.min(self.focus) }
    fn end(&self)   -> DocPos { self.anchor.max(self.focus) }

    fn contains(&self, page: usize, idx: usize) -> bool {
        let pos = DocPos { page, char_idx: idx };
        pos >= self.start() && pos <= self.end()
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct PdfViewer {
    doc: Option<Doc>,
    textures: Vec<TextureHandle>,
    texts: Vec<String>,
    scroll_offset: f32,
    scroll_velocity: f32,

    // Text selection
    selection: Option<Selection>,
    dragging: bool,
    /// Screen rects of each page from the previous frame — used for hit testing
    page_rects: Vec<Rect>,

    // Search
    search_open: bool,
    search_query: String,
    search_hits: Vec<Hit>,
    search_cursor: usize,

    status: String,
}

impl PdfViewer {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        cc.egui_ctx.options_mut(|o| { o.line_scroll_speed = 500.0; });
        cc.egui_ctx.style_mut(|s| {
            s.scroll_animation = egui::style::ScrollAnimation::none();
        });
        Self {
            doc: None,
            textures: vec![],
            texts: vec![],
            scroll_offset: 0.0,
            scroll_velocity: 0.0,
            selection: None,
            dragging: false,
            page_rects: vec![],
            search_open: false,
            search_query: String::new(),
            search_hits: vec![],
            search_cursor: 0,
            status: "Open a PDF with Ctrl+O".into(),
        }
    }

    fn open_file(&mut self, ctx: &Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            let path_str = path.to_string_lossy().to_string();
            self.status = format!("Loading {}...", path_str);
            match pdf::load(&path_str) {
                Ok(doc) => {
                    self.textures = doc.pages.iter().map(|p| {
                        let img = ColorImage::from_rgba_unmultiplied(
                            [p.width as usize, p.height as usize], &p.rgba,
                        );
                        ctx.load_texture("page", img, TextureOptions::LINEAR)
                    }).collect();
                    self.texts = doc.pages.iter().map(|p| p.text.clone()).collect();
                    self.page_rects = vec![Rect::ZERO; doc.pages.len()];
                    self.status = format!("{} — {} pages", doc.path, doc.pages.len());
                    self.doc = Some(doc);
                    self.selection = None;
                    self.search_hits.clear();
                    self.search_query.clear();
                }
                Err(e) => self.status = format!("Error: {e}"),
            }
        }
    }

    // ── Hit test: given screen pos, find (page, char_idx) ────────────────────
    fn hit_test(&self, pos: egui::Pos2) -> Option<DocPos> {
        let doc = self.doc.as_ref()?;
        for (pi, page_rect) in self.page_rects.iter().enumerate() {
            if !page_rect.contains(pos) { continue; }
            let page = &doc.pages[pi];
            if page.chars.is_empty() { return None; }
            let dw = page_rect.width();
            let dh = page_rect.height();
            let local_x = (pos.x - page_rect.min.x) / dw;
            let local_y = (pos.y - page_rect.min.y) / dh;

            // Find closest char — first try exact hit, then nearest by center
            let mut best: Option<(usize, f32)> = None;
            for (ci, c) in page.chars.iter().enumerate() {
                // Exact bounding box hit
                if local_x >= c.x && local_x <= c.x + c.w
                    && local_y >= c.y && local_y <= c.y + c.h
                {
                    return Some(DocPos { page: pi, char_idx: ci });
                }
                // Track nearest center on same horizontal band
                if local_y >= c.y - 0.005 && local_y <= c.y + c.h + 0.005 {
                    let cx = c.x + c.w * 0.5;
                    let dist = (cx - local_x).abs();
                    if best.map(|(_, d)| dist < d).unwrap_or(true) {
                        best = Some((ci, dist));
                    }
                }
            }
            if let Some((ci, _)) = best {
                return Some(DocPos { page: pi, char_idx: ci });
            }
        }
        None
    }

    // ── Copy selected text to clipboard ──────────────────────────────────────
    fn copy_selection(&mut self) {
        let text = self.selected_text();
        if !text.is_empty() {
            if let Ok(mut cb) = Clipboard::new() {
                let _ = cb.set_text(&text);
                self.status = format!("Copied {} chars", text.len());
            }
        }
    }

    fn selected_text(&self) -> String {
        let sel = match &self.selection { Some(s) => s, None => return String::new() };
        let doc = match &self.doc { Some(d) => d, None => return String::new() };
        let start = sel.start();
        let end   = sel.end();
        let mut out = String::new();
        for pi in start.page..=end.page {
            if pi >= doc.pages.len() { break; }
            let chars = &doc.pages[pi].chars;
            let from = if pi == start.page { start.char_idx } else { 0 };
            let to   = if pi == end.page   { end.char_idx.min(chars.len().saturating_sub(1)) }
                       else                { chars.len().saturating_sub(1) };
            for ci in from..=to {
                if ci < chars.len() { out.push(chars[ci].ch); }
            }
            if pi < end.page { out.push('\n'); }
        }
        out
    }

    fn copy_all(&mut self) {
        let all = self.texts.join("\n\n");
        if let Ok(mut cb) = Clipboard::new() {
            let _ = cb.set_text(all);
            self.status = "Copied all text to clipboard".into();
        }
    }

    fn run_search(&mut self) {
        self.search_hits = search::find_all(&self.texts, &self.search_query);
        self.search_cursor = 0;
        self.status = if self.search_hits.is_empty() {
            if self.search_query.is_empty() { String::new() }
            else { format!("No results for \"{}\"", self.search_query) }
        } else {
            format!("1 of {} match{}", self.search_hits.len(),
                if self.search_hits.len() == 1 { "" } else { "es" })
        };
    }

    fn search_next(&mut self) {
        if !self.search_hits.is_empty() {
            self.search_cursor = (self.search_cursor + 1) % self.search_hits.len();
            self.status = format!("{} of {} matches",
                self.search_cursor + 1, self.search_hits.len());
        }
    }

    fn search_prev(&mut self) {
        if !self.search_hits.is_empty() {
            self.search_cursor = self.search_cursor
                .checked_sub(1).unwrap_or(self.search_hits.len() - 1);
            self.status = format!("{} of {} matches",
                self.search_cursor + 1, self.search_hits.len());
        }
    }
}

impl eframe::App for PdfViewer {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {

        // ── Kinetic scrolling ────────────────────────────────────────────────
        let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_delta != 0.0 {
            self.scroll_velocity -= scroll_delta * 5.0;
        }
        if self.scroll_velocity.abs() > 0.5 {
            self.scroll_offset = (self.scroll_offset + self.scroll_velocity).max(0.0);
            self.scroll_velocity *= 0.85;
            ctx.request_repaint();
        } else {
            self.scroll_velocity = 0.0;
        }

        // ── Mouse selection ──────────────────────────────────────────────────
        let pointer = ctx.input(|i| i.pointer.clone());

        if pointer.primary_pressed() {
            // Start drag — clear old selection
            self.selection = None;
            self.dragging = true;
            if let Some(pos) = pointer.press_origin() {
                if let Some(doc_pos) = self.hit_test(pos) {
                    self.selection = Some(Selection { anchor: doc_pos, focus: doc_pos });
                }
            }
        }

        if self.dragging {
            if let Some(pos) = pointer.hover_pos() {
                if let Some(doc_pos) = self.hit_test(pos) {
                    if let Some(sel) = &mut self.selection {
                        sel.focus = doc_pos;
                    }
                }
            }
        }

        if pointer.primary_released() {
            self.dragging = false;
            // Degenerate selection (single click, no drag) → clear
            if let Some(sel) = &self.selection {
                if sel.anchor == sel.focus { self.selection = None; }
            }
        }

        // ── Keyboard shortcuts ───────────────────────────────────────────────
        let mods = ctx.input(|i| i.modifiers);

        if ctx.input(|i| i.key_pressed(Key::O)) && mods.ctrl {
            self.open_file(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::F)) && mods.ctrl {
            self.search_open = !self.search_open;
            if !self.search_open { self.search_hits.clear(); self.search_query.clear(); }
        }
        if ctx.input(|i| i.key_pressed(Key::A)) && mods.ctrl {
            if self.selection.is_some() { self.copy_selection(); }
            else { self.copy_all(); }
        }
        if ctx.input(|i| i.key_pressed(Key::C)) && mods.ctrl {
            self.copy_selection();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.search_open = false;
            self.search_hits.clear();
            self.search_query.clear();
            self.selection = None;
        }
        if self.search_open && ctx.input(|i| i.key_pressed(Key::Enter)) {
            if mods.shift { self.search_prev(); } else { self.search_next(); }
        }

        // ── Cursor: show text cursor over pages ──────────────────────────────
        if self.doc.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Text);
        }

        // ── Toolbar ──────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📂 Open (Ctrl+O)").clicked() { self.open_file(ctx); }
                if ui.button("🔍 Find (Ctrl+F)").clicked() { self.search_open = !self.search_open; }
                if ui.button("📋 Copy All (Ctrl+A)").clicked() { self.copy_all(); }
                if self.selection.is_some() {
                    if ui.button("📋 Copy Selection (Ctrl+C)").clicked() { self.copy_selection(); }
                }
            });
        });

        // ── Search bar ───────────────────────────────────────────────────────
        if self.search_open {
            egui::TopBottomPanel::top("search").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Find:");
                    let resp = ui.text_edit_singleline(&mut self.search_query);
                    if resp.changed() { self.run_search(); }
                    if !self.search_hits.is_empty() {
                        ui.label(format!("{}/{}", self.search_cursor + 1, self.search_hits.len()));
                    }
                    if ui.button("◀").clicked() { self.search_prev(); }
                    if ui.button("▶").clicked() { self.search_next(); }
                    if ui.button("✕").clicked() {
                        self.search_open = false;
                        self.search_hits.clear();
                        self.search_query.clear();
                    }
                });
            });
        }

        // ── Status bar ───────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.status);
        });

        // ── Page canvas ──────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_gray(100)))
            .show(ctx, |ui| {
                if self.textures.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(egui::RichText::new("Open a PDF to get started")
                            .size(20.0).color(egui::Color32::LIGHT_GRAY));
                    });
                    return;
                }

                let active_page = self.search_hits.get(self.search_cursor).map(|h| h.page);

                let scroll_out = egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .animated(true)
                    .vertical_scroll_offset(self.scroll_offset)
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let doc = self.doc.as_ref().unwrap();

                            for (pi, tex) in self.textures.iter().enumerate() {
                                let tex_size = tex.size_vec2();
                                let max_w = (ui.available_width() - 40.0).max(100.0);
                                let scale = (max_w / tex_size.x).min(1.0);
                                let display = Vec2::new(tex_size.x * scale, tex_size.y * scale);

                                let is_active = active_page == Some(pi);
                                let stroke = if is_active {
                                    egui::Stroke::new(3.0, egui::Color32::from_rgb(50, 140, 255))
                                } else {
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(60))
                                };

                                let _frame_resp = egui::Frame::default()
                                    .stroke(stroke)
                                    .shadow(egui::Shadow {
                                        blur: 12, spread: 0,
                                        offset: [3, 3].into(),
                                        color: egui::Color32::from_black_alpha(80),
                                    })
                                    .show(ui, |ui| {
                                        let img_resp = ui.image((tex.id(), display));

                                        // Record this page's screen rect for hit testing
                                        if pi < self.page_rects.len() {
                                            self.page_rects[pi] = img_resp.rect;
                                        }

                                        // Draw selection highlights on top of the image
                                        let painter = ui.painter_at(img_resp.rect);
                                        let page = &doc.pages[pi];
                                        let dw = img_resp.rect.width();
                                        let dh = img_resp.rect.height();

                                        // Selection highlight (blue)
                                        if let Some(sel) = &self.selection {
                                            for (ci, c) in page.chars.iter().enumerate() {
                                                if sel.contains(pi, ci) {
                                                    let r = Rect::from_min_size(
                                                        egui::pos2(
                                                            img_resp.rect.min.x + c.x * dw,
                                                            img_resp.rect.min.y + c.y * dh,
                                                        ),
                                                        Vec2::new(c.w * dw, c.h * dh),
                                                    );
                                                    painter.rect_filled(
                                                        r,
                                                        0.0,
                                                        egui::Color32::from_rgba_unmultiplied(70, 140, 255, 100),
                                                    );
                                                }
                                            }
                                        }

                                        // Search highlights (yellow)
                                        for hit in &self.search_hits {
                                            if hit.page != pi { continue; }
                                            // hit.start/end are byte offsets into page text
                                            // map them to char indices
                                            let mut byte_pos = 0usize;
                                            for (_ci, c) in page.chars.iter().enumerate() {
                                                if byte_pos >= hit.start && byte_pos < hit.end {
                                                    let r = Rect::from_min_size(
                                                        egui::pos2(
                                                            img_resp.rect.min.x + c.x * dw,
                                                            img_resp.rect.min.y + c.y * dh,
                                                        ),
                                                        Vec2::new(c.w * dw, c.h * dh),
                                                    );
                                                    let color = if self.search_hits.get(self.search_cursor)
                                                        .map(|h| h.page == pi && h.start == hit.start)
                                                        .unwrap_or(false)
                                                    {
                                                        egui::Color32::from_rgba_unmultiplied(255, 140, 0, 150)
                                                    } else {
                                                        egui::Color32::from_rgba_unmultiplied(255, 230, 0, 100)
                                                    };
                                                    painter.rect_filled(r, 0.0, color);
                                                }
                                                byte_pos += c.ch.len_utf8();
                                            }
                                        }
                                    });

                                ui.add_space(16.0);
                            }
                        });
                    });

                self.scroll_offset = scroll_out.state.offset.y;
            });
    }
}