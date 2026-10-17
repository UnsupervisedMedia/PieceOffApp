//! The playback bar shown under a video puzzle: a scrubbable timeline with
//! loop markers, transport buttons, and local audio and quality controls.

use super::*;
use eframe::egui::RichText;

/// What a drag on the timeline is moving.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TimelineDrag {
    Playhead,
    LoopStart,
    LoopEnd,
}

#[derive(Clone, Copy)]
pub(crate) enum SeekStep {
    LoopStart,
    LoopEnd,
    Back,
    Forward,
}

const SPEEDS: [f32; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Shortest loop the timeline handles can make.
const MIN_LOOP_MS: u64 = 500;
const SEEK_STEP_MS: i64 = 5_000;
const HANDLE_GRAB_RADIUS: f32 = 9.0;

/// Which part of the timeline a press at `x` grabs: a loop handle when the
/// press is on one and no closer to the playhead, otherwise the playhead.
pub(crate) fn timeline_drag_target(x: f32, start_x: f32, end_x: f32, head_x: f32) -> TimelineDrag {
    let to_start = (x - start_x).abs();
    let to_end = (x - end_x).abs();
    let to_head = (x - head_x).abs();
    let (handle, distance) = if to_start <= to_end {
        (TimelineDrag::LoopStart, to_start)
    } else {
        (TimelineDrag::LoopEnd, to_end)
    };
    if distance <= HANDLE_GRAB_RADIUS && distance < to_head {
        handle
    } else {
        TimelineDrag::Playhead
    }
}

fn icon_button(ui: &mut egui::Ui, enabled: bool, glyph: &str, hint: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(glyph).size(16.0)).min_size(Vec2::new(34.0, 30.0)),
    )
    .on_hover_text(hint)
    .clicked()
}

