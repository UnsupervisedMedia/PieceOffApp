use super::*;

/// How long a stopped decoder waits before the app starts it again.
pub(crate) const DECODER_AUTO_RETRY_DELAY: Duration = Duration::from_secs(2);

impl PieceOffApp {
    pub(crate) fn audio_usable(&self) -> bool {
        self.audio_launcher.is_some() || !audio_disabled()
    }

    pub(crate) fn restart_failed_decoder(&mut self, ctx: &egui::Context) {
        let decoder_stopped = self
            .decoder_worker_alive
            .as_ref()
            .is_some_and(|alive| !alive.load(Ordering::Acquire));
        if !decoder_stopped
            || self.decoder_restarts_after_error >= MAX_DECODER_RECOVERY_ATTEMPTS
            || !self.is_video
        {
            return;
        }
        let Some(path) = self.media_path.clone() else {
            return;
        };
        self.decoder_restarts_after_error += 1;
        self.decoder_recovering = true;
        self.status = "Restarting the video decoder…".into();
        if self.media_extension == "webp" {
            self.start_animated_webp_decoder(path, ctx.clone());
        } else {
            let audio_enabled = self.media_has_audio && self.audio_usable();
            self.start_video_decoder_using(
                path,
                audio_enabled,
                ctx.clone(),
                self.decoder_ffmpeg_program.clone(),
            );
        }
    }

    /// A decoder that gave up after its own retries is tried again every few
    /// seconds, within the same budget as restarts triggered by playback
    /// updates (which a quiet room may not send for a long time). The budget
    /// refills as soon as a frame arrives.
    pub(crate) fn auto_recover_decoder(&mut self, ctx: &egui::Context) {
        let dead = self.snapshot.is_some()
            && self.is_video
            && self.media_ready
            && self
                .decoder_worker_alive
                .as_ref()
                .is_some_and(|alive| !alive.load(Ordering::Acquire));
        if !dead {
            self.decoder_dead_since = None;
            return;
        }
        let since = *self.decoder_dead_since.get_or_insert_with(Instant::now);
        if since.elapsed() < DECODER_AUTO_RETRY_DELAY {
            ctx.request_repaint_after(DECODER_AUTO_RETRY_DELAY - since.elapsed());
            return;
        }
        self.decoder_dead_since = Some(Instant::now());
        if self.decoder_restarts_after_error < MAX_DECODER_RECOVERY_ATTEMPTS {
            self.sync_decoder_playback(ctx);
        } else if !self.decoder_gave_up_reported {
            self.decoder_gave_up_reported = true;
            self.status = "The video stopped and could not be restarted. Use More → Restart video to try again.".into();
        }
    }

    /// Start the video decoder afresh even if it is running, with a full
    /// restart budget: the player's way out when automatic recovery gave up.
    pub(crate) fn restart_video_decoder(&mut self, ctx: &egui::Context) {
        let Some(path) = self.media_path.clone().filter(|_| self.is_video) else {
            return;
        };
        self.decoder_restarts_after_error = 0;
        self.decoder_gave_up_reported = false;
        self.decoder_dead_since = None;
        self.decoder_recovering = true;
        if self.media_extension == "webp" {
            self.start_animated_webp_decoder(path, ctx.clone());
        } else {
            let audio_enabled = self.media_has_audio && self.audio_usable();
            self.start_video_decoder_using(
                path,
                audio_enabled,
                ctx.clone(),
                self.decoder_ffmpeg_program.clone(),
            );
        }
        self.sync_decoder_playback(ctx);
        self.status = "Restarting the video…".into();
    }

    /// With repeat Off or All the server moves the room on at the end of the
    /// item. Until that snapshot arrives, hold the last frame instead of
    /// letting the decoder wrap to the start of the same clip.
    pub(crate) fn hold_at_item_end(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if snapshot.repeat == RepeatMode::One || self.holding_at_end || !snapshot.playback.playing {
            return;
        }
        let end = item_end_ms(&snapshot.playback, self.media_duration_ms);
        let now = crate::board::server_clock_ms();
        if end == 0 || unwrapped_position_at(&snapshot.playback, now) < end {
            return;
        }
        self.holding_at_end = true;
        let rate = snapshot.playback.rate;
        if let Some(playing) = &self.decoder_playing {
            playing.store(false, Ordering::Relaxed);
        }
        if let Some(control) = &self.decoder_control {
            let _ = control.send(DecoderCommand {
                playing: false,
                position_ms: end,
                rate,
                loop_start_ms: self.loop_start_ms,
                loop_end_ms: self.loop_end_ms,
                resync: false,
                issued_at: Instant::now(),
            });
        }
        if let Some(control) = &self.audio_control {
            let _ = control.send(AudioCommand {
                playing: false,
                position_ms: end,
                rate,
                loop_start_ms: snapshot.playback.loop_start_ms,
                loop_end_ms: snapshot.playback.loop_end_ms,
                volume: self.audio_volume,
                muted: self.audio_muted,
                resync: false,
                issued_at: Instant::now(),
            });
        }
    }

