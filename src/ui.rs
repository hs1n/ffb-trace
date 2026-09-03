use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Stroke, Vec2, WindowLevel};
use egui_plot::{HLine, Line, LineStyle, MarkerShape, Plot, PlotPoints, Points, Text, VLine};
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::spectrum::analyze_spectrum;
use crate::tracker::{FfbTracker, ForceSample};

pub struct FfbTraceApp {
    tracker: Arc<RwLock<FfbTracker>>,
    source_description: Arc<RwLock<String>>,
    paused: bool,
    paused_history: Option<VecDeque<ForceSample>>,
    time_window_s: f64,
    is_mini: bool,
    reveal_serial: bool,
}

impl FfbTraceApp {
    pub fn new(
        tracker: Arc<RwLock<FfbTracker>>,
        source_description: Arc<RwLock<String>>,
        is_mini: bool,
    ) -> Self {
        Self {
            tracker,
            source_description,
            paused: false,
            paused_history: None,
            time_window_s: 6.0,
            is_mini,
            reveal_serial: false,
        }
    }
}

impl eframe::App for FfbTraceApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();

        // ------------------------------------------------------------------
        // Driver Rig Keyboard Shortcuts (High-speed hands-on operation)
        // ------------------------------------------------------------------
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.paused = !self.paused;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.tracker.write().reset_session();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::M)) {
            self.is_mini = !self.is_mini;
            if self.is_mini {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(440.0, 96.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(880.0, 680.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(WindowLevel::Normal));
            }
        }

        let (tracker_snapshot, tuning_rec) = {
            let t = self.tracker.read();
            let history_snapshot = if !self.paused {
                self.paused_history = None;
                t.history.clone()
            } else {
                if self.paused_history.is_none() {
                    self.paused_history = Some(t.history.clone());
                }
                self.paused_history.clone().unwrap_or_default()
            };
            (
                (
                    t.device_name.clone(),
                    t.current_level,
                    t.current_level_pct,
                    t.is_clip_latched,
                    t.peak_level_pct,
                    t.effective_hz,
                    t.constant_count,
                    t.clip_count,
                    t.total_clip_percentage(),
                    t.rolling_clip_percentage(),
                    t.current_gain,
                    t.histogram_bins,
                    history_snapshot,
                ),
                t.tuning_recommendation(),
            )
        };

        let (
            device_name,
            current_level,
            level_pct,
            is_clip_latched,
            peak_pct,
            effective_hz,
            constant_count,
            clip_count,
            clip_pct,
            rolling_clip_pct,
            _current_gain,
            histogram_bins,
            history,
        ) = tracker_snapshot;

        let (rec_tag, rec_primary, rec_sub) = tuning_rec;
        let source_desc = self.source_description.read().clone();

        // --- sms-telemetry Unified Palette & Contrast Tokens ---
        let paper_bg = Color32::from_rgb(16, 17, 21); // #101115
        let panel_bg = Color32::from_rgb(22, 23, 26); // #16171a
        let sunken_bg = Color32::from_rgb(11, 12, 14); // #0b0c0e
        let line_border = Color32::from_rgb(44, 46, 54); // #2c2e36 subtle divider
        let ref_blue = Color32::from_rgb(78, 161, 255); // #4ea1ff reference trace
        let compared_orange = Color32::from_rgb(255, 159, 67); // #ff9f43 peak hold / caution
        let loss_red = Color32::from_rgb(255, 95, 87); // #ff5f57 clipping alert
        let gain_green = Color32::from_rgb(43, 212, 168); // #2bd4a8 nominal green
        let ink = Color32::from_rgb(235, 236, 240); // #ebecf0 high-contrast primary
        let ink_dim = Color32::from_rgb(168, 174, 186); // #a8aeba secondary label
        let ink_faint = Color32::from_rgb(115, 122, 134); // #737a86 captions & units

        let mut style = (*ctx.style()).clone();
        style.visuals.panel_fill = paper_bg;
        style.visuals.window_fill = panel_bg;
        style.visuals.widgets.noninteractive.bg_fill = panel_bg;
        ctx.set_style(style);

        let display_dev = if self.reveal_serial {
            if device_name.is_empty() {
                "Waiting for FFB device...".to_string()
            } else {
                device_name.clone()
            }
        } else {
            if device_name.is_empty() {
                "Waiting for FFB device...".to_string()
            } else {
                crate::runner::mask_serial(&device_name)
            }
        };

        // ==================================================================
        // MINI-HUD MODE (STREAMLINED ALWAYS-ON-TOP STRIP)
        // ==================================================================
        if self.is_mini {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("ffb-trace").size(13.0).color(ink).strong());

                    let (status_text, status_color) = if is_clip_latched {
                        ("CLIP", loss_red)
                    } else {
                        ("OK", gain_green)
                    };
                    ui.label(
                        RichText::new(status_text)
                            .size(11.0)
                            .color(status_color)
                            .strong()
                            .monospace(),
                    );

                    let force_color = if is_clip_latched { loss_red } else { ink };
                    ui.label(
                        RichText::new(format!("{:>+5.1}%", level_pct))
                            .size(15.0)
                            .color(force_color)
                            .monospace()
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!("pk:{:>4.1}%", peak_pct))
                            .size(11.0)
                            .color(compared_orange)
                            .monospace(),
                    );
                    ui.label(
                        RichText::new(format!("{:.0}Hz", effective_hz))
                            .size(11.0)
                            .color(ref_blue)
                            .monospace(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(RichText::new("Full [M]").size(10.0).color(ink_dim))
                            .clicked()
                        {
                            self.is_mini = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                                780.0, 540.0,
                            )));
                            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                                WindowLevel::Normal,
                            ));
                        }
                        if ui
                            .button(RichText::new("Reset [R]").size(10.0).color(ink_dim))
                            .clicked()
                        {
                            self.tracker.write().reset_session();
                        }
                    });
                });

                render_force_bar(
                    ui,
                    level_pct,
                    peak_pct,
                    is_clip_latched,
                    22.0,
                    sunken_bg,
                    line_border,
                    ref_blue,
                    loss_red,
                    compared_orange,
                );
            });
            return;
        }

        // ==================================================================
        // FULL DASHBOARD MODE (PRECISION MOTORSPORT TELEMETRY)
        // ==================================================================
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

            // --------------------------------------------------------------
            // 1. INSTRUMENT TOP BAR
            // --------------------------------------------------------------
            egui::Frame::NONE
                .fill(panel_bg)
                .stroke(Stroke::new(1.0_f32, line_border))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Vec2::new(10.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("ffb-trace").size(15.0).color(ink).strong());
                        ui.separator();

                        // Clickable device name with mask/reveal tooltip
                        let dev_resp = ui.add(
                            egui::Label::new(RichText::new(&display_dev).size(12.0).color(ink_dim))
                                .sense(egui::Sense::click()),
                        );
                        if dev_resp.clicked() {
                            self.reveal_serial = !self.reveal_serial;
                        }
                        if dev_resp.hovered() {
                            dev_resp.on_hover_text(if self.reveal_serial {
                                "Click to mask device serial"
                            } else {
                                "Click to reveal device serial"
                            });
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Mini Mode toggle
                            if ui
                                .button(RichText::new("Mini [M]").size(11.0).color(ink_dim))
                                .clicked()
                            {
                                self.is_mini = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                                    440.0, 96.0,
                                )));
                                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                                    WindowLevel::AlwaysOnTop,
                                ));
                            }

                            // Reset Session
                            if ui
                                .button(RichText::new("Reset [R]").size(11.0).color(ink_dim))
                                 .clicked()
                            {
                                self.tracker.write().reset_session();
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&source_desc)
                                .size(10.5)
                                .color(ink_faint)
                                .monospace(),
                        );
                    });
                });

            // --------------------------------------------------------------
            // 2. PRIMARY FORCE METER & DIGITAL READOUT
            // --------------------------------------------------------------
            egui::Frame::NONE
                .fill(panel_bg)
                .stroke(Stroke::new(1.0_f32, line_border))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Vec2::new(12.0, 10.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (status_text, status_color) = if is_clip_latched {
                            ("CLIPPING", loss_red)
                        } else if rolling_clip_pct > 0.5 {
                            ("HIGH LOAD", compared_orange)
                        } else {
                            ("NOMINAL", gain_green)
                        };

                        ui.label(
                            RichText::new(status_text)
                                .size(11.5)
                                .color(status_color)
                                .strong()
                                .monospace(),
                        );

                        ui.add_space(8.0);

                        let dir_text = if level_pct > 0.5 {
                            "R"
                        } else if level_pct < -0.5 {
                            "L"
                        } else {
                            "C"
                        };

                        ui.label(
                            RichText::new(dir_text)
                                .size(13.0)
                                .color(ink_faint)
                                .monospace(),
                        );

                        let force_color = if is_clip_latched { loss_red } else { ink };

                        ui.label(
                            RichText::new(format!("{:>+5.1}%", level_pct))
                                .size(20.0)
                                .color(force_color)
                                .monospace()
                                .strong(),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("Peak: {:>4.1}%", peak_pct))
                                    .size(12.0)
                                    .color(compared_orange)
                                    .monospace(),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!("Raw: {:>+6} / 32767", current_level))
                                    .size(11.0)
                                    .color(ink_faint)
                                    .monospace(),
                            );
                        });
                    });

                    ui.add_space(6.0);

                    // High-contrast 26px master force bar
                    render_force_bar(
                        ui,
                        level_pct,
                        peak_pct,
                        is_clip_latched,
                        26.0,
                        sunken_bg,
                        line_border,
                        ref_blue,
                        loss_red,
                        compared_orange,
                    );
                });

            // --------------------------------------------------------------
            // 3. STATS TELEMETRY GRID (4 EQUAL CARDS)
            // --------------------------------------------------------------
            ui.columns(4, |cols| {
                // Card 1: Session Clipping
                let clip_color = if clip_pct > 1.0 {
                    loss_red
                } else if clip_pct > 0.0 {
                    compared_orange
                } else {
                    gain_green
                };
                render_stat_card(
                    &mut cols[0],
                    "SESSION CLIPPING",
                    &format!("{:.3} %", clip_pct),
                    clip_color,
                    &format!("{} / {} ticks", clip_count, constant_count),
                    ink_faint,
                    panel_bg,
                    line_border,
                );

                // Card 2: Rolling 5s Window
                let rolling_color = if rolling_clip_pct > 0.5 {
                    loss_red
                } else if rolling_clip_pct > 0.0 {
                    compared_orange
                } else {
                    ink
                };
                let hint = if rolling_clip_pct > 0.0 {
                    "clipping in corner"
                } else {
                    "headroom nominal"
                };
                render_stat_card(
                    &mut cols[1],
                    "ACTIVE WINDOW (5s)",
                    &format!("{:.1} %", rolling_clip_pct),
                    rolling_color,
                    hint,
                    ink_faint,
                    panel_bg,
                    line_border,
                );

                // Card 3: Update Rate
                let interval_ms = if effective_hz > 0.0 {
                    1000.0 / effective_hz
                } else {
                    0.0
                };
                render_stat_card(
                    &mut cols[2],
                    "UPDATE RATE",
                    &format!("{:.0} Hz", effective_hz),
                    ref_blue,
                    &format!("{:.2} ms / tick", interval_ms),
                    ink_faint,
                    panel_bg,
                    line_border,
                );

                // Card 4: FFB Tuning Suggestion
                let rec_color = if rec_tag == "CLIPPING HIGH" {
                    loss_red
                } else if rec_tag == "HEADROOM LOW" {
                    compared_orange
                } else if rec_tag == "OPTIMAL" {
                    gain_green
                } else {
                    ink
                };
                render_stat_card(
                    &mut cols[3],
                    "TUNING ADVICE",
                    &rec_primary,
                    rec_color,
                    &rec_sub,
                    ink_faint,
                    panel_bg,
                    line_border,
                );
            });

            // --------------------------------------------------------------
            // 4. MAIN TELEMETRY CARDS (WAVEFORM / HISTOGRAM / SPECTRUM)
            // --------------------------------------------------------------
            let available_h = ui.available_height();
            let top_card_h = (available_h * 0.49).clamp(160.0, 360.0);
            let bottom_card_h = (available_h - top_card_h - 8.0).max(140.0);

            // Card 1: Waveform Card
            egui::Frame::NONE
                .fill(panel_bg)
                .stroke(Stroke::new(1.0_f32, line_border))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Vec2::new(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(top_card_h - 16.0);
                    render_waveform_view(
                        ui,
                        &history,
                        &mut self.paused,
                        &mut self.time_window_s,
                        top_card_h - 16.0,
                        ref_blue,
                        loss_red,
                        line_border,
                        compared_orange,
                        ink_dim,
                        ink_faint,
                    );
                });

            ui.add_space(8.0);

            // Card 2 (Distribution) & Card 3 (Spectrum)
            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    egui::Frame::NONE
                        .fill(panel_bg)
                        .stroke(Stroke::new(1.0_f32, line_border))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Vec2::new(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(bottom_card_h - 16.0);
                            render_histogram_view(
                                ui,
                                &histogram_bins,
                                constant_count,
                                bottom_card_h - 16.0,
                                sunken_bg,
                                line_border,
                                ref_blue,
                                loss_red,
                                gain_green,
                                ink_dim,
                                ink_faint,
                            );
                        });
                });

                cols[1].vertical(|ui| {
                    egui::Frame::NONE
                        .fill(panel_bg)
                        .stroke(Stroke::new(1.0_f32, line_border))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Vec2::new(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(bottom_card_h - 16.0);
                            render_spectrum_view(
                                ui,
                                &history,
                                bottom_card_h - 16.0,
                                line_border,
                                ref_blue,
                                gain_green,
                                compared_orange,
                                loss_red,
                                ink_dim,
                                ink_faint,
                            );
                        });
                });
            });
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_stat_card(
    ui: &mut egui::Ui,
    title: &str,
    primary_text: &str,
    primary_color: Color32,
    sub_text: &str,
    sub_color: Color32,
    bg: Color32,
    border: Color32,
) {
    egui::Frame::NONE
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Vec2::new(10.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(54.0);
            ui.label(
                RichText::new(title)
                    .size(9.5)
                    .color(Color32::from_rgb(120, 126, 138))
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(primary_text)
                    .size(15.0)
                    .color(primary_color)
                    .monospace()
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(sub_text)
                    .size(10.0)
                    .color(sub_color)
                    .monospace(),
            );
        });
}