impl PieceOffApp {
    /// Host: jump within the active loop.
    pub(crate) fn seek_step(&mut self, step: SeekStep) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let playback = snapshot.playback.clone();
        let position = if playback.playing {
            estimated_position_ms(&playback)
        } else {
            self.video_position_ms
        };
        let loop_start = playback.loop_start_ms;
        let loop_end = if playback.loop_end_ms == 0 {
            self.media_duration_ms
        } else {
            playback.loop_end_ms.min(self.media_duration_ms)
        };
        let next = match step {
            SeekStep::LoopStart => loop_start,
            SeekStep::LoopEnd => loop_end.saturating_sub(1).max(loop_start),
            SeekStep::Back => seek_loop_by(position, -SEEK_STEP_MS, loop_start, loop_end),
            SeekStep::Forward => seek_loop_by(position, SEEK_STEP_MS, loop_start, loop_end),
        };
        self.publish_playback(playback.playing, next);
    }

    /// The scrub bar: the whole clip, with the loop range highlighted and its
    /// in and out points draggable by the host.
    fn timeline_bar(&mut self, ui: &mut egui::Ui, is_host: bool, is_playing: bool) {
        let duration = self.media_duration_ms.max(1);
        let sense = if is_host {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), sense);
        let center_y = rect.center().y;
        let track = Rect::from_min_max(
            Pos2::new(rect.left() + 10.0, center_y - 3.0),
            Pos2::new(rect.right() - 10.0, center_y + 3.0),
        );
        let to_x =
            |ms: u64| track.left() + track.width() * (ms.min(duration) as f32 / duration as f32);
        let to_ms = |x: f32| {
            (f64::from(((x - track.left()) / track.width()).clamp(0.0, 1.0)) * duration as f64)
                as u64
        };

        if is_host {
            if response.drag_started() {
                let press_x = ui
                    .input(|input| input.pointer.press_origin())
                    .map_or(rect.center().x, |pos| pos.x);
                let target = timeline_drag_target(
                    press_x,
                    to_x(self.loop_start_ms),
                    to_x(self.loop_end_ms),
                    to_x(self.video_position_ms),
                );
                self.timeline_drag = Some(target);
                self.seeking = target == TimelineDrag::Playhead;
            }
            if let (Some(target), Some(pointer)) =
                (self.timeline_drag, response.interact_pointer_pos())
            {
                let at = to_ms(pointer.x);
                match target {
                    TimelineDrag::Playhead => {
                        self.video_position_ms = at.clamp(
                            self.loop_start_ms,
                            self.loop_end_ms.saturating_sub(1).max(self.loop_start_ms),
                        );
                    }
                    TimelineDrag::LoopStart => {
                        self.loop_start_ms = at.min(self.loop_end_ms.saturating_sub(MIN_LOOP_MS));
                    }
                    TimelineDrag::LoopEnd => {
                        self.loop_end_ms = at
                            .max(self.loop_start_ms.saturating_add(MIN_LOOP_MS))
                            .min(duration);
                    }
                }
            }
            if response.drag_stopped() && self.timeline_drag.take().is_some() {
                self.seeking = false;
                self.publish_playback(is_playing, self.video_position_ms);
            } else if response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                self.publish_playback(is_playing, to_ms(pointer.x));
            }
        }

        let painter = ui.painter_at(rect);
        let loop_start_x = to_x(self.loop_start_ms);
        let loop_end_x = to_x(self.loop_end_ms);
        let head_x = to_x(self.video_position_ms);
        painter.rect_filled(track, 3.0, Color32::from_white_alpha(22));
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(loop_start_x, track.top()),
                Pos2::new(loop_end_x, track.bottom()),
            ),
            3.0,
            Color32::from_white_alpha(48),
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(loop_start_x, track.top()),
                Pos2::new(head_x.max(loop_start_x), track.bottom()),
            ),
            3.0,
            ACCENT,
        );
        let handle_color = if is_host {
            Color32::from_gray(225)
        } else {
            Color32::from_gray(130)
        };
        for (x, dragged) in [
            (loop_start_x, TimelineDrag::LoopStart),
            (loop_end_x, TimelineDrag::LoopEnd),
        ] {
            let active = self.timeline_drag == Some(dragged);
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(x, center_y),
                    Vec2::new(if active { 5.0 } else { 3.0 }, 18.0),
                ),
                1.5,
                if active { ACCENT } else { handle_color },
            );
        }
        let hovered = response.hovered() || self.timeline_drag.is_some();
        painter.circle_filled(
            Pos2::new(head_x, center_y),
            if hovered { 7.5 } else { 6.0 },
            Color32::WHITE,
        );
        if let Some(hover) = response.hover_pos() {
            painter.vline(
                hover.x.clamp(track.left(), track.right()),
                rect.y_range(),
                Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
            );
            let hint = match timeline_drag_target(hover.x, loop_start_x, loop_end_x, head_x) {
                _ if !is_host => clip::format_clock(to_ms(hover.x)),
                TimelineDrag::LoopStart => "Drag to set where the loop starts".to_owned(),
                TimelineDrag::LoopEnd => "Drag to set where the loop ends".to_owned(),
                TimelineDrag::Playhead => clip::format_clock(to_ms(hover.x)),
            };
            response.on_hover_text_at_pointer(hint);
        }
    }

    pub(crate) fn render_transport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        is_host: bool,
    ) {
        let is_playing = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.playback.playing);
        let duration = self.media_duration_ms.max(1);
        self.loop_start_ms = self.loop_start_ms.min(duration.saturating_sub(1));
        self.loop_end_ms = self
            .loop_end_ms
            .clamp(self.loop_start_ms.saturating_add(1), duration);
        ui.add_space(2.0);
        self.timeline_bar(ui, is_host, is_playing);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if icon_button(ui, is_host, "⏮", "Jump to the loop start (Home)") {
                self.seek_step(SeekStep::LoopStart);
            }
            if icon_button(ui, is_host, "⏪", "Back 5 seconds (Left)") {
                self.seek_step(SeekStep::Back);
            }
            let play = ui
                .add_enabled(
                    is_host,
                    egui::Button::new(
                        RichText::new(if is_playing { "⏸" } else { "▶" })
                            .size(20.0)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT_FILL)
                    .corner_radius(18)
                    .min_size(Vec2::new(46.0, 36.0)),
                )
                .on_hover_text(if is_playing {
                    "Pause for everyone (Space)"
                } else {
                    "Play for everyone (Space)"
                });
            if play.clicked() {
                self.set_playing(!is_playing);
            }
            if icon_button(ui, is_host, "⏩", "Forward 5 seconds (Right)") {
                self.seek_step(SeekStep::Forward);
            }
            if icon_button(ui, is_host, "⏭", "Jump to the loop end (End)") {
                self.seek_step(SeekStep::LoopEnd);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    clip::format_clock(self.video_position_ms),
                    clip::format_clock(duration)
                ))
                .monospace(),
            );
            let whole_clip = self.loop_start_ms == 0 && self.loop_end_ms >= duration;
            if !whole_clip {
                ui.add_space(8.0);
                ui.weak(format!(
                    "Loop {} – {}",
                    clip::format_clock(self.loop_start_ms),
                    clip::format_clock(self.loop_end_ms)
                ));
                if is_host
                    && ui
                        .small_button("Whole clip")
                        .on_hover_text("Loop the entire clip again")
                        .clicked()
                {
                    self.loop_start_ms = 0;
                    self.loop_end_ms = duration;
                    self.publish_playback(is_playing, self.video_position_ms);
                }
            }
            if !is_host {
                ui.add_space(8.0);
                ui.weak("The host controls playback");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if self.last_video_frame.is_some() {
                    let frozen = self.still_assist_texture.is_some();
                    if ui
                        .selectable_label(frozen, "Freeze")
                        .on_hover_text(
                            "Hold the current frame on your pieces; playback carries on",
                        )
                        .clicked()
                    {
                        self.toggle_still_assist(ctx);
                    }
                }
                let mut quality = None;
                ui.menu_button(self.settings.video_quality.short_label(), |ui| {
                    ui.weak("Video quality on this device");
                    for option in VideoQuality::ALL {
                        if ui
                            .selectable_label(self.settings.video_quality == option, option.label())
                            .clicked()
                        {
                            quality = Some(option);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Video quality");
                if let Some(quality) = quality {
                    self.set_video_quality(quality, ctx);
                }
                let speed_label = format!("{:.2}×", self.playback_rate);
                if is_host {
                    let mut speed = None;
                    ui.menu_button(speed_label, |ui| {
                        ui.weak("Playback speed for everyone");
                        for option in SPEEDS {
                            if ui
                                .selectable_label(
                                    (self.playback_rate - option).abs() < 0.01,
                                    format!("{option:.2}×"),
                                )
                                .clicked()
                            {
                                speed = Some(option);
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Playback speed");
                    if let Some(speed) = speed {
                        self.playback_rate = speed;
                        self.publish_playback(is_playing, self.video_position_ms);
                    }
                } else {
                    ui.weak(speed_label);
                }
                ui.separator();
                if self.media_has_audio && self.audio_control.is_some() {
                    ui.menu_button("Sync", |ui| {
                        ui.weak("Audio delay on this device");
                        let mut offset = self.audio_offset_ms.load(Ordering::Relaxed);
                        let slider = ui.add(
                            egui::Slider::new(&mut offset, -500..=500)
                                .suffix(" ms")
                                .step_by(10.0),
                        );
                        if slider.changed() {
                            self.audio_offset_ms.store(offset, Ordering::Relaxed);
                            self.settings.audio_offset_ms = offset;
                        }
                        if slider.drag_stopped() || slider.lost_focus() {
                            self.persist_settings();
                        }
                        if ui.small_button("Reset").clicked() {
                            self.audio_offset_ms.store(0, Ordering::Relaxed);
                            self.settings.audio_offset_ms = 0;
                            self.persist_settings();
                        }
                        ui.weak("Raise it if sound arrives before the picture,\nfor example on Bluetooth speakers.");
                    })
                    .response
                    .on_hover_text("Line the sound up with the picture");
                    ui.spacing_mut().slider_width = 90.0;
                    let volume =
                        ui.add(egui::Slider::new(&mut self.audio_volume, 0..=100).show_value(false));
                    if volume.changed() {
                        self.audio_muted = false;
                        self.sync_audio_levels();
                    }
                    if volume.drag_stopped() {
                        self.persist_audio_levels();
                    }
                    let silent = self.audio_muted || self.audio_volume == 0;
                    if ui
                        .button(RichText::new(if silent { "🔇" } else { "🔊" }).size(16.0))
                        .on_hover_text(if self.audio_muted { "Unmute" } else { "Mute" })
                        .clicked()
                    {
                        self.audio_muted = !self.audio_muted;
                        self.sync_audio_levels();
                        self.persist_audio_levels();
                    }
                } else if self.media_has_audio {
                    ui.weak("Audio off");
                } else {
                    ui.weak("No audio");
                }
            });
        });
        ui.add_space(2.0);
    }
}
