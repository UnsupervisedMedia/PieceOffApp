//! The library shelf shown on the start screen, and the app actions behind it.

use super::*;
use eframe::egui::RichText;

const CARD_WIDTH: f32 = 224.0;
const THUMB_HEIGHT: f32 = 126.0;

const BUILT_INS: [(&str, &str, &str, &[u8]); 8] = [
    (
        "Slow Orbit",
        "Psychedelic · soft colour fields drift in a seamless loop",
        "Slow Orbit.webp",
        include_bytes!("../assets/builtin/Slow Orbit.webp"),
    ),
    (
        "Tidal Glass",
        "Ocean inspired · slow rolling bands and moving light",
        "Tidal Glass.webp",
        include_bytes!("../assets/builtin/Tidal Glass.webp"),
    ),
    (
        "Aurora Veil",
        "Aurora · flowing curtains of northern light",
        "Aurora Veil.webp",
        include_bytes!("../assets/builtin/Aurora Veil.webp"),
    ),
    (
        "Dune Light",
        "Desert · sculpted warm dunes beneath a moving glow",
        "Dune Light.webp",
        include_bytes!("../assets/builtin/Dune Light.webp"),
    ),
    (
        "Leaf Garden",
        "Botanical · layered leaves sway gently and return",
        "Leaf Garden.webp",
        include_bytes!("../assets/builtin/Leaf Garden.webp"),
    ),
    (
        "Glass Garden",
        "Kaleidoscopic · jewel-like glass catches the light",
        "Glass Garden.webp",
        include_bytes!("../assets/builtin/Glass Garden.webp"),
    ),
    (
        "Deep Sea Bloom",
        "Underwater · a bioluminescent bloom pulses slowly",
        "Deep Sea Bloom.webp",
        include_bytes!("../assets/builtin/Deep Sea Bloom.webp"),
    ),
    (
        "Geometric Bloom",
        "Psychedelic · a radial flower and moving colour wave",
        "Geometric Bloom.webp",
        include_bytes!("../assets/builtin/Geometric Bloom.webp"),
    ),
];

fn format_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let size = bytes as f64;
    if size >= 1024.0 * MIB {
        format!("{:.1} GiB", size / (1024.0 * MIB))
    } else if size >= MIB {
        format!("{:.0} MiB", size / MIB)
    } else {
        format!("{:.0} KiB", (size / 1024.0).max(1.0))
    }
}

/// What a card asked for this frame.
enum CardAction {
    Resume,
    NewGame,
    Export,
    AskDelete,
    ConfirmDelete,
    CancelDelete,
}

impl PieceOffApp {
    fn open_builtin(&mut self, title: &str, filename: &str, bytes: &[u8], ctx: &egui::Context) {
        if self.network.is_some() || self.probe_rx.is_some() {
            return;
        }
        let path = self.library.dir.join("built-in-media").join(filename);
        let result = (|| {
            std::fs::create_dir_all(path.parent().ok_or("Invalid built-in media path")?)
                .map_err(|error| error.to_string())?;
            let needs_update = std::fs::read(&path)
                .map(|existing| existing.as_slice() != bytes)
                .unwrap_or(true);
            if needs_update {
                std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
            }
            Ok::<_, String>(())
        })();
        match result {
            Ok(()) => {
                self.begin_probe(path, true, ctx);
                self.title = title.to_owned();
            }
            Err(error) => self.status = format!("Could not prepare built-in puzzle: {error}"),
        }
    }

    /// Start a background job that puts the live puzzle on the shelf: a new
    /// entry, or a refresh of the entry this room was resumed from.
    pub(crate) fn save_to_library(&mut self) {
        let (Some(snapshot), Some(media_path)) = (&self.snapshot, &self.media_path) else {
            self.status = "Join a room with its media loaded before saving to the library.".into();
            return;
        };
        let mut paused = snapshot.clone();
        paused.playback.position_ms = estimated_position_ms(&snapshot.playback);
        paused.playback.playing = false;
        paused.playback.changed_at_ms = 0;
        let label = if self.library_entry_id.is_some() {
            format!("Updated “{}” in the library.", paused.title)
        } else {
            format!("Saved “{}” to the library.", paused.title)
        };
        let request = library::SaveRequest {
            dir: self.library.dir.clone(),
            existing_id: self.library_entry_id.clone(),
            snapshot: paused,
            media_path: media_path.clone(),
            is_video: self.is_video,
            ffmpeg: self.decoder_ffmpeg_program.clone(),
        };
        self.spawn_library_job(label, move || library::save_to_library(request));
        self.status = "Saving to the library…".into();
    }

    pub(crate) fn import_into_library(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("PieceOff puzzle package", &["pieceoff"])
            .pick_files()
        else {
            return;
        };
        for path in paths {
            let dir = self.library.dir.clone();
            let ffmpeg = self.decoder_ffmpeg_program.clone();
            let label = format!("Imported {} into the library.", file_label(&path));
            self.spawn_library_job(label, move || {
                library::import_into_library(dir, path, ffmpeg)
            });
        }
        self.status = "Importing into the library…".into();
    }