#[allow(clippy::too_many_arguments)]
fn render_force_bar(
    ui: &mut egui::Ui,
    level_pct: f32,
    peak_pct: f32,
    is_clipped: bool,
    height: f32,
    bg: Color32,
    border: Color32,
    fill: Color32,
    clip_fill: Color32,
    peak_color: Color32,
) {
    let desired_size = Vec2::new(ui.available_width(), height);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    let painter = ui.painter();

    // Background track
    painter.rect_filled(rect, CornerRadius::same(3), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Outside,
    );

    let center_x = rect.center().x;
    let half_width = rect.width() / 2.0 - 2.0;

    // Subtle graduation ticks at -75%, -50%, -25%, 0, +25%, +50%, +75%
    let fractions: [f32; 7] = [-0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75];
    for fraction in fractions {
        let tick_x = center_x + fraction * half_width;
        let is_center = fraction == 0.0;
        let stroke = if is_center {
            Stroke::new(1.5_f32, border)
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(32, 34, 40))
        };
        painter.line_segment(
            [
                Pos2::new(tick_x, rect.top()),
                Pos2::new(tick_x, rect.bottom()),
            ],
            stroke,
        );
    }

    // Active Force Bar fill
    let fill_width = (level_pct.abs() / 100.0) * half_width;
    let active_color = if is_clipped { clip_fill } else { fill };

    if fill_width > 0.5 {
        let (bar_left, bar_right) = if level_pct >= 0.0 {
            (center_x, center_x + fill_width)
        } else {
            (center_x - fill_width, center_x)
        };

        let bar_rect = Rect::from_min_max(
            Pos2::new(bar_left, rect.top() + 2.0),
            Pos2::new(bar_right, rect.bottom() - 2.0),
        );
        painter.rect_filled(bar_rect, CornerRadius::same(1), active_color);
    }

    // Peak hold indicator line
    let peak_offset = (peak_pct / 100.0) * half_width;
    let peak_stroke = Stroke::new(1.5_f32, peak_color);

    if peak_offset > 1.0 {
        painter.line_segment(
            [
                Pos2::new(center_x + peak_offset, rect.top() + 1.0),
                Pos2::new(center_x + peak_offset, rect.bottom() - 1.0),
            ],
            peak_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(center_x - peak_offset, rect.top() + 1.0),
                Pos2::new(center_x - peak_offset, rect.bottom() - 1.0),
            ],
            peak_stroke,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_waveform_view(
    ui: &mut egui::Ui,
    history: &VecDeque<ForceSample>,
    paused: &mut bool,
    time_window_s: &mut f64,
    height: f32,
    ref_blue: Color32,
    loss_red: Color32,
    line_border: Color32,
    compared_orange: Color32,
    ink_dim: Color32,
    ink_faint: Color32,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("FORCE WAVEFORM")
                .size(9.5)
                .color(ink_dim)
                .strong(),
        );
        ui.label(
            RichText::new("(-100% to +100%)")
                .size(9.5)
                .color(ink_faint)
                .monospace(),
        );

        if *paused {
            ui.label(
                RichText::new("PAUSED [Space]")
                    .size(9.5)
                    .color(compared_orange)
                    .strong(),
            );
        }

        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let pause_text = if *paused {
                    "Resume [Space]"
                } else {
                    "Pause [Space]"
                };
                if ui
                    .button(RichText::new(pause_text).size(10.5).color(ink_dim))
                    .clicked()
                {
                    *paused = !*paused;
                }

                ui.separator();

                ui.selectable_value(time_window_s, 10.0, "10s");
                ui.selectable_value(time_window_s, 6.0, "6s");
                ui.selectable_value(time_window_s, 3.0, "3s");
            },
        );
    });

    let plot_points: PlotPoints = history
        .iter()
        .map(|s| [s.time_s, s.level_pct as f64])
        .collect();

    let line = Line::new(plot_points).color(ref_blue).width(1.4_f32);

    let clip_points: PlotPoints = history
        .iter()
        .filter(|s| s.is_clipped || s.level_pct.abs() >= 99.5)
        .map(|s| [s.time_s, s.level_pct as f64])
        .collect();

    let clip_markers = Points::new(clip_points)
        .color(loss_red)
        .radius(2.5_f32)
        .shape(MarkerShape::Circle);

    let latest_time = history.back().map_or(0.0, |s| s.time_s);
    let x_min = (latest_time - *time_window_s).max(0.0);
    let x_max = latest_time.max(*time_window_s);

    let plot_h = (height - 24.0).max(80.0);
    Plot::new("ffb_waveform")
        .height(plot_h)
        .include_y(-108.0)
        .include_y(108.0)
        .include_x(x_min)
        .include_x(x_max)
        .show_axes([false, true])
        .show_grid([true, true])
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .show(ui, |plot_ui| {
            plot_ui.hline(
                HLine::new(100.0)
                    .color(loss_red)
                    .style(LineStyle::Dashed { length: 4.0 }),
            );
            plot_ui.hline(
                HLine::new(-100.0)
                    .color(loss_red)
                    .style(LineStyle::Dashed { length: 4.0 }),
            );
            plot_ui.hline(
                HLine::new(0.0).color(line_border).style(LineStyle::Solid),
            );

            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(
                    x_min + (x_max - x_min) * 0.02,
                    102.0,
                ),
                RichText::new("+100% Clip").size(9.0).color(loss_red),
            ));
            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(
                    x_min + (x_max - x_min) * 0.02,
                    -102.0,
                ),
                RichText::new("-100% Clip").size(9.0).color(loss_red),
            ));

            plot_ui.line(line);
            plot_ui.points(clip_markers);
        });
}