    pub(crate) fn sync_decoder_playback(&mut self, ctx: &egui::Context) {
        // Any new room state ends a hold.
        self.holding_at_end = false;
        if self.snapshot.is_some() {
            self.restart_failed_decoder(ctx);
        }
        if let Some(snapshot) = &self.snapshot {
            let state = snapshot.playback.clone();
            let position_ms = estimated_position_ms(&state);
            self.video_position_ms = position_ms;
            self.loop_start_ms = state
                .loop_start_ms
                .min(self.media_duration_ms.saturating_sub(1));
            self.loop_end_ms = if state.loop_end_ms == 0 {
                self.media_duration_ms
            } else {
                state.loop_end_ms.min(self.media_duration_ms)
            };
            self.playback_rate = state.rate;
            if let Some(decoder) = &self.decoder_playing {
                decoder.store(state.playing, Ordering::Relaxed);
            }
            if let Some(control) = &self.decoder_control {
                let _ = control.send(DecoderCommand {
                    playing: state.playing,
                    position_ms,
                    rate: state.rate,
                    loop_start_ms: self.loop_start_ms,
                    loop_end_ms: self.loop_end_ms,
                    resync: false,
                    issued_at: Instant::now(),
                });
            }
        }
        self.sync_decoder_audio();
    }

    pub(crate) fn sync_decoder_audio(&self) {
        let (Some(snapshot), Some(control)) = (&self.snapshot, &self.audio_control) else {
            return;
        };
        let state = &snapshot.playback;
        let _ = control.send(AudioCommand {
            playing: state.playing,
            position_ms: estimated_position_ms(state),
            rate: state.rate,
            loop_start_ms: state.loop_start_ms,
            loop_end_ms: state.loop_end_ms,
            volume: self.audio_volume,
            muted: self.audio_muted,
            resync: false,
            issued_at: Instant::now(),
        });
    }

    /// Apply a volume or mute change without restarting the audio.
    pub(crate) fn sync_audio_levels(&mut self) {
        if let (Some(snapshot), Some(control)) = (&self.snapshot, &self.audio_control) {
            let state = &snapshot.playback;
            let _ = control.send(AudioCommand {
                playing: state.playing,
                position_ms: estimated_position_ms(state),
                rate: state.rate,
                loop_start_ms: state.loop_start_ms,
                loop_end_ms: state.loop_end_ms,
                volume: self.audio_volume,
                muted: self.audio_muted,
                resync: true,
                issued_at: Instant::now(),
            });
        }
    }

    pub(crate) fn persist_audio_levels(&mut self) {
        self.settings.audio_volume = Some(self.audio_volume);
        self.settings.audio_muted = self.audio_muted;
        self.persist_settings();
    }

    pub(crate) fn sync_decoder_drift(&self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let state = &snapshot.playback;
        let position_ms = estimated_position_ms(state);
        if let Some(control) = &self.decoder_control {
            let _ = control.send(DecoderCommand {
                playing: state.playing,
                position_ms,
                rate: state.rate,
                loop_start_ms: state.loop_start_ms,
                loop_end_ms: state.loop_end_ms,
                resync: true,
                issued_at: Instant::now(),
            });
        }
        if let Some(control) = &self.audio_control {
            let _ = control.send(AudioCommand {
                playing: state.playing,
                position_ms,
                rate: state.rate,
                loop_start_ms: state.loop_start_ms,
                loop_end_ms: state.loop_end_ms,
                volume: self.audio_volume,
                muted: self.audio_muted,
                resync: true,
                issued_at: Instant::now(),
            });
        }
    }

    /// The point of PieceOff is moving pieces: the host's fresh room starts
    /// looping as soon as the media is shared, once per room.
    pub(crate) fn autoplay_for_host(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let is_host = self.client_id.as_deref() == Some(snapshot.host_id.as_str());
        let untouched = snapshot.playback.changed_at_ms == 0 && !snapshot.playback.playing;
        if !(is_host && self.connected && self.is_video && self.media_ready && untouched)
            || self.autoplay_room.as_deref() == Some(snapshot.id.as_str())
        {
            return;
        }
        self.autoplay_room = Some(snapshot.id.clone());
        self.set_playing(true);
    }