    fn spawn_library_job(
        &mut self,
        label: String,
        work: impl FnOnce() -> Result<library::LibraryEntry, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let repaint = self.repaint.clone();
        thread::spawn(move || {
            let _ = tx.send(work());
            if let Some(repaint) = repaint {
                repaint.request_repaint();
            }
        });
        self.library_jobs
            .push(library::LibraryJob { label, outcome: rx });
    }

    /// Record finished background jobs in the index.
    pub(crate) fn poll_library_jobs(&mut self) {
        let mut index = 0;
        while index < self.library_jobs.len() {
            let result = match self.library_jobs[index].outcome.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.library_jobs.remove(index);
                    self.status = "A library task ended unexpectedly.".into();
                    continue;
                }
            };
            let job = self.library_jobs.remove(index);
            match result.and_then(|entry| {
                let id = entry.id.clone();
                self.library.upsert(entry).map(|()| id)
            }) {
                Ok(id) => {
                    self.library_thumbs.remove(&id);
                    self.status = job.label;
                }
                Err(error) => self.status = format!("Could not update the library: {error}"),
            }
        }
    }

    /// Load a library puzzle. `resume` rehosts its saved progress at once;
    /// otherwise only the media is loaded so a new game can be set up.
    fn open_library_entry(&mut self, id: &str, resume: bool, ctx: &egui::Context) {
        let path = self.library.package_path(id);
        self.open_package_at(&path, ctx);
        if self.pending_snapshot.is_none() {
            return;
        }
        if resume {
            self.library_entry_id = Some(id.to_owned());
            self.create_room();
        } else {
            self.pending_snapshot = None;
            self.status = "Choose a piece count, then create a room.".into();
        }
    }

    fn export_library_entry(&mut self, id: &str) {
        let Some(entry) = self.library.entries.iter().find(|entry| entry.id == id) else {
            return;
        };
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("PieceOff puzzle package", &["pieceoff"])
            .set_file_name(format!("{}.pieceoff", entry.title))
            .save_file()
        else {
            return;
        };
        match std::fs::copy(self.library.package_path(id), &destination) {
            Ok(_) => self.status = format!("Exported to {}.", destination.display()),
            Err(error) => self.status = format!("Could not export the puzzle: {error}"),
        }
    }

    fn delete_library_entry(&mut self, id: &str) {
        match self.library.remove(id) {
            Ok(()) => {
                self.library_thumbs.remove(id);
                if self.library_entry_id.as_deref() == Some(id) {
                    self.library_entry_id = None;
                }
                self.status = "Removed the puzzle from the library.".into();
            }
            Err(error) => self.status = format!("Could not delete the puzzle: {error}"),
        }
    }

    fn library_thumbnail(
        &mut self,
        ctx: &egui::Context,
        id: &str,
    ) -> Option<(egui::TextureId, Vec2)> {
        if !self.library_thumbs.contains_key(id) {
            let texture = std::fs::read(self.library.thumbnail_path(id))
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
                .map(|image| {
                    ctx.load_texture(
                        format!("library-{id}"),
                        image_to_color(image),
                        TextureOptions::LINEAR,
                    )
                });
            self.library_thumbs.insert(id.to_owned(), texture);
        }
        self.library_thumbs
            .get(id)
            .and_then(Option::as_ref)
            .map(|texture| (texture.id(), texture.size_vec2()))
    }

    /// The shelf: search, sort, and one card per saved puzzle.
    pub(crate) fn library_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("Built-in puzzles").size(18.0).strong());
            ui.weak("Original PieceOff scenes");
        });
        let busy = self.network.is_some() || self.probe_rx.is_some();
        let mut builtin_choice = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(12.0);
            for (title, description, filename, bytes) in BUILT_INS {
                if ui
                    .add_enabled(!busy, egui::Button::new(title))
                    .on_hover_text(description)
                    .clicked()
                {
                    builtin_choice = Some((title, filename, bytes));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.weak("Eight original procedural loops · silent, seamless, and native to PieceOff.");
        });
        if let Some((title, filename, bytes)) = builtin_choice {
            self.open_builtin(title, filename, bytes, ctx);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("Library").size(20.0).strong());
            ui.weak(format!("{} puzzles", self.library.entries.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Import…")
                    .on_hover_text("Add .pieceoff packages")
                    .clicked()
                {
                    self.import_into_library();
                }
                egui::ComboBox::from_id_salt("library-sort")
                    .selected_text(self.library_sort.label())
                    .show_ui(ui, |ui| {
                        for sort in library::LibrarySort::ALL {
                            ui.selectable_value(&mut self.library_sort, sort, sort.label());
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut self.library_search)
                        .hint_text("Search puzzles")
                        .desired_width(200.0),
                );
            });
        });
        if let Some(notice) = &self.library_notice {
            ui.colored_label(Color32::from_rgb(245, 190, 80), notice);
        }
        if !self.library_jobs.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Working on the library…");
            });
        }
        ui.separator();
        if self.library.entries.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Your library is empty").size(17.0));
                ui.add_space(4.0);
                ui.weak("Puzzles you save appear here with their progress.");
                ui.weak("Use More → Save to library in a room, or Import a .pieceoff file.");
                ui.add_space(8.0);
                ui.weak(
                    "Choose a built-in scene above, select media, or drop a file on the window.",
                );
            });
            return;
        }
        let matches: Vec<library::LibraryEntry> = self
            .library
            .filtered(&self.library_search, self.library_sort)
            .into_iter()
            .cloned()
            .collect();
        if matches.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.weak(format!(
                    "No puzzles match “{}”.",
                    self.library_search.trim()
                ));
            });
            return;
        }
        let mut requested: Option<(String, CardAction)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(12.0);
                ui.add_space(4.0);
                // Cards are laid out by hand: a wrapping layout sizes framed
                // children by the whole remaining area.
                let columns =
                    (((ui.available_width() - 8.0) / (CARD_WIDTH + 32.0)) as usize).max(1);
                for row in matches.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        ui.add_space(8.0);
                        for entry in row {
                            let thumb = self.library_thumbnail(ctx, &entry.id);
                            let deleting =
                                self.library_delete_pending.as_deref() == Some(&entry.id);
                            if let Some(action) = library_card(ui, entry, thumb, deleting, busy) {
                                requested = Some((entry.id.clone(), action));
                            }
                        }
                    });
                }
            });
        if let Some((id, action)) = requested {
            match action {
                CardAction::Resume => self.open_library_entry(&id, true, ctx),
                CardAction::NewGame => self.open_library_entry(&id, false, ctx),
                CardAction::Export => self.export_library_entry(&id),
                CardAction::AskDelete => self.library_delete_pending = Some(id),
                CardAction::CancelDelete => self.library_delete_pending = None,
                CardAction::ConfirmDelete => {
                    self.library_delete_pending = None;
                    self.delete_library_entry(&id);
                }
            }
        }
    }
}

