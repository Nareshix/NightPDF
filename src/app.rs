use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Rect, Sense, Stroke};
use rfd::FileDialog;
use std::time::Instant;

use crate::scroll::{accumulate_velocity, wheel_detent_step, KineticScrolling, MAGIC_SCROLL_FACTOR};
use crate::theme::{self, THEMES};
use crate::viewer::PdfViewer;

impl eframe::App for PdfViewer {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_bookmark();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let wants_keyboard = ctx.wants_keyboard_input();
        let mut do_copy = false;

        // TODO: enum
        let (open, ctrl_f, ctrl_a, esc, ctrl_g, ctrl_plus, ctrl_minus, ctrl_h, ctrl_r) = ctx
            .input_mut(|i| {
                if !wants_keyboard {
                    if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                        do_copy = true;
                    }
                    if i.consume_key(egui::Modifiers::COMMAND, Key::C)
                        || i.consume_key(egui::Modifiers::CTRL, Key::C)
                    {
                        do_copy = true;
                    }
                }

                (
                    i.consume_key(egui::Modifiers::COMMAND, Key::O)
                        || i.consume_key(egui::Modifiers::CTRL, Key::O),
                    i.consume_key(egui::Modifiers::COMMAND, Key::F)
                        || i.consume_key(egui::Modifiers::CTRL, Key::F),
                    !wants_keyboard
                        && (i.consume_key(egui::Modifiers::COMMAND, Key::A)
                            || i.consume_key(egui::Modifiers::CTRL, Key::A)),
                    i.key_pressed(Key::Escape),
                    i.consume_key(egui::Modifiers::COMMAND, Key::G)
                        || i.consume_key(egui::Modifiers::CTRL, Key::G),
                    i.consume_key(egui::Modifiers::CTRL, Key::Equals)
                        || i.consume_key(egui::Modifiers::COMMAND, Key::Equals),
                    i.consume_key(egui::Modifiers::CTRL, Key::Minus)
                        || i.consume_key(egui::Modifiers::COMMAND, Key::Minus),
                    i.consume_key(egui::Modifiers::CTRL, Key::H)
                        || i.consume_key(egui::Modifiers::COMMAND, Key::H),
                    i.consume_key(egui::Modifiers::CTRL, Key::R)
                        || i.consume_key(egui::Modifiers::COMMAND, Key::R),
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
        if ctrl_g && self.total_pages > 0 {
            self.show_jump = true;
            self.jump_input.clear();
            self.jump_error = false;
        }
        if do_copy && !self.selected_text.is_empty() {
            ctx.copy_text(self.selected_text.clone());
        }
        if ctrl_a {
            self.select_all();
        }
        if esc {
            self.show_search = false;
            self.search_bounds.clear();
            self.search_match_count = 0;
            self.search_current_match = 0;

            self.show_jump = false;
            self.jump_error = false;
        }

        if ctrl_h {
            if self.show_toolbar {
                self.show_toolbar = false;
                self.toolbar_hover_shown = false;
            } else {
                self.show_toolbar = true;
                self.toolbar_hover_shown = false;
            }
        }
        if ctrl_r {
            self.zoom = 0.75;
            self.kinetic = None;
            self.page_cache.clear();
            self.page_cache_order.clear();
            self.show_zoom_input = false;
        }
        if ctrl_f || ctrl_g {
            self.show_toolbar = true;
            self.toolbar_hover_shown = false;
        }

        if !self.show_toolbar {
            let near_top =
                ctx.input(|i| i.pointer.latest_pos().map(|p| p.y < 8.0).unwrap_or(false));
            if near_top {
                self.show_toolbar = true;
                self.toolbar_hover_shown = true;
            }
        } else if self.toolbar_hover_shown {
            // hide again once mouse moves away from toolbar area
            let still_in_toolbar =
                ctx.input(|i| i.pointer.latest_pos().map(|p| p.y < 40.0).unwrap_or(false));
            if !still_in_toolbar {
                self.show_toolbar = false;
                self.toolbar_hover_shown = false;
            }
        }
        if ctrl_plus {
            let old_zoom = self.zoom;
            self.zoom = (self.zoom + 0.25).min(3.0);
            self.offset *= (self.zoom / old_zoom) as f64;
            self.kinetic = None;
            self.page_cache.clear();
            self.page_cache_order.clear();
        }
        if ctrl_minus {
            let old_zoom = self.zoom;
            self.zoom = (self.zoom - 0.25).max(0.3);
            self.offset *= (self.zoom / old_zoom) as f64;
            self.kinetic = None;
            self.page_cache.clear();
            self.page_cache_order.clear();
        }
        if self.show_toolbar {
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if self.toolbar_hover_shown {
                        if ui
                            .button("S")
                            .on_hover_text("Pin toolbar (Ctrl+H)")
                            .clicked()
                        {
                            self.show_toolbar = true;
                            self.toolbar_hover_shown = false;
                        }
                    } else if ui
                        .button("H")
                        .on_hover_text("Hide toolbar (Ctrl+H)")
                        .clicked()
                    {
                        self.show_toolbar = false;
                        self.toolbar_hover_shown = false;
                    }
                    ui.separator();
                    if ui
                        .button("📂")
                        .on_hover_text("Open Folder (Ctrl+O)")
                        .clicked()
                    {
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

                    // ── Jump to Page ──
                    if self.total_pages > 0 {
                        if self.show_jump {
                            let mut enter_pressed = false;
                            ui.input(|i| {
                                if i.key_pressed(Key::Enter) {
                                    enter_pressed = true;
                                }
                            });

                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.jump_input)
                                    .desired_width(40.0)
                                    .hint_text("#"),
                            );

                            if ctrl_g {
                                resp.request_focus();
                            }

                            ui.label(format!("/ {}", self.total_pages));

                            if (resp.has_focus() || resp.lost_focus()) && enter_pressed {
                                if let Ok(p) = self.jump_input.trim().parse::<usize>() {
                                    if p > 0 && p <= self.total_pages {
                                        self.target_scroll_page = Some(p - 1);
                                        self.show_jump = false;
                                        self.jump_error = false;
                                    } else {
                                        self.jump_error = true;
                                    }
                                } else {
                                    self.jump_error = true;
                                }
                            }

                            if self.jump_error {
                                ui.colored_label(Color32::from_rgb(255, 100, 100), "Invalid page");
                            }

                            if ui.button("❌").clicked() {
                                self.show_jump = false;
                                self.jump_error = false;
                            }
                        } else if ui
                            .button(format!(
                                "📄 {} / {}",
                                self.current_page + 1,
                                self.total_pages
                            ))
                            .on_hover_text("Jump to page (Ctrl+G)")
                            .clicked()
                        {
                            self.show_jump = true;
                            self.jump_input.clear();
                            self.jump_error = false;
                        }
                    } else {
                        ui.label("📄 0 pages");
                    }

                    ui.separator();
                    if ui.button("−").clicked() {
                        let old_zoom = self.zoom;
                        self.zoom = (self.zoom - 0.25).max(0.3);
                        self.offset *= (self.zoom / old_zoom) as f64;
                        self.kinetic = None;
                        self.page_cache.clear();
                        self.page_cache_order.clear();
                        self.show_zoom_input = false;
                    }
                    if self.show_zoom_input {
                        let mut enter_pressed = false;
                        ui.input(|i| {
                            if i.key_pressed(Key::Enter) {
                                enter_pressed = true;
                            }
                        });
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.zoom_input)
                                .desired_width(48.0)
                                .hint_text("%"),
                        );
                        resp.request_focus();
                        if enter_pressed || resp.lost_focus() {
                            let cleaned = self.zoom_input.trim().trim_end_matches('%');
                            if let Ok(pct) = cleaned.parse::<f32>() {
                                let old_zoom = self.zoom;
                                self.zoom = (pct / 100.0).clamp(0.3, 3.0);
                                self.offset *= (self.zoom / old_zoom) as f64;
                                self.kinetic = None;
                                self.page_cache.clear();
                                self.page_cache_order.clear();
                            }
                            self.show_zoom_input = false;
                        }
                    } else if ui
                        .button(format!("{:.0}%", self.zoom * 100.0))
                        .on_hover_text("Click to enter zoom %")
                        .clicked()
                    {
                        self.zoom_input = format!("{:.0}", self.zoom * 100.0);
                        self.show_zoom_input = true;
                    }
                    if ui.button("+").clicked() {
                        let old_zoom = self.zoom;
                        self.zoom = (self.zoom + 0.25).min(3.0);
                        self.offset *= (self.zoom / old_zoom) as f64;
                        self.kinetic = None;
                        self.page_cache.clear();
                        self.page_cache_order.clear();
                        self.show_zoom_input = false;
                    }
                    if ui
                        .button("↺")
                        .on_hover_text("Reset zoom to 75% (CTRL+R)")
                        .clicked()
                    {
                        self.zoom = 0.75;
                        self.kinetic = None;
                        self.page_cache.clear();
                        self.page_cache_order.clear();
                        self.show_zoom_input = false;
                    }
                    ui.separator();
                    if ui.button("🔍").on_hover_text("Search (Ctrl+F)").clicked() {
                        self.show_search = !self.show_search;
                        if !self.show_search {
                            self.search_bounds.clear();
                            self.search_match_count = 0;
                            self.search_current_match = 0;
                        }
                    }
                    if !self.selected_text.is_empty() && ui.button("📋 Copy  Ctrl+C").clicked() {
                        ctx.copy_text(self.selected_text.clone());
                    }
                    if let Some(path) = self.current_file_path.clone() {
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&path)
                            .to_string();
                        ui.separator();
                        ui.label(&name).on_hover_text(&path);
                    }
                });

                ui.add_space(4.0);
            });
        }

        // Search bar
        if self.show_search {
            egui::TopBottomPanel::top("search").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("🔍");

                    let mut enter_pressed = false;
                    let mut shift_pressed = false;
                    ui.input(|i| {
                        if i.key_pressed(Key::Enter) {
                            enter_pressed = true;
                            shift_pressed = i.modifiers.shift;
                        }
                    });

                    let resp = ui.add_sized(
                        [280.0, 24.0],
                        egui::TextEdit::singleline(&mut self.search_input)
                            .hint_text("Search all pages…"),
                    );

                    if ctrl_f {
                        resp.request_focus();
                    }

                    if resp.changed() {
                        self.search_query = self.search_input.clone();
                        self.do_search();
                    }

                    if (resp.has_focus() || (resp.lost_focus() && enter_pressed)) && enter_pressed {
                        if shift_pressed {
                            self.prev_search_match();
                        } else {
                            self.next_search_match();
                        }
                        resp.request_focus();
                    }

                    if self.search_match_count > 0 {
                        if ui.button(" < ").on_hover_text("Previous match").clicked() {
                            self.prev_search_match();
                        }
                        if ui.button(" > ").on_hover_text("Next match").clicked() {
                            self.next_search_match();
                        }
                        ui.colored_label(
                            Color32::from_rgb(100, 220, 120),
                            format!(
                                "{} / {}",
                                self.search_current_match + 1,
                                self.search_match_count
                            ),
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

        // ── Main scroll area ──────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let bg = theme::theme_bg(self.theme_idx);
                let avail_rect = ui.available_rect_before_wrap();
                ui.painter().rect_filled(avail_rect, 0.0, bg);

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

                let avail_w = avail_rect.width();
                let view_h = avail_rect.height() as f64;

                if (avail_w - self.last_avail_w).abs() > 1.0 {
                    self.page_cache.clear();
                    self.page_cache_order.clear();
                    self.last_avail_w = avail_w;
                }

                let content_h = self.total_content_height(avail_w) as f64;
                let max_offset = (content_h - view_h).max(0.0);
                let step = wheel_detent_step(view_h);

                // ── Kinetic scroll input ──────────────────────────────────────
                let (raw_delta, is_trackpad) = ctx.input(|i| {
                    let trackpad = i.smooth_scroll_delta != i.raw_scroll_delta;
                    (i.raw_scroll_delta.y, trackpad)
                });
                self.is_trackpad = is_trackpad;

                if raw_delta != 0.0 {
                    if let Some(k) = &mut self.kinetic {
                        k.stop();
                    }
                    let scaled = if is_trackpad {
                        -(raw_delta as f64) * MAGIC_SCROLL_FACTOR
                    } else {
                        -(raw_delta as f64) / 50.0 * step
                    };
                    self.tracker.push(scaled);
                    self.last_scroll = Some(Instant::now());
                    self.offset = (self.offset + scaled).clamp(0.0, max_offset);
                    ctx.request_repaint();
                } else if self
                    .last_scroll
                    .is_some_and(|t| t.elapsed().as_millis() > 30)
                {
                    let mut vel = self.tracker.velocity();
                    if vel.abs() > 20.0 {
                        accumulate_velocity(&mut self.kinetic, &mut vel);
                        self.kinetic =
                            Some(KineticScrolling::new(0.0, max_offset, self.offset, vel));
                    }
                    self.last_scroll = None;
                    self.tracker.clear();
                }

                if let Some(k) = &mut self.kinetic {
                    let (pos, running) = k.tick();
                    self.offset = pos.clamp(0.0, max_offset);
                    if running {
                        ctx.request_repaint();
                    } else {
                        self.kinetic = None;
                    }
                }

                // ── Jump to page ──────────────────────────────────────────────
                let page_starts = self.page_y_starts(avail_w);

                if let Some(target) = self.target_scroll_page.take() {
                    if let Some(&y) = page_starts.get(target) {
                        self.offset = (y as f64).clamp(0.0, max_offset);
                        self.kinetic = None;
                    }
                }

                // ── Jump to search match ──────────────────────────────────────
                if self.jump_to_match {
                    self.jump_to_match = false;
                    // Extract before mutably borrowing self
                    let jump_info = self
                        .search_bounds
                        .get(self.search_current_match)
                        .map(|(mp, pr)| (*mp, pr.top().value));
                    if let Some((match_page, pr_top)) = jump_info {
                        let page_size = self.page_display_size(match_page, avail_w);
                        let height_pts = self.page_infos[match_page].height_pts;
                        if let Some(&page_y) = page_starts.get(match_page) {
                            let rel_y = (height_pts - pr_top) / height_pts * page_size.y;
                            let abs_y = page_y as f64 + rel_y as f64;
                            self.offset = (abs_y - view_h / 2.0).clamp(0.0, max_offset);
                            self.kinetic = None;
                        }
                    }
                }

                // ── Page rendering loop ───────────────────────────────────────
                let viewport_rect = avail_rect;
                let viewport_center_y = viewport_rect.center().y;
                let mut best_page = self.current_page;
                let mut best_dist = f32::MAX;

                ui.set_clip_rect(viewport_rect);

                for page_idx in 0..self.total_pages {
                    let size = self.page_display_size(page_idx, avail_w);
                    let side_pad = ((avail_w - size.x) / 2.0).max(0.0);
                    let y = page_starts[page_idx] - self.offset as f32 + viewport_rect.min.y;
                    let page_rect = Rect::from_min_size(
                        Pos2::new(viewport_rect.min.x + side_pad, y),
                        size,
                    );

                    if self.page_screen_rects.len() > page_idx {
                        self.page_screen_rects[page_idx] = page_rect;
                    }

                    let is_visible = viewport_rect.intersects(page_rect);

                    if is_visible {
                        let dist = (page_rect.center().y - viewport_center_y).abs();
                        if dist < best_dist {
                            best_dist = dist;
                            best_page = page_idx;
                        }

                        self.ensure_page_rendered(page_idx, ctx, size.x);

                        let painter = ui.painter_at(viewport_rect);
                        if let Some(texture) = self.page_cache.get(&page_idx) {
                            let tex_id = texture.id();

                            painter.image(
                                tex_id,
                                page_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );

                            // Search highlights
                            let mut search_rects = Vec::new();
                            let mut active_rect = None;

                            for (i, &(pi, pr)) in self.search_bounds.iter().enumerate() {
                                if pi == page_idx {
                                    if i == self.search_current_match {
                                        active_rect = Some(pr);
                                    } else {
                                        search_rects.push(pr);
                                    }
                                }
                            }

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

                            // Text selection highlights
                            let sel_rects: Vec<_> = self
                                .selected_rects
                                .iter()
                                .filter(|(pi, _)| *pi == page_idx)
                                .map(|(_, r)| *r)
                                .collect();

                            let mut screen_rects = Vec::new();
                            for pr in &sel_rects {
                                if let Some(sr) = self.pdf_rect_to_screen_page(pr, page_idx) {
                                    screen_rects.push(sr);
                                }
                            }

                            let mut merged_rects: Vec<Rect> = Vec::new();
                            for sr in screen_rects {
                                if let Some(last) = merged_rects.last_mut() {
                                    let center_y_diff = (sr.center().y - last.center().y).abs();
                                    let height = sr.height().min(last.height());
                                    if center_y_diff < height * 0.5 {
                                        let gap = sr.min.x - last.max.x;
                                        if gap < height * 2.0 && gap > -height {
                                            *last = last.union(sr);
                                            continue;
                                        }
                                    }
                                }
                                merged_rects.push(sr);
                            }

                            for sr in merged_rects {
                                painter.rect_filled(
                                    sr,
                                    2.0,
                                    Color32::from_rgba_premultiplied(80, 140, 255, 110),
                                );
                            }
                        }
                    } else {
                        // Only draw placeholder for pages near the viewport edge
                        let near = page_rect.max.y > viewport_rect.min.y - viewport_rect.height()
                            && page_rect.min.y < viewport_rect.max.y + viewport_rect.height();
                        if near {
                            let painter = ui.painter_at(viewport_rect);
                            painter.rect_filled(page_rect, 4.0, Color32::from_gray(40));
                            painter.text(
                                page_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("{}", page_idx + 1),
                                egui::FontId::proportional(14.0),
                                Color32::from_gray(80),
                            );
                        }
                    }

                    // Per-page interaction — ui.interact does not advance the layout cursor
                    let response = ui.interact(
                        page_rect,
                        ui.id().with(("page", page_idx)),
                        Sense::click_and_drag(),
                    );

                    if response.hovered() {
                        let cursor = ctx
                            .input(|i| i.pointer.latest_pos())
                            .map(|pos| {
                                if self.is_pos_over_link(pos, page_idx) {
                                    CursorIcon::PointingHand
                                } else if self.is_pos_over_text(pos, page_idx) {
                                    CursorIcon::Text
                                } else {
                                    CursorIcon::Default
                                }
                            })
                            .unwrap_or(CursorIcon::Default);
                        ctx.set_cursor_icon(cursor);
                    }
                    if response.clicked() {
                        let link_target = ctx
                            .input(|i| i.pointer.interact_pos())
                            .and_then(|pos| self.get_link_target_page(pos, page_idx));
                        if let Some(target_page) = link_target {
                            self.target_scroll_page = Some(target_page);
                        } else {
                            self.clear_selection();
                        }
                        ctx.request_repaint();
                    }
                    if response.double_clicked() {
                        let pos = ctx
                            .input(|i| i.pointer.interact_pos())
                            .unwrap_or(Pos2::ZERO);
                        self.select_word_at(pos, page_idx);
                        ctx.request_repaint();
                    }
                    if response.triple_clicked() {
                        let pos = ctx
                            .input(|i| i.pointer.interact_pos())
                            .unwrap_or(Pos2::ZERO);
                        self.select_line_at(pos, page_idx);
                        ctx.request_repaint();
                    }
                    if response.drag_started() {
                        self.clear_selection();
                        if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                            if let Some((px, py)) = self.screen_to_pdf_page(pos, page_idx) {
                                self.drag_start = Some((page_idx, Pos2::new(px, py)));
                                self.drag_end = self.drag_start;
                            }
                        }
                    }
                    if response.dragged() {
                        if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                            let target_page = self
                                .page_at_pos(pos)
                                .or_else(|| self.nearest_page_to_pos(pos));

                            if let Some(curr_page) = target_page {
                                if let Some((px, py)) = self.screen_to_pdf_page(pos, curr_page) {
                                    self.drag_end = Some((curr_page, Pos2::new(px, py)));
                                    self.update_selection();
                                    ctx.request_repaint();
                                }
                            }
                        }
                    }
                }

                self.current_page = best_page;

                // Autosave bookmark every 2 seconds
                let now = ctx.input(|i| i.time);
                if (now - self.last_save_time) > 2.0 {
                    self.save_bookmark();
                    self.last_save_time = now;
                }

                // Auto-scroll when dragging near edges
                if self.drag_start.is_some() && ctx.input(|i| i.pointer.primary_down()) {
                    if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                        let scroll_zone = 60.0_f32;
                        let scroll_speed = 1200.0_f64;
                        let dt = ctx.input(|i| i.predicted_dt) as f64;
                        let mut auto_scrolled = false;

                        if pos.y > viewport_rect.bottom() - scroll_zone {
                            let intensity =
                                ((pos.y - (viewport_rect.bottom() - scroll_zone)) / scroll_zone)
                                    as f64;
                            self.offset = (self.offset
                                + scroll_speed * intensity.clamp(0.0, 2.0) * dt)
                                .clamp(0.0, max_offset);
                            auto_scrolled = true;
                        } else if pos.y < viewport_rect.top() + scroll_zone {
                            let intensity =
                                (((viewport_rect.top() + scroll_zone) - pos.y) / scroll_zone)
                                    as f64;
                            self.offset = (self.offset
                                - scroll_speed * intensity.clamp(0.0, 2.0) * dt)
                                .max(0.0);
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