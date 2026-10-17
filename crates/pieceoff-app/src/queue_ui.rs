//! The room's video queue: adding files, picking what plays, repeat mode, and
//! the video lock. The server owns the queue; this only sends requests.

use super::*;

impl PieceOffApp {
    /// While a large file is moving, say how far along it is.
    pub(crate) fn show_transfer_progress(&mut self) {
        let line = if let Some((done, total)) = UPLOAD_PROGRESS.current() {
            Some(format!(
                "Uploading the video to the room: {} of {}",
                format_bytes(done),
                format_bytes(total)
            ))
        } else {
            DOWNLOAD_PROGRESS.current().map(|(done, total)| {
                format!(
                    "Downloading the room's video: {} of {}",
                    format_bytes(done),
                    format_bytes(total)
                )
            })
        };
        if let Some(line) = line {
            self.status = line;
        }
    }

    /// Only the host may change the queue, and nobody may while it is locked.
    pub(crate) fn queue_controls_enabled(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            !snapshot.video_lock && self.client_id.as_deref() == Some(snapshot.host_id.as_str())
        })
    }

    /// Probe and hash a local file on a worker thread (it can be several
    /// gigabytes), then `poll_queue_probe` asks the room to queue it.
    pub(crate) fn send_queue_add_from_file(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.queue_probe_rx = Some(rx);
        self.status = "Reading the video before adding it…".into();
        let repaint = self.repaint.clone();
        thread::spawn(move || {
            let result = inspect_media(&path);
            let _ = tx.send((path, result));
            if let Some(repaint) = repaint {
                repaint.request_repaint();
            }
        });
    }

    pub(crate) fn poll_queue_probe(&mut self) {
        let Some(rx) = &self.queue_probe_rx else {
            return;
        };
        let (path, result) = match rx.try_recv() {
            Ok(finished) => finished,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.queue_probe_rx = None;
                return;
            }
        };
        self.queue_probe_rx = None;
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                self.status = format!("Could not add that file: {error}");
                return;
            }
        };
        let extension = match supported_media_extension(&path) {
            Ok(extension) => extension,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled")
            .to_owned();
        self.media_files
            .lock()
            .unwrap()
            .insert(metadata.sha256.clone(), path);
        self.send(ClientMessage::QueueAdd {
            sha256: metadata.sha256,
            extension,
            title,
            width: metadata.size.0,
            height: metadata.size.1,
            duration_ms: metadata.duration_ms,
        });
        self.status = "Adding the video to the queue…".into();
    }

    /// Run a queue key binding; a no-op for anyone who cannot change the queue.
    pub(crate) fn run_queue_action(&mut self, action: Action) {
        if !self.queue_controls_enabled() {
            return;
        }
        let Some(repeat) = self.snapshot.as_ref().map(|snapshot| snapshot.repeat) else {
            return;
        };
        match action {
            Action::NextVideo => self.send(ClientMessage::NextItem),
            Action::PreviousVideo => self.send(ClientMessage::PrevItem),
            Action::CycleRepeat => {
                let mode = match repeat {
                    RepeatMode::Off => RepeatMode::One,
                    RepeatMode::One => RepeatMode::All,
                    RepeatMode::All => RepeatMode::Off,
                };
                self.send(ClientMessage::SetRepeat { mode });
            }
            _ => {}
        }
    }

    pub(crate) fn handle_queue_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.connected || ctx.wants_keyboard_input() || self.rebinding.is_some() {
            return;
        }
        for action in [
            Action::NextVideo,
            Action::PreviousVideo,
            Action::CycleRepeat,
        ] {
            if self.controls.pressed(ctx, action) {
                self.run_queue_action(action);
                return;
            }
        }
    }

    pub(crate) fn queue_window(&mut self, ctx: &egui::Context) {
        let Some(snapshot) = self.snapshot.clone() else {
            self.show_queue = false;
            return;
        };
        let can_edit = self.queue_controls_enabled();
        let is_host = self.is_host();
        let mut open = true;
        let mut select = None;
        let mut remove = None;
        let mut swap = None;
        let mut add = false;
        let mut repeat = snapshot.repeat;
        let mut locked = snapshot.video_lock;
        egui::Window::new("Videos")
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                if snapshot.video_lock {
                    ui.colored_label(ACCENT, "Video locked by the host.");
                }
                let count = snapshot.queue.len();
                egui::Grid::new("video-queue")
                    .num_columns(3)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for (index, item) in snapshot.queue.iter().enumerate() {
                            let playing = index == snapshot.current;
                            ui.label(if playing { "▶" } else { "" });
                            let mut label = item.title.clone();
                            if !item.available {
                                label.push_str(" (uploading…)");
                            }
                            if ui
                                .add_enabled(can_edit && !playing, egui::Button::new(label))
                                .on_hover_text("Play this video now. Puzzle progress is kept.")
                                .clicked()
                            {
                                select = Some(index);
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        can_edit && index > 0,
                                        egui::Button::new("↑").small(),
                                    )
                                    .clicked()
                                {
                                    swap = Some((index, index - 1));
                                }
                                if ui
                                    .add_enabled(
                                        can_edit && index + 1 < count,
                                        egui::Button::new("↓").small(),
                                    )
                                    .clicked()
                                {
                                    swap = Some((index, index + 1));
                                }
                                if ui
                                    .add_enabled(
                                        can_edit && count > 1,
                                        egui::Button::new("✕").small(),
                                    )
                                    .on_hover_text("Remove from the queue")
                                    .clicked()
                                {
                                    remove = Some(index);
                                }
                            });
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                if ui
                    .add_enabled(can_edit, egui::Button::new("Add video…"))
                    .clicked()
                {
                    add = true;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Repeat");
                    for (mode, text) in [
                        (RepeatMode::Off, "Off"),
                        (RepeatMode::One, "This video"),
                        (RepeatMode::All, "All"),
                    ] {
                        ui.add_enabled_ui(can_edit, |ui| {
                            ui.selectable_value(&mut repeat, mode, text);
                        });
                    }
                });
                ui.add_enabled_ui(is_host, |ui| {
                    ui.checkbox(&mut locked, "Lock video (no changes until unlocked)");
                });
                if !is_host {
                    ui.weak("Only the host can change the queue.");
                }
            });
        if let Some(index) = select {
            self.send(ClientMessage::SelectItem { index });
        }
        if let Some(index) = remove {
            self.send(ClientMessage::QueueRemove { index });
        }
        if let Some((from, to)) = swap {
            self.send(ClientMessage::QueueMove { from, to });
        }
        if repeat != snapshot.repeat {
            self.send(ClientMessage::SetRepeat { mode: repeat });
        }
        if locked != snapshot.video_lock {
            self.send(ClientMessage::SetVideoLock { locked });
        }
        if add {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Media", pieceoff_core::SUPPORTED_MEDIA_EXTENSIONS)
                .pick_file()
            {
                self.send_queue_add_from_file(path);
            }
        }
        self.show_queue = open;
    }
}
