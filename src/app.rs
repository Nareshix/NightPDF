use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Rect, Sense, Stroke};
use rfd::FileDialog;

use crate::theme::{self, THEMES};
use crate::viewer::PdfViewer;

impl eframe::App for PdfViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut do_copy = false;

        let (open, ctrl_f, ctrl_a, esc, enter) = ctx.input_mut(|i| {
            // Check for the native OS copy event (egui intercepts Ctrl+C / Cmd+C and creates this)
            if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                do_copy = true;
            }
            // Fallback: check raw keystrokes just in case
            if i.consume_key(egui::Modifiers::COMMAND, Key::C) || i.consume_key(egui::Modifiers::CTRL, Key::C) {
                do_copy = true;
            }

            (
                i.consume_key(egui::Modifiers::COMMAND, Key::O) || i.consume_key(egui::Modifiers::CTRL, Key::O),
                i.consume_key(egui::Modifiers::COMMAND, Key::F) || i.consume_key(egui::Modifiers::CTRL, Key::F),
                i.consume_key(egui::Modifiers::COMMAND, Key::A) || i.consume_key(egui::Modifiers::CTRL, Key::A),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Enter),
            )
        });

        if open {
            if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                self.load_pdf(&p);
            }
        }
        if ctrl_f {
            self.show_search = !self.show_search;
        }
        if do_copy {
            if !self.selected_text.is_empty() {
                ctx.copy_text(self.selected_text.clone());
            }
        }
        if ctrl_a {
            self.select_all();
        }
        if esc {
            self.show_search = false;
            self.search_bounds.clear();
        }

        // ── Smooth scroll physics ─────────────────────────────────────────────
        let (raw_scroll, dt) = ctx.input(|i| (i.raw_scroll_delta.y, i.predicted_dt));

        if raw_scroll.abs() > 0.1 {
            self.scroll_velocity += raw_scroll * 25.0;
            self.scroll_velocity = self.scroll_velocity.clamp(-8000.0, 8000.0);
        }

        let friction = (-12.0_f32 * dt).exp();
        self.scroll_velocity *= friction;
        self.scroll_offset -= self.scroll_velocity * dt;
        self.scroll_offset = self.scroll_offset.max(0.0);

        if self.scroll_velocity.abs() > 1.0 {
            ctx.request_repaint();
        }

        // ── Toolbar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("📂 Open  Ctrl+O").clicked() {
                    if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                        self.load_pdf(&p);
                    }
                }
                ui.separator();
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(THEMES[self.theme_idx].0)
                    .show_ui(ui, |ui| {
                        for (i, (name, _, _, _)) in THEMES.iter().enumerate() {
                            if ui.selectable_label(self.theme_idx == i, *name).clicked() {
                                self.theme_idx = i;
                                self.page_cache.clear();
                                self.page_cache_order.clear();
                            }
                        }
                    });
                ui.separator();
                ui.label(format!("{} pages", self.total_pages));
                ui.separator();
                if ui.button("−").clicked() {
                    self.zoom = (self.zoom - 0.15).max(0.3);
                    self.page_cache.clear();
                    self.page_cache_order.clear();
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").clicked() {
                    self.zoom = (self.zoom + 0.15).min(3.0);
                    self.page_cache.clear();
                    self.page_cache_order.clear();
                }
                ui.separator();
                if ui.button("🔍  Ctrl+F").clicked() {
                    self.show_search = !self.show_search;
                }
                if !self.selected_text.is_empty() {
                    if ui.button("📋 Copy  Ctrl+C").clicked() {
                        ctx.copy_text(self.selected_text.clone());
                    }
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
                        egui::TextEdit::singleline(&mut self.search_input)
                            .hint_text("Search all pages…"),
                    );
                    resp.request_focus();

                    // Live search as you type
                    if resp.changed() {
                        self.search_query = self.search_input.clone();
                        self.do_search();
                    }

                    if (resp.has_focus() && enter) || ui.button("Find").clicked() {
                        self.search_query = self.search_input.clone();
                        self.do_search();
                    }

                    if self.search_match_count > 0 {
                        ui.colored_label(
                            Color32::from_rgb(100, 220, 120),
                            format!(
                                "{} match{}",
                                self.search_match_count,
                                if self.search_match_count == 1 {
                                    ""
                                } else {
                                    "es"
                                }
                            ),
                        );
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
                let preview = if self.selected_text.chars().count() > 70 {
                    let truncated: String = self.selected_text.chars().take(70).collect();
                    format!("{}…", truncated.replace('\n', " "))
                } else {
                    self.selected_text.replace('\n', " ")
                };
                    ui.colored_label(Color32::from_rgb(100, 200, 255), format!("\"{}\"", preview));
                    ui.weak("— Ctrl+C to copy");
                } else {
                    ui.weak("Drag to select  •  Double-click: word  •  Triple-click: line  •  Ctrl+A: Select All  •  Ctrl+F: search all pages");
                }
            });
            ui.add_space(3.0);
        });

        // ── Main scroll area ──────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            let bg = theme::theme_bg(self.theme_idx);
            ui.painter()
                .rect_filled(ui.available_rect_before_wrap(), 0.0, bg);

            if self.document.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("📄  Open a PDF to get started")
                            .size(22.0)
                            .color(Color32::from_gray(140)),
                    );
                });
                return;
            }

            let avail_w = ui.available_width();
            let viewport_rect = ui.clip_rect();

            let scroll_output = egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .vertical_scroll_offset(self.scroll_offset)
                .show(ui, |ui| {
                    ui.add_space(12.0);

                    for page_idx in 0..self.total_pages {
                        let size = self.page_display_size(page_idx, avail_w);
                        let side_pad = ((avail_w - size.x) / 2.0).max(0.0);

                        ui.horizontal(|ui| {
                            ui.add_space(side_pad);

                            let (page_rect, response) =
                                ui.allocate_exact_size(size, Sense::click_and_drag());

                            if self.page_screen_rects.len() > page_idx {
                                self.page_screen_rects[page_idx] = page_rect;
                            }

                            let is_visible = viewport_rect.intersects(page_rect);

                            if is_visible {
                                self.ensure_page_rendered(page_idx, ctx);

                                if let Some(texture) = self.page_cache.get(&page_idx) {
                                    let tex_id = texture.id();
                                    let painter = ui.painter();

                                    painter.image(
                                        tex_id,
                                        page_rect,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                        Color32::WHITE,
                                    );

                                    let search_rects: Vec<_> = self
                                        .search_bounds
                                        .iter()
                                        .filter(|(pi, _)| *pi == page_idx)
                                        .map(|(_, r)| *r)
                                        .collect();
                                    for pr in &search_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx)
                                        {
                                            painter.rect_filled(
                                                sr,
                                                2.0,
                                                Color32::from_rgba_premultiplied(0, 150, 255, 80), // High-visibility Cyan
                                            );
                                            painter.rect_stroke(
                                                sr,
                                                2.0,
                                                Stroke::new(1.5, Color32::from_rgb(0, 150, 255)),
                                                egui::StrokeKind::Outside,
                                            );
                                        }
                                    }

                                    let sel_rects: Vec<_> = self
                                        .selected_rects
                                        .iter()
                                        .filter(|(pi, _)| *pi == page_idx)
                                        .map(|(_, r)| *r)
                                        .collect();
                                    for pr in &sel_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx)
                                        {
                                            painter.rect_filled(
                                                sr,
                                                0.0,
                                                Color32::from_rgba_premultiplied(80, 140, 255, 110),
                                            );
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
                                let pos = ctx
                                    .input(|i| i.pointer.interact_pos())
                                    .unwrap_or(Pos2::ZERO);
                                let same_spot = self
                                    .last_click_pos
                                    .map(|p| p.distance(pos) < 5.0)
                                    .unwrap_or(false);
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
                                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                                    if let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) {
                                        self.drag_start = Some((page_idx, Pos2::new(px, py)));
                                        self.drag_end = self.drag_start; // init to prevent null
                                    }
                                }
                            }

                            if response.dragged() {
                                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                                    let target_page = self
                                        .page_at_pos(pos)
                                        .or_else(|| self.nearest_page_to_pos(pos));

                                    if let Some(curr_page) = target_page {
                                        if let Some((px, py)) =
                                            self.screen_to_pdf_page(pos, curr_page)
                                        {
                                            self.drag_end = Some((curr_page, Pos2::new(px, py)));
                                            self.update_selection();
                                            ctx.request_repaint();
                                        }
                                    }
                                }
                            }
                        });

                        ui.add_space(8.0);
                    }

                    ui.add_space(12.0);
                });

            // Sync the scroll offset with the UI
            self.scroll_offset = scroll_output.state.offset.y;

            // ── AUTO-SCROLL WHEN DRAGGING ─────────────────────────────────────────
            if self.drag_start.is_some() && ctx.input(|i| i.pointer.primary_down()) {
                if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                    let scroll_zone = 60.0;
                    let scroll_speed = 1200.0;
                    let dt = ctx.input(|i| i.predicted_dt);
                    let mut auto_scrolled = false;

                    if pos.y > viewport_rect.bottom() - scroll_zone {
                        let intensity =
                            (pos.y - (viewport_rect.bottom() - scroll_zone)) / scroll_zone;
                        self.scroll_offset += scroll_speed * intensity.clamp(0.0, 2.0) * dt;
                        auto_scrolled = true;
                    }
                    else if pos.y < viewport_rect.top() + scroll_zone {
                        let intensity = ((viewport_rect.top() + scroll_zone) - pos.y) / scroll_zone;
                        self.scroll_offset -= scroll_speed * intensity.clamp(0.0, 2.0) * dt;
                        self.scroll_offset = self.scroll_offset.max(0.0);
                        auto_scrolled = true;
                    }

                    if auto_scrolled {
                        let target_page = self
                            .page_at_pos(pos)
                            .or_else(|| self.nearest_page_to_pos(pos));
                        if let Some(curr_page) = target_page {
                            if let Some((px, py)) = self.screen_to_pdf_page(pos, curr_page) {
                                self.drag_end = Some((curr_page, Pos2::new(px, py)));
                                self.update_selection();
                            }
                        }
                        ctx.request_repaint();
                    }
                }
            }
        });
    }
}