fn library_card(
    ui: &mut egui::Ui,
    entry: &library::LibraryEntry,
    thumb: Option<(egui::TextureId, Vec2)>,
    deleting: bool,
    busy: bool,
) -> Option<CardAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(Color32::from_rgb(32, 36, 44))
        .corner_radius(10)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(CARD_WIDTH);
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                let (frame, _) =
                    ui.allocate_exact_size(Vec2::new(CARD_WIDTH, THUMB_HEIGHT), Sense::hover());
                ui.painter()
                    .rect_filled(frame, 6.0, Color32::from_rgb(17, 19, 24));
                match thumb {
                    Some((texture, size)) => {
                        let image_rect = fit_media_rect(frame, size);
                        ui.painter().image(
                            texture,
                            image_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().text(
                            frame.center(),
                            egui::Align2::CENTER_CENTER,
                            if entry.is_video { "Video" } else { "Image" },
                            egui::FontId::proportional(14.0),
                            Color32::from_gray(120),
                        );
                    }
                }
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(&entry.title).strong()).truncate());
                let kind = if entry.is_video { "Video" } else { "Image" };
                ui.weak(format!(
                    "{} pieces · {kind} · {}",
                    entry.pieces(),
                    format_size(entry.package_bytes)
                ));
                let progress = f32::from(entry.progress_percent) / 100.0;
                ui.add(egui::ProgressBar::new(progress).desired_height(8.0).fill(
                    if entry.finished() {
                        SUCCESS
                    } else {
                        ACCENT_FILL
                    },
                ));
                ui.weak(entry.status_line());
                if deleting {
                    ui.label("Delete this puzzle from the library?");
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Delete").color(Color32::from_rgb(240, 110, 110)))
                            .clicked()
                        {
                            action = Some(CardAction::ConfirmDelete);
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(CardAction::CancelDelete);
                        }
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    if !entry.finished() {
                        let label = if entry.progress_percent == 0 {
                            "Play"
                        } else {
                            "Resume"
                        };
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(
                                    RichText::new(label).strong().color(Color32::WHITE),
                                )
                                .fill(ACCENT_FILL),
                            )
                            .on_hover_text("Open a room with the saved progress")
                            .clicked()
                        {
                            action = Some(CardAction::Resume);
                        }
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("New game"))
                        .on_hover_text("Cut the same picture again, with a piece count you choose")
                        .clicked()
                    {
                        action = Some(CardAction::NewGame);
                    }
                    ui.menu_button("More", |ui| {
                        if ui.button("Export…").clicked() {
                            ui.close();
                            action = Some(CardAction::Export);
                        }
                        if ui.button("Delete…").clicked() {
                            ui.close();
                            action = Some(CardAction::AskDelete);
                        }
                    });
                });
            });
        });
    action
}