    pub(crate) fn handle_video_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.connected
            || !self.is_video
            || !self.media_ready
            || ctx.wants_keyboard_input()
            || self
                .snapshot
                .as_ref()
                .zip(self.client_id.as_deref())
                .is_none_or(|(snapshot, client_id)| snapshot.host_id != client_id)
        {
            return;
        }
        if self.rebinding.is_some() {
            return;
        }
        let toggle = self.controls.pressed(ctx, Action::PlayPause);
        let back = self.controls.pressed(ctx, Action::SeekBack);
        let forward = self.controls.pressed(ctx, Action::SeekForward);
        let start = self.controls.pressed(ctx, Action::LoopStart);
        let end = self.controls.pressed(ctx, Action::LoopEnd);
        if toggle {
            let playing = self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.playback.playing);
            self.set_playing(!playing);
            return;
        }
        let step = if start {
            SeekStep::LoopStart
        } else if end {
            SeekStep::LoopEnd
        } else if back {
            SeekStep::Back
        } else if forward {
            SeekStep::Forward
        } else {
            return;
        };
        self.seek_step(step);
    }

    pub(crate) fn set_playing(&mut self, playing: bool) {
        if let Some(snapshot) = &self.snapshot {
            let position = if snapshot.playback.playing {
                estimated_position_ms(&snapshot.playback)
            } else {
                self.video_position_ms
            };
            self.publish_playback(playing, position);
        }
    }

    pub(crate) fn publish_playback(&mut self, playing: bool, position_ms: u64) {
        let loop_start_ms = self
            .loop_start_ms
            .min(self.media_duration_ms.saturating_sub(1));
        let mut loop_end_ms = self.loop_end_ms.min(self.media_duration_ms);
        if self.media_duration_ms > 0 && loop_end_ms <= loop_start_ms {
            loop_end_ms = self.media_duration_ms;
        }
        let position_ms =
            if position_ms < loop_start_ms || (loop_end_ms > 0 && position_ms >= loop_end_ms) {
                loop_start_ms
            } else {
                position_ms
            };
        self.video_position_ms = position_ms;
        self.loop_start_ms = loop_start_ms;
        self.loop_end_ms = loop_end_ms;
        self.last_playback_send = Some(PlaybackState {
            playing,
            position_ms,
            rate: self.playback_rate,
            loop_start_ms,
            loop_end_ms,
            changed_at_ms: 0,
        });
        self.send(ClientMessage::SetPlayback {
            playing,
            position_ms,
            rate: self.playback_rate,
            loop_start_ms,
            loop_end_ms,
        });
    }

    pub(crate) fn toggle_still_assist(&mut self, ctx: &egui::Context) {
        if self.still_assist_texture.take().is_some() {
            return;
        }
        if let Some(frame) = &self.last_video_frame {
            self.still_assist_texture = Some(ctx.load_texture(
                "still-frame-assist",
                capture_still_frame(frame),
                TextureOptions::LINEAR,
            ));
        }
    }
}

/// Where the room's playback is now. The server stamps playback changes with
/// its own clock, so read the server's clock too: a client whose clock is off
/// would otherwise play ahead of or behind everyone else.
pub(crate) fn estimated_position_ms(playback: &PlaybackState) -> u64 {
    estimated_position_at(playback, crate::board::server_clock_ms())
}

/// Position at `now_ms` without wrapping at the loop end.
pub(crate) fn unwrapped_position_at(playback: &PlaybackState, now_ms: u64) -> u64 {
    if !playback.playing {
        return playback.position_ms;
    }
    let elapsed = now_ms.saturating_sub(playback.changed_at_ms);
    playback
        .position_ms
        .saturating_add((elapsed as f64 * playback.rate as f64) as u64)
}

/// Where the current item ends: its loop out, or the whole clip.
pub(crate) fn item_end_ms(playback: &PlaybackState, duration_ms: u64) -> u64 {
    if playback.loop_end_ms > 0 {
        playback.loop_end_ms
    } else {
        duration_ms
    }
}

pub(crate) fn estimated_position_at(playback: &PlaybackState, now_ms: u64) -> u64 {
    if !playback.playing {
        return playback.position_ms;
    }
    let elapsed = now_ms.saturating_sub(playback.changed_at_ms);
    let mut position = playback
        .position_ms
        .saturating_add((elapsed as f64 * playback.rate as f64) as u64);
    if playback.loop_end_ms > playback.loop_start_ms && position >= playback.loop_end_ms {
        let span = playback.loop_end_ms - playback.loop_start_ms;
        position = playback.loop_start_ms + (position - playback.loop_start_ms) % span;
    }
    position
}

pub(crate) fn seek_loop_by(
    position_ms: u64,
    delta_ms: i64,
    loop_start_ms: u64,
    loop_end_ms: u64,
) -> u64 {
    let min = loop_start_ms;
    let max = loop_end_ms.saturating_sub(1).max(min);
    let position_ms = position_ms.clamp(min, max);
    if delta_ms < 0 {
        position_ms.saturating_sub(delta_ms.unsigned_abs()).max(min)
    } else {
        position_ms.saturating_add(delta_ms as u64).min(max)
    }
}

pub(crate) fn looped_position_distance_ms(
    a: u64,
    b: u64,
    loop_start_ms: u64,
    loop_end_ms: u64,
) -> u64 {
    if loop_end_ms <= loop_start_ms {
        return a.abs_diff(b);
    }
    let span = loop_end_ms - loop_start_ms;
    let normalize = |position: u64| loop_start_ms + position.saturating_sub(loop_start_ms) % span;
    let a = normalize(a);
    let b = normalize(b);
    let straight = a.abs_diff(b);
    straight.min(span.saturating_sub(straight))
}
