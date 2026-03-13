use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Rect, Sense, Stroke};
use rfd::FileDialog;

use crate::theme::{self, THEMES};
use crate::viewer::PdfViewer;

impl eframe::App for PdfViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check if the user is typing in a text box (like the search bar)
        let wants_keyboard = ctx.wants_keyboard_input();
        let mut do_copy = false;

        let (open, ctrl_f, ctrl_a, esc) = ctx.input_mut(|i| {
            // Only steal the copy event if the search box IS NOT focused
            if !wants_keyboard {
                if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                    do_copy = true;
                }
                if i.consume_key(egui::Modifiers::COMMAND, Key::C) || i.consume_key(egui::Modifiers::CTRL, Key::C) {
                    do_copy = true;
                }
            }

            (
                i.consume_key(egui::Modifiers::COMMAND, Key::O) || i.consume_key(egui::Modifiers::CTRL, Key::O),
                i.consume_key(egui::Modifiers::COMMAND, Key::F) || i.consume_key(egui::Modifiers::CTRL, Key::F),
                // Only steal Ctrl+A if the search box IS NOT focused!
                !wants_keyboard && (i.consume_key(egui::Modifiers::COMMAND, Key::A) || i.consume_key(egui::Modifiers::CTRL, Key::A)),
                i.key_pressed(Key::Escape),
            )
        });

        if open {
            if let Some(p) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                self.load_pdf(&p);
            }
        }
        if ctrl_f {
            self.show_search = !self.show_search;
            if !self.show_search {
                self.search_bounds.clear();
                self.search_match_count = 0;
                self.search_current_match = 0;
            }
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
            self.search_match_count = 0;
            self.search_current_match = 0;
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
                    if !self.show_search {
                        self.search_bounds.clear();
                        self.search_match_count = 0;
                        self.search_current_match = 0;
                    }
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

                    // 1. Detect Enter key BEFORE the TextEdit has a chance to swallow it
                    let mut enter_pressed = false;
                    let mut shift_pressed = false;
                    ui.input(|i| {
                        if i.key_pressed(Key::Enter) {
                            enter_pressed = true;
                            shift_pressed = i.modifiers.shift;
                        }
                    });

                    // 2. Draw the Search text box
                    let resp = ui.add_sized(
                        [280.0, 24.0],
                        egui::TextEdit::singleline(&mut self.search_input)
                            .hint_text("Search all pages…"),
                    );

                    // Only request focus right when they press Ctrl+F
                    if ctrl_f {
                        resp.request_focus();
                    }

                    // Live search as you type
                    if resp.changed() {
                        self.search_query = self.search_input.clone();
                        self.do_search();
                    }

                    // 3. Process Enter jumps ONLY if the Search box is active.
                    // (Singleline TextEdits intentionally drop focus when Enter is pressed,
                    // so we check if it currently has focus OR if it just lost it this exact frame).
                    if resp.has_focus() || (resp.lost_focus() && enter_pressed) {
                        if enter_pressed {
                            if shift_pressed {
                                self.prev_search_match();
                            } else {
                                self.next_search_match();
                            }
                            // Re-grab focus instantly so the user can just mash Enter repeatedly!
                            resp.request_focus();
                        }
                    }

                    if self.search_match_count > 0 {
                        // Navigation Arrows (using safe standard characters)
                        if ui.button(" < ").on_hover_text("Previous match").clicked() {
                            self.prev_search_match();
                        }
                        if ui.button(" > ").on_hover_text("Next match").clicked() {
                            self.next_search_match();
                        }

                        // Match Counter (e.g. 1 / 3)
                        ui.colored_label(
                            Color32::from_rgb(100, 220, 120),
                            format!("{} / {}", self.search_current_match + 1, self.search_match_count),
                        );
                    } else if !self.search_query.is_empty() {
                        ui.colored_label(Color32::from_rgb(255, 100, 100), "No matches");
                    }

                    if ui.button(" X ").clicked() {
                        self.show_search = false;
                        self.search_bounds.clear();
                        self.search_match_count = 0;
                        self.search_current_match = 0;
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

            // ── SCROLL TO MATCH LOGIC ─────────────────────────────────────────
            // Calculates the exact pixel Y offset of the target search match
            if self.jump_to_match && self.search_match_count > 0 {
                self.jump_to_match = false;
                let (page_idx, rect) = self.search_bounds[self.search_current_match];

                let mut y_offset = 12.0; // initial top space
                for i in 0..page_idx {
                    y_offset += self.page_display_size(i, avail_w).y + 8.0; // 8.0 is padding between pages
                }

                if let Some(info) = self.page_infos.get(page_idx) {
                    let page_size = self.page_display_size(page_idx, avail_w);
                    let top_pt = rect.top().value;
                    // Compute absolute distance from top of document
                    let rel_y = ((info.height_pts - top_pt) / info.height_pts) * page_size.y;

                    // Automatically Scroll - minus 100 padding so the text isn't glued to the top of screen
                    self.scroll_offset = (y_offset + rel_y - 100.0).max(0.0);
                    self.scroll_velocity = 0.0; // Stop any existing physics scroll velocity
                }
            }

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

                                    // Render Search Highlights
                                    let mut search_rects = Vec::new();
                                    let mut active_rect = None;

                                    // Filter out regular vs active search hits
                                    for (i, &(pi, pr)) in self.search_bounds.iter().enumerate() {
                                        if pi == page_idx {
                                            if i == self.search_current_match {
                                                active_rect = Some(pr);
                                            } else {
                                                search_rects.push(pr);
                                            }
                                        }
                                    }

                                    // Render general background matches (Cyan)
                                    for pr in search_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(&pr, page_idx) {
                                            painter.rect_filled(
                                                sr,
                                                2.0,
                                                Color32::from_rgba_premultiplied(0, 150, 255, 80),
                                            );
                                            painter.rect_stroke(
                                                sr,
                                                2.0,
                                                Stroke::new(1.5, Color32::from_rgb(0, 150, 255)),
                                                egui::StrokeKind::Outside,
                                            );
                                        }
                                    }

                                    // Render the ACTIVE current match on top (Bright Orange)
                                    if let Some(pr) = active_rect {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(&pr, page_idx) {
                                            painter.rect_filled(
                                                sr,
                                                2.0,
                                                Color32::from_rgba_premultiplied(255, 150, 0, 120),
                                            );
                                            painter.rect_stroke(
                                                sr,
                                                2.0,
                                                Stroke::new(2.5, Color32::from_rgb(255, 200, 0)),
                                                egui::StrokeKind::Outside,
                                            );
                                        }
                                    }

                                    // Render Text Selection Highlights (Connected & Merged)
                                    let sel_rects: Vec<_> = self
                                        .selected_rects
                                        .iter()
                                        .filter(|(pi, _)| *pi == page_idx)
                                        .map(|(_, r)| *r)
                                        .collect();

                                    // Convert to screen coordinates
                                    let mut screen_rects = Vec::new();
                                    for pr in &sel_rects {
                                        if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx) {
                                            screen_rects.push(sr);
                                        }
                                    }

                                    // Merge adjacent characters/words on the same line into solid blocks
                                    let mut merged_rects: Vec<Rect> = Vec::new();
                                    for sr in screen_rects {
                                        if let Some(last) = merged_rects.last_mut() {
                                            let center_y_diff = (sr.center().y - last.center().y).abs();
                                            let height = sr.height().min(last.height());

                                            // If they are on the same line (centers are close)
                                            if center_y_diff < height * 0.5 {
                                                let gap = sr.min.x - last.max.x;
                                                // If they are close horizontally (bridges the gap of a spacebar)
                                                if gap < height * 2.0 && gap > -height {
                                                    *last = last.union(sr); // Merge them into one big rectangle!
                                                    continue;
                                                }
                                            }
                                        }
                                        merged_rects.push(sr);
                                    }

                                    // Draw the merged continuous blocks
                                    for sr in merged_rects {
                                        painter.rect_filled(
                                            sr,
                                            2.0, // Added a slight 2.0 pixel curve to the edges
                                            Color32::from_rgba_premultiplied(80, 140, 255, 110),
                                        );
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