#[allow(clippy::too_many_arguments)]
fn render_histogram(
    ui: &mut egui::Ui,
    bins: &[u64; 21],
    total_count: u64,
    height: f32,
    bg: Color32,
    border: Color32,
    bar_fill: Color32,
    clip_fill: Color32,
    center_fill: Color32,
    text_color: Color32,
) {
    let desired_size = Vec2::new(ui.available_width(), height);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), bg);

    if total_count == 0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No FFB telemetry data",
            egui::FontId::proportional(12.0),
            text_color,
        );
        return;
    }

    let max_bin_val = bins.iter().copied().max().unwrap_or(1).max(1);
    let bar_count = bins.len();
    let padding = 4.0_f32;
    let available_w = rect.width() - (padding * 2.0);
    let bar_width = available_w / (bar_count as f32);
    let max_bar_h = rect.height() - 24.0;

    for (i, &val) in bins.iter().enumerate() {
        let x = rect.left() + padding + (i as f32 * bar_width);
        let fraction = val as f32 / max_bin_val as f32;
        let bar_h = (fraction * max_bar_h).max(1.0);
        let y = rect.bottom() - 18.0 - bar_h;

        let is_edge = i == 0 || i == bar_count - 1;
        let is_center = i == 10;
        let color = if is_edge {
            clip_fill
        } else if is_center {
            center_fill
        } else {
            bar_fill
        };

        let bar_rect = Rect::from_min_max(
            Pos2::new(x + 1.0, y),
            Pos2::new(x + bar_width - 1.0, rect.bottom() - 18.0),
        );
        painter.rect_filled(bar_rect, CornerRadius::ZERO, color);
    }

    // Baseline
    painter.line_segment(
        [
            Pos2::new(rect.left() + padding, rect.bottom() - 18.0),
            Pos2::new(rect.right() - padding, rect.bottom() - 18.0),
        ],
        Stroke::new(1.0_f32, border),
    );

    // Labels
    painter.text(
        Pos2::new(rect.left() + padding, rect.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        "-100% Clip",
        egui::FontId::monospace(9.0),
        clip_fill,
    );
    painter.text(
        Pos2::new(rect.center().x, rect.bottom() - 4.0),
        egui::Align2::CENTER_BOTTOM,
        "0% Center",
        egui::FontId::monospace(9.0),
        text_color,
    );
    painter.text(
        Pos2::new(rect.right() - padding, rect.bottom() - 4.0),
        egui::Align2::RIGHT_BOTTOM,
        "+100% Clip",
        egui::FontId::monospace(9.0),
        clip_fill,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_histogram_view(
    ui: &mut egui::Ui,
    bins: &[u64; 21],
    total_count: u64,
    height: f32,
    bg: Color32,
    border: Color32,
    bar_fill: Color32,
    clip_fill: Color32,
    center_fill: Color32,
    ink_dim: Color32,
    ink_faint: Color32,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("FORCE DISTRIBUTION")
                .size(9.5)
                .color(ink_dim)
                .strong(),
        );
        ui.label(
            RichText::new("21 Bins")
                .size(9.5)
                .color(ink_faint)
                .monospace(),
        );
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new(format!("{} updates", total_count))
                        .size(9.5)
                        .color(ink_faint)
                        .monospace(),
                );
            },
        );
    });

    let hist_h = (height - 22.0).max(60.0);
    render_histogram(
        ui,
        bins,
        total_count,
        hist_h,
        bg,
        border,
        bar_fill,
        clip_fill,
        center_fill,
        ink_faint,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_spectrum_view(
    ui: &mut egui::Ui,
    history: &VecDeque<ForceSample>,
    height: f32,
    line_border: Color32,
    ref_blue: Color32,
    gain_green: Color32,
    compared_orange: Color32,
    loss_red: Color32,
    ink_dim: Color32,
    ink_faint: Color32,
) {
    let analysis = analyze_spectrum(history);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("VIBRATION SPECTRUM")
                .size(9.5)
                .color(ink_dim)
                .strong(),
        );
        ui.label(
            RichText::new("FFT 0-100Hz")
                .size(9.5)
                .color(ink_faint)
                .monospace(),
        );

        if analysis.dominant_magnitude_pct >= 0.5 {
            let (band_desc, band_color) = if analysis.dominant_freq_hz < 4.0 {
                ("SAT", ref_blue)
            } else if analysis.dominant_freq_hz < 15.0 {
                ("Curbs", gain_green)
            } else if analysis.dominant_freq_hz < 40.0 {
                ("Scrub", compared_orange)
            } else {
                ("Engine", loss_red)
            };

            ui.label(
                RichText::new(format!(
                    "Peak {:.0}Hz ({:.0}%) • {}",
                    analysis.dominant_freq_hz, analysis.dominant_magnitude_pct, band_desc
                ))
                .size(9.5)
                .color(band_color)
                .strong(),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("Eng {:.0}%", analysis.bands.high_freq_pct))
                    .size(9.0)
                    .color(loss_red)
                    .monospace(),
            );
            ui.label(RichText::new("•").size(9.0).color(line_border));
            ui.label(
                RichText::new(format!("Scrub {:.0}%", analysis.bands.road_texture_pct))
                    .size(9.0)
                    .color(compared_orange)
                    .monospace(),
            );
            ui.label(RichText::new("•").size(9.0).color(line_border));
            ui.label(
                RichText::new(format!("Curb {:.0}%", analysis.bands.chassis_curbs_pct))
                    .size(9.0)
                    .color(gain_green)
                    .monospace(),
            );
            ui.label(RichText::new("•").size(9.0).color(line_border));
            ui.label(
                RichText::new(format!("SAT {:.0}%", analysis.bands.steering_pct))
                    .size(9.0)
                    .color(ref_blue)
                    .monospace(),
            );
        });
    });

    let points: PlotPoints = analysis
        .bins
        .iter()
        .map(|b| [b.freq_hz as f64, b.magnitude_pct as f64])
        .collect();

    let max_mag = analysis
        .bins
        .iter()
        .map(|b| b.magnitude_pct)
        .fold(0.0_f32, f32::max)
        .max(25.0);

    let plot_h = (height - 24.0).max(60.0);
    Plot::new("ffb_spectrum")
        .height(plot_h)
        .include_x(0.0)
        .include_x(100.0)
        .include_y(0.0)
        .include_y(max_mag as f64 * 1.15)
        .show_axes([true, true])
        .show_grid([true, true])
        .x_axis_label("Hz")
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .show(ui, |plot_ui| {
            plot_ui.vline(
                VLine::new(4.0)
                    .color(line_border)
                    .style(LineStyle::Dashed { length: 3.0 }),
            );
            plot_ui.vline(
                VLine::new(15.0)
                    .color(line_border)
                    .style(LineStyle::Dashed { length: 3.0 }),
            );
            plot_ui.vline(
                VLine::new(40.0)
                    .color(line_border)
                    .style(LineStyle::Dashed { length: 3.0 }),
            );

            let y_label_pos = max_mag as f64 * 1.05;
            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(2.0, y_label_pos),
                RichText::new("SAT").size(8.5).color(ref_blue),
            ));
            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(9.5, y_label_pos),
                RichText::new("Curbs").size(8.5).color(gain_green),
            ));
            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(27.5, y_label_pos),
                RichText::new("Scrub").size(8.5).color(compared_orange),
            ));
            plot_ui.text(Text::new(
                egui_plot::PlotPoint::new(70.0, y_label_pos),
                RichText::new("Engine").size(8.5).color(loss_red),
            ));

            let line = Line::new(points)
                .color(ref_blue)
                .fill(0.0_f32)
                .width(1.8_f32);
            plot_ui.line(line);

            if analysis.dominant_magnitude_pct >= 0.5 {
                let peak_pt = Points::new(vec![[
                    analysis.dominant_freq_hz as f64,
                    analysis.dominant_magnitude_pct as f64,
                ]])
                .color(loss_red)
                .radius(4.0_f32)
                .shape(MarkerShape::Circle);
                plot_ui.points(peak_pt);
            }
        });
}
