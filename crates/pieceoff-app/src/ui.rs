use super::*;
use eframe::egui::RichText;

pub(crate) const ACCENT: Color32 = Color32::from_rgb(96, 165, 250);
pub(crate) const ACCENT_FILL: Color32 = Color32::from_rgb(37, 99, 235);
pub(crate) const SUCCESS: Color32 = Color32::from_rgb(110, 220, 145);
const WARNING: Color32 = Color32::from_rgb(245, 190, 80);
const DANGER: Color32 = Color32::from_rgb(240, 110, 110);
const PANEL: Color32 = Color32::from_rgb(24, 27, 33);
const CARD: Color32 = Color32::from_rgb(32, 36, 44);
const CANVAS: Color32 = Color32::from_rgb(35, 38, 44);
const TOAST_SECONDS: f64 = 6.0;
/// Piece-count presets offered when creating a puzzle.
pub(crate) const PIECE_PRESETS: [(&str, u16); 4] =
    [("Easy", 12), ("Medium", 48), ("Hard", 108), ("Expert", 192)];

/// One dark look for the whole app: roomier controls, rounded corners, and a
/// single accent colour.
pub(crate) fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.window_margin = egui::Margin::same(12);
        let visuals = &mut style.visuals;
        visuals.panel_fill = PANEL;
        visuals.window_fill = CARD;
        visuals.extreme_bg_color = Color32::from_rgb(15, 17, 21);
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(8);
        visuals.selection.bg_fill = ACCENT_FILL;
        visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals.hyperlink_color = ACCENT;
        let radius = egui::CornerRadius::same(6);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = radius;
        }
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(46, 51, 62);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(60, 67, 82);
        visuals.widgets.active.weak_bg_fill = Color32::from_rgb(72, 80, 98);
    });
}

/// Rows and columns giving roughly `target` pieces that are close to square
/// for media of the given aspect ratio (width / height).
pub(crate) fn grid_for_piece_count(target: u16, aspect: f32) -> (u16, u16) {
    let target = f32::from(target.clamp(4, MAX_PUZZLE_PIECES));
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect.clamp(0.1, 10.0)
    } else {
        4.0 / 3.0
    };
    let mut columns = ((target * aspect).sqrt().round() as u16).clamp(2, 30);
    let mut rows = ((target / f32::from(columns)).round() as u16).clamp(2, 30);
    while rows * columns > MAX_PUZZLE_PIECES {
        if columns >= rows {
            columns -= 1;
        } else {
            rows -= 1;
        }
    }
    (rows, columns)
}

/// `grid_for_piece_count` with cells shaped for `style`, so hexagons and
/// triangles come out regular rather than stretched.
pub(crate) fn grid_for_style(target: u16, aspect: f32, style: CutStyle) -> (u16, u16) {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        4.0 / 3.0
    };
    grid_for_piece_count(target, aspect / style.cell_aspect())
}

fn cut_style_hint(style: CutStyle) -> &'static str {
    match style {
        CutStyle::Jigsaw => "Classic jigsaw pieces on a square grid",
        CutStyle::Hexagon => "Hexagons with knobs; up to six neighbours each",
        CutStyle::Triangle => "Triangles with small knobs; up to three neighbours each",
    }
}

impl PieceOffApp {
    /// Switch the cut style; a grid picked from a preset follows to the same
    /// preset in the new style.
    pub(crate) fn set_cut_style(&mut self, style: CutStyle, aspect: f32) {
        let preset = PIECE_PRESETS.iter().find(|(_, count)| {
            grid_for_style(*count, aspect, self.cut_style) == (self.rows, self.columns)
        });
        self.cut_style = style;
        if let Some((_, count)) = preset {
            (self.rows, self.columns) = grid_for_style(*count, aspect, style);
        }
    }
}

const MAX_LOGGED_MESSAGES: usize = 200;

/// One status message kept for the session's message history.
pub(crate) struct LogEntry {
    pub(crate) text: String,
    pub(crate) at: Instant,
    pub(crate) is_problem: bool,
}

/// Whether a status message reports something going wrong. Statuses are
/// plain sentences, so this looks for the wording the app uses for failures.
pub(crate) fn message_is_problem(text: &str) -> bool {
    // Player names are free text; never let one flag a join or leave notice.
    if text.ends_with(" joined the puzzle.") || text.contains(" left. Their pieces") {
        return false;
    }
    let text = text.to_ascii_lowercase();
    [
        "could not",
        "cannot",
        "can't",
        "failed",
        "error",
        "lost",
        "closed",
        "exceed",
        "invalid",
        "unsupported",
        "not found",
        "do not match",
        "does not match",
        "denied",
        "another player",
        "too large",
        "missing",
        "unavailable",
        "install ff",
        "only the host",
        "timed out",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// The toolbar's head count: "Just you", "3 players", "2 players · 1 watching".
pub(crate) fn presence_label(peers: usize, watchers: usize, spectating: bool) -> String {
    let players = peers + usize::from(!spectating);
    let watchers = watchers + usize::from(spectating);
    let players = match players {
        1 if !spectating => "Just you".to_owned(),
        1 => "1 player".to_owned(),
        count => format!("{count} players"),
    };
    if watchers == 0 {
        players
    } else {
        format!("{players} · {watchers} watching")
    }
}

pub(crate) fn format_ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..5 => "now".into(),
        seconds @ 5..60 => format!("{seconds}s ago"),
        seconds @ 60..3600 => format!("{}m ago", seconds / 60),
        seconds => format!("{}h {}m ago", seconds / 3600, seconds / 60 % 60),
    }
}

/// A filled dot used as a connection indicator.
fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn card<R>(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(15.0));
            ui.add_space(4.0);
            add_contents(ui)
        })
        .inner
}

fn primary_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
            .fill(ACCENT_FILL)
            .min_size(Vec2::new(ui.available_width(), 34.0)),
    )
}

impl PieceOffApp {
    pub(crate) fn render_media_preview(&self, ui: &mut egui::Ui) {
        let Some(texture) = self.texture.as_ref() else {
            ui.centered_and_justified(|ui| {
                if self.media_path.is_some() {
                    ui.label("Waiting for the first video frame…");
                } else {
                    ui.label(
                        RichText::new("Drop an image or video here, or choose one to preview it.")
                            .size(16.0)
                            .weak(),
                    );
                }
            });
            return;
        };
        let available = ui.available_size().max(Vec2::splat(1.0));
        let (response, painter) = ui.allocate_painter(available, Sense::hover());
        let canvas = response.rect.shrink(16.0);
        let image_rect = fit_media_rect(canvas, texture.size_vec2());
        painter.rect_filled(image_rect.expand(3.0), 6.0, Color32::from_rgb(17, 19, 24));
        painter.image(
            texture.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        // Show how the chosen grid and cut style will cut the picture.
        if self.pending_snapshot.is_none() {
            let stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(45));
            for outline in pieceoff_core::cut_preview(
                self.cut_style,
                self.rows,
                self.columns,
                image_rect.width(),
                image_rect.height(),
            ) {
                let points = outline
                    .iter()
                    .map(|point| image_rect.min + Vec2::new(point[0], point[1]))
                    .collect();
                painter.add(egui::Shape::closed_line(points, stroke));
            }
        }
    }

    /// A tooltip that names the keys currently bound to `action`.
    pub(crate) fn hint_with_keys(&self, hint: &str, action: Action) -> String {
        if self.controls.bindings_for(action).is_empty() {
            hint.to_owned()
        } else {
            format!("{hint} ({})", self.controls.describe(action))
        }
    }

    /// Board shortcuts; ignored while a text field has focus or a key is
    /// being rebound.
    pub(crate) fn handle_board_shortcuts(&mut self, ctx: &egui::Context) {
        // The chat box sets this again each frame it has the keyboard.
        let typing = std::mem::take(&mut self.chat.typing);
        if ctx.wants_keyboard_input() || typing || self.rebinding.is_some() {
            return;
        }
        let none = egui::Modifiers::NONE;
        if self.controls.pressed(ctx, Action::ToggleHelp) {
            self.help_open = !self.help_open;
        }
        if ctx.input_mut(|input| input.consume_key(none, egui::Key::Escape)) {
            if self.replay.is_some() {
                self.replay = None;
            } else if self.help_open {
                self.help_open = false;
            } else if self.controls_open {
                self.controls_open = false;
                self.rebinding = None;
            } else if self.messages_open {
                self.messages_open = false;
            } else if self.confirm_restart {
                self.confirm_restart = false;
            } else {
                self.selected_pieces.clear();
            }
        }
        if self.controls.pressed(ctx, Action::ToggleMessages) {
            self.messages_open = !self.messages_open;
        }
        if self.snapshot.is_none() {
            return;
        }
        let step = self.settings.mouse.zoom_step;
        for (action, command) in [
            (Action::FitView, ViewCommand::Fit),
            (Action::FocusBoard, ViewCommand::Board),
            (Action::FocusTray, ViewCommand::LoosePieces),
            (Action::ZoomIn, ViewCommand::ZoomBy(step)),
            (Action::ZoomOut, ViewCommand::ZoomBy(1.0 / step)),
        ] {
            if self.controls.pressed(ctx, action) {
                self.view_command = Some(command);
            }
        }
        if self.controls.pressed(ctx, Action::ToggleGhost) {
            self.settings.show_ghost = !self.settings.show_ghost;
            self.persist_settings();
        }
        // Spectators and replays leave the room's pieces alone.
        let playing = !self.spectating && self.replay.is_none();
        if self.controls.pressed(ctx, Action::TidyTray) && playing {
            self.arrange_tray(TrayArrangement::EdgesFirst);
        }
        if self.controls.pressed(ctx, Action::ShuffleTray) && playing {
            self.arrange_tray(TrayArrangement::Shuffle);
        }
        if self.controls.pressed(ctx, Action::FocusChat) {
            self.chat.show();
            self.chat.focus_requested = true;
        }
        if self.controls.pressed(ctx, Action::Ping) {
            self.chat.ping_requested = true;
        }
        if self.controls.pressed(ctx, Action::NewHolder) && playing {
            self.holders.create_requested = true;
        }
    }

    /// Files dropped on the window become the puzzle media (or package).
    fn accept_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        });
        let Some(path) = dropped else { return };
        if self.snapshot.is_some() || self.network.is_some() || self.probe_rx.is_some() {
            self.status = "Leave the room before opening another puzzle.".into();
            return;
        }
        self.open_path(path, ctx);
    }

    /// Open a media file or a `.pieceoff` package, whichever `path` is.
    fn open_path(&mut self, path: PathBuf, ctx: &egui::Context) {
        let is_package = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pieceoff"));
        if is_package {
            self.open_package_at(&path, ctx);
        } else {
            self.pending_snapshot = None;
            self.begin_probe(path, false, ctx);
        }
    }

    fn room_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, is_host: bool) {
        let Some((code, title, media_available)) = self.snapshot.as_ref().map(|snapshot| {
            (
                snapshot.invite_code.clone(),
                snapshot.title.clone(),
                snapshot.media_available,
            )
        }) else {
            return;
        };
        ui.label(RichText::new(title).strong());
        if ui
            .button(RichText::new(&code).monospace())
            .on_hover_text("Copy the invite code")
            .clicked()
        {
            ctx.copy_text(code);
            self.status = "Invite code copied.".into();
        }
        if self.connected {
            status_dot(ui, SUCCESS);
            ui.menu_button(
                presence_label(self.peers.len(), self.spectators.len(), self.spectating),
                |ui| {
                    let me = if is_host {
                        format!("{} (you, host)", self.player_name)
                    } else if self.spectating {
                        format!("{} (you, watching)", self.player_name)
                    } else {
                        format!("{} (you)", self.player_name)
                    };
                    ui.label(RichText::new(me).strong());
                    for peer in &self.peers {
                        ui.label(peer);
                    }
                    if self.peers.is_empty() && !self.spectating {
                        ui.weak("Share the invite code to play together.");
                    }
                    if !self.spectators.is_empty() {
                        ui.separator();
                        ui.weak(format!("Watching: {}", self.spectators.join(", ")));
                    }
                },
            );
            if self.spectating {
                ui.label(RichText::new("Watching").color(ACCENT))
                    .on_hover_text(
                        "You joined to watch: chat works, pieces and holders do not move",
                    );
            }
        } else if self.network.is_some() {
            status_dot(ui, WARNING);
            ui.colored_label(WARNING, "Connecting…");
        } else {
            status_dot(ui, DANGER);
            ui.colored_label(DANGER, "Offline");
            if ui.button("Rejoin").clicked() {
                self.join_room();
            }
        }
        if self.media_uploading {
            ui.spinner();
            ui.label("Uploading media…");
        } else if !media_available {
            ui.colored_label(WARNING, "Media only on this device")
                .on_hover_text("Other players cannot download the media from the server yet.");
        }
        ui.separator();
        let zoom_step = self.settings.mouse.zoom_step;
        for (label, hint, action, command) in [
            ("Fit", "Show everything", Action::FitView, ViewCommand::Fit),
            (
                "Board",
                "Focus the board",
                Action::FocusBoard,
                ViewCommand::Board,
            ),
            (
                "Tray",
                "Focus the loose pieces",
                Action::FocusTray,
                ViewCommand::LoosePieces,
            ),
            (
                "−",
                "Zoom out",
                Action::ZoomOut,
                ViewCommand::ZoomBy(1.0 / zoom_step),
            ),
            (
                "+",
                "Zoom in",
                Action::ZoomIn,
                ViewCommand::ZoomBy(zoom_step),
            ),
        ] {
            let hint = self.hint_with_keys(hint, action);
            if ui.button(label).on_hover_text(hint).clicked() {
                self.view_command = Some(command);
            }
        }
        let lock_hint = if self.settings.lock_view {
            "Board locked: pan and zoom are disabled. Click to unlock"
        } else {
            "Lock the board so it cannot be panned or zoomed"
        };
        if ui
            .selectable_label(self.settings.lock_view, "Lock")
            .on_hover_text(lock_hint)
            .clicked()
        {
            self.settings.lock_view = !self.settings.lock_view;
            self.persist_settings();
        }
        ui.separator();
        for (filter, label) in [
            (PieceFilter::All, "All"),
            (PieceFilter::Edges, "Edges"),
            (PieceFilter::Interior, "Interior"),
        ] {
            if ui
                .selectable_label(self.piece_filter == filter, label)
                .on_hover_text("Filter the loose pieces in the tray")
                .clicked()
            {
                self.piece_filter = filter;
                self.selected_pieces.clear();
            }
        }
        ui.separator();
        if !self.spectating {
            ui.menu_button("Arrange", |ui| {
                if ui
                    .button("Edges first")
                    .on_hover_text(self.hint_with_keys(
                        "Corners, then edges, then the rest, in order",
                        Action::TidyTray,
                    ))
                    .clicked()
                {
                    ui.close();
                    self.arrange_tray(TrayArrangement::EdgesFirst);
                }
                if ui
                    .button("Shuffle")
                    .on_hover_text(self.hint_with_keys(
                        "Mix the loose pieces into a new order",
                        Action::ShuffleTray,
                    ))
                    .clicked()
                {
                    ui.close();
                    self.arrange_tray(TrayArrangement::Shuffle);
                }
                ui.weak("Assembled and held pieces, and pieces in holders, stay put.");
            })
            .response
            .on_hover_text("Tidy the loose pieces in the tray");
            ui.menu_button("Holders", |ui| self.holders_menu(ui, is_host))
                .response
                .on_hover_text("Your own sorting areas on the table");
        }
        ui.menu_button("More", |ui| {
            let mut ghost = self.settings.show_ghost;
            if ui
                .checkbox(
                    &mut ghost,
                    self.hint_with_keys("Board ghost", Action::ToggleGhost),
                )
                .on_hover_text("Show a faint copy of the media behind the board")
                .changed()
            {
                self.settings.show_ghost = ghost;
                self.persist_settings();
            }
            let mut cursors = !self.settings.hide_peer_cursors;
            if ui
                .checkbox(&mut cursors, "Show other players' cursors")
                .on_hover_text("Draw where everyone else is pointing, with their name")
                .changed()
            {
                self.settings.hide_peer_cursors = !cursors;
                self.persist_settings();
            }
            let mut timer = !self.settings.hide_solve_timer;
            if ui
                .checkbox(&mut timer, "Show solve timer")
                .on_hover_text("Show the room's shared solve clock beside the progress")
                .changed()
            {
                self.settings.hide_solve_timer = !timer;
                self.persist_settings();
            }
            self.effects_controls(ui);
            if !self.is_video && self.media_ready {
                ui.checkbox(&mut self.show_reference, "Reference image");
            }
            if ui.button("Controls…").clicked() {
                ui.close();
                self.controls_open = true;
            }
            if ui
                .button("Watch timelapse")
                .on_hover_text(
                    "Replay this solve so far from the scattered start, on your screen only",
                )
                .clicked()
            {
                ui.close();
                self.request_timelapse();
            }
            ui.separator();
            if ui
                .add_enabled(
                    self.media_path.is_some(),
                    egui::Button::new("Save package…"),
                )
                .clicked()
            {
                ui.close();
                self.save_package();
            }
            let library_label = if self.library_entry_id.is_some() {
                "Update library copy"
            } else {
                "Save to library"
            };
            if ui
                .add_enabled(self.media_path.is_some(), egui::Button::new(library_label))
                .on_hover_text("Keep this puzzle and its progress on your shelf")
                .clicked()
            {
                ui.close();
                self.save_to_library();
            }
            if self.is_video
                && ui
                    .button("Restart video")
                    .on_hover_text("Start the video decoder again if the picture has frozen")
                    .clicked()
            {
                ui.close();
                self.restart_video_decoder(ctx);
            }
            if is_host
                && ui
                    .button("Restart puzzle…")
                    .on_hover_text("Scatter all pieces and start over")
                    .clicked()
            {
                ui.close();
                self.confirm_restart = true;
            }
        });
        if ui
            .button("Videos")
            .on_hover_text("The room's video queue, repeat, and lock")
            .clicked()
        {
            self.show_queue = !self.show_queue;
        }
        if ui
            .button("?")
            .on_hover_text(self.hint_with_keys("Keyboard and mouse shortcuts", Action::ToggleHelp))
            .clicked()
        {
            self.help_open = !self.help_open;
        }
        ui.add_space(ui.available_width());
        if ui
            .add(egui::Button::new(RichText::new("Leave").color(Color32::WHITE)).fill(DANGER))
            .on_hover_text("Leave this room")
            .clicked()
        {
            self.leave_room();
        }
    }

    /// The start screen's side panel: pick media, create a room, or join one.
    fn lobby_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let idle = self.network.is_none() && self.probe_rx.is_none();
        card(ui, "Start a puzzle", |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(idle, egui::Button::new("Choose media…"))
                    .clicked()
                {
                    self.choose_media(ctx);
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Open package…"))
                    .clicked()
                {
                    self.open_package(ctx);
                }
            });
            if self.media_path.is_some()
                && idle
                && ui
                    .small_button("Back to library")
                    .on_hover_text("Drop this media and browse your saved puzzles")
                    .clicked()
            {
                self.back_to_library();
            }
            let Some(file_name) = self.media_path.as_ref().map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("media")
                    .to_owned()
            }) else {
                ui.weak("Pick an image or video, or drop one on the window.");
                return;
            };
            ui.label(format!(
                "{file_name} · {}×{}",
                self.media_size.0, self.media_size.1
            ));
            ui.add_space(4.0);
            ui.label("Title");
            ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY));
            if self.pending_snapshot.is_none() {
                let aspect = self.media_size.0 as f32 / self.media_size.1.max(1) as f32;
                ui.label("Cut");
                ui.horizontal(|ui| {
                    for style in CutStyle::ALL {
                        if ui
                            .selectable_label(self.cut_style == style, style.label())
                            .on_hover_text(cut_style_hint(style))
                            .clicked()
                        {
                            self.set_cut_style(style, aspect);
                        }
                    }
                });
                ui.label("Pieces");
                ui.horizontal(|ui| {
                    for (label, count) in PIECE_PRESETS {
                        let grid = grid_for_style(count, aspect, self.cut_style);
                        let tip = format!("{} × {} = {} pieces", grid.0, grid.1, grid.0 * grid.1);
                        if ui
                            .selectable_label((self.rows, self.columns) == grid, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            (self.rows, self.columns) = grid;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let max_rows = max_grid_dimension_for_other(self.columns);
                    ui.add(
                        egui::DragValue::new(&mut self.rows)
                            .range(2..=max_rows)
                            .suffix(" rows"),
                    );
                    let max_columns = max_grid_dimension_for_other(self.rows);
                    ui.add(
                        egui::DragValue::new(&mut self.columns)
                            .range(2..=max_columns)
                            .suffix(" cols"),
                    );
                    ui.weak(format!("{} pieces", self.rows * self.columns));
                });
            } else {
                ui.weak("This package keeps its saved pieces and progress.");
            }
            ui.add_space(6.0);
            let label = if self.pending_snapshot.is_some() {
                "Create room from package"
            } else {
                "Create room"
            };
            if primary_button(ui, self.network.is_none(), label).clicked() {
                self.create_room();
            }
        });
        ui.add_space(8.0);
        self.server_card(ui);
        ui.add_space(8.0);
        card(ui, "Join a room", |ui| {
            ui.horizontal(|ui| {
                let code = ui.add(
                    egui::TextEdit::singleline(&mut self.invite_code)
                        .hint_text("Invite code")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(120.0),
                );
                let submitted =
                    code.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let join = ui.add_enabled(
                    self.network.is_none() && !self.invite_code.trim().is_empty(),
                    egui::Button::new("Join"),
                );
                if join.clicked() || (submitted && self.network.is_none()) {
                    self.join_room();
                }
            });
            ui.checkbox(&mut self.watch_only, "Watch only")
                .on_hover_text("Join as a spectator: see the room and chat, without moving pieces or counting as a player");
            if !self.recent_rooms.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.weak("Recent rooms");
                    if self.recent_room_selection.is_some()
                        && self.network.is_none()
                        && ui
                            .small_button("Forget")
                            .on_hover_text("Remove the selected room from this list")
                            .clicked()
                    {
                        self.forget_selected_recent_room();
                    }
                });
                let mut selected = None;
                for (index, room) in self.recent_rooms.iter().enumerate() {
                    let label = format!("{} · {}", room.title, room.invite_code);
                    if ui
                        .selectable_label(self.recent_room_selection == Some(index), label)
                        .on_hover_text(&room.server_url)
                        .clicked()
                    {
                        selected = Some(index);
                    }
                }
                if let Some(index) = selected {
                    self.select_recent_room(index);
                }
            }
        });
        ui.add_space(8.0);
        card(ui, "You", |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.player_name).desired_width(f32::INFINITY));
            if ui.button("Controls…").clicked() {
                self.controls_open = true;
            }
            self.media_tools_section(ui);
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if self.network.is_some() || self.probe_rx.is_some() {
                ui.spinner();
            }
            ui.weak(&self.status);
        });
    }

    /// Where rooms are hosted and joined: this device by default, or any
    /// `host`, `host:port` or `ws(s)://` address.
    fn server_card(&mut self, ui: &mut egui::Ui) {
        card(ui, "Room server", |ui| {
            ui.horizontal(|ui| {
                let field = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.server_url)
                            .hint_text("host, host:port or ws://host:port/ws")
                            .desired_width((ui.available_width() - 70.0).max(80.0)),
                    )
                    .on_hover_text(format!("Source: {}", self.server_url_source));
                if field.changed() {
                    self.server_edit_dirty = true;
                }
                if field.lost_focus() {
                    self.commit_server_url_if_edited();
                }
                if ui
                    .add_enabled(self.network.is_none(), egui::Button::new("Reset"))
                    .on_hover_text("Use the server on this device")
                    .clicked()
                {
                    self.reset_server_url();
                }
            });
            match self.resolved_server_url() {
                Ok(url) if url != self.server_url.trim() => {
                    ui.weak(format!("Will connect to {url}"));
                }
                Ok(_) => {}
                Err(message) => {
                    ui.colored_label(WARNING, message);
                }
            }
        });
    }

    /// Which FFmpeg the app found, and a way to point it at another.
    fn media_tools_section(&mut self, ui: &mut egui::Ui) {
        let missing = self
            .tools
            .iter()
            .any(|report| report.resolved.source == media_tools::ToolSource::Missing);
        let title = if missing {
            RichText::new("Media tools (FFmpeg missing)").color(WARNING)
        } else {
            RichText::new("Media tools")
        };
        egui::CollapsingHeader::new(title)
            .id_salt("media-tools")
            .default_open(missing)
            .show(ui, |ui| {
                if self.tools.is_empty() {
                    ui.weak("Looking for FFmpeg…");
                }
                for report in &self.tools {
                    let name = report.tool.name();
                    match (&report.resolved.source, &report.version) {
                        (media_tools::ToolSource::Missing, _) => {
                            ui.colored_label(WARNING, format!("{name}: not found"));
                        }
                        (source, version) => {
                            ui.label(format!("{name}: {}", source.label()));
                            ui.weak(version.as_deref().unwrap_or("Found, but it did not run."));
                        }
                    }
                }
                if missing {
                    ui.weak("Video puzzles need FFmpeg. Install it, or choose the folder that holds ffmpeg and ffprobe.");
                }
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.choose_tools_directory(Some(dir));
                        }
                    }
                    if self.settings.ffmpeg_dir.is_some() && ui.button("Use default").clicked() {
                        self.choose_tools_directory(None);
                    }
                });
            });
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.help_open;
        let mut open_controls = false;
        egui::Window::new("Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let mouse = &self.settings.mouse;
                let select = format!("{}-click", mouse.select_modifier.label());
                let pan = if mouse.drag_empty_pans {
                    "Drag empty space".to_owned()
                } else {
                    "Shift-drag".to_owned()
                };
                egui::Grid::new("shortcuts")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        let mut rows: Vec<(String, String)> = vec![
                            (
                                "Drag a piece".into(),
                                "Move it (and anything joined or selected)".into(),
                            ),
                            (select, "Add or remove a piece from the selection".into()),
                            ("Esc".into(), "Clear the selection, close a window".into()),
                            (pan, "Pan (Shift-drag pans anywhere)".into()),
                            ("Mouse wheel".into(), "Zoom".into()),
                            ("Double-click".into(), "Fit everything".into()),
                        ];
                        if let Some(button) = mouse.pan_button.pointer_button() {
                            let name = if button == egui::PointerButton::Middle {
                                "Middle-drag"
                            } else {
                                "Right-drag"
                            };
                            rows.push((name.into(), "Pan".into()));
                        }
                        for action in Action::ALL {
                            if self.controls.bindings_for(action).is_empty() {
                                continue;
                            }
                            let what = if action.host_only() {
                                format!("{} (host)", action.label())
                            } else {
                                action.label().to_owned()
                            };
                            rows.push((self.controls.describe(action), what));
                        }
                        for (keys, action) in rows {
                            ui.label(RichText::new(keys).strong());
                            ui.label(action);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                if ui.button("Customize controls…").clicked() {
                    open_controls = true;
                }
            });
        self.help_open = open;
        if open_controls {
            self.controls_open = true;
        }
    }

    /// Log each new status message so it can be recalled after its toast
    /// has gone.
    pub(crate) fn record_status(&mut self, now: f64) {
        if self.toast.0 == self.status {
            return;
        }
        self.toast = (self.status.clone(), now);
        self.toast_dismissed = false;
        if self.status.is_empty() {
            return;
        }
        let is_problem = message_is_problem(&self.status);
        if is_problem && !self.messages_open {
            self.unseen_problems += 1;
        }
        if self.messages.len() >= MAX_LOGGED_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(LogEntry {
            text: self.status.clone(),
            at: Instant::now(),
            is_problem,
        });
    }

    /// Float the latest status message over the board. Ordinary messages
    /// fade; problems stay until dismissed or replaced.
    fn show_toast(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        let age = now - self.toast.1;
        let is_problem = message_is_problem(&self.status);
        if self.status.is_empty() || self.toast_dismissed || (!is_problem && age >= TOAST_SECONDS) {
            return;
        }
        let opacity = if is_problem {
            1.0
        } else {
            ((TOAST_SECONDS - age) / 0.4).clamp(0.0, 1.0) as f32
        };
        egui::Area::new(egui::Id::new("status-toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -14.0))
            .constrain_to(ctx.available_rect())
            .interactable(is_problem)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(225))
                    .stroke(if is_problem {
                        Stroke::new(1.0_f32, DANGER)
                    } else {
                        Stroke::NONE
                    })
                    .corner_radius(14)
                    .inner_margin(egui::Margin::symmetric(14, 7))
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.horizontal(|ui| {
                            let color = if is_problem { DANGER } else { Color32::WHITE };
                            ui.label(RichText::new(&self.status).color(color));
                            if is_problem && ui.small_button("Dismiss").clicked() {
                                self.toast_dismissed = true;
                            }
                        });
                    });
            });
        if !is_problem {
            ctx.request_repaint_after(Duration::from_secs_f64(
                (TOAST_SECONDS - age).clamp(0.02, 0.4),
            ));
        }
    }

    /// Toolbar button that opens the message history; it counts problems
    /// that arrived while the list was closed.
    fn messages_button(&mut self, ui: &mut egui::Ui) {
        let label = if self.unseen_problems > 0 {
            RichText::new(format!("Messages ({})", self.unseen_problems)).color(DANGER)
        } else {
            RichText::new("Messages")
        };
        if ui
            .button(label)
            .on_hover_text(self.hint_with_keys(
                "Everything PieceOff has reported this session",
                Action::ToggleMessages,
            ))
            .clicked()
        {
            self.messages_open = !self.messages_open;
        }
    }

    fn messages_window(&mut self, ctx: &egui::Context) {
        self.unseen_problems = 0;
        let mut open = true;
        let mut clear = false;
        egui::Window::new("Messages")
            .open(&mut open)
            .collapsible(false)
            .default_size([460.0, 320.0])
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-12.0, 48.0))
            .show(ctx, |ui| {
                if self.messages.is_empty() {
                    ui.weak("Nothing has been reported yet.");
                    return;
                }
                ui.horizontal(|ui| {
                    ui.weak(format!(
                        "{} this session, newest first",
                        self.messages.len()
                    ));
                    if ui.small_button("Clear").clicked() {
                        clear = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for entry in self.messages.iter().rev() {
                            ui.horizontal_top(|ui| {
                                ui.add_sized(
                                    [64.0, 0.0],
                                    egui::Label::new(
                                        RichText::new(format_ago(entry.at.elapsed())).weak(),
                                    ),
                                );
                                let text = RichText::new(&entry.text);
                                ui.add(
                                    egui::Label::new(if entry.is_problem {
                                        text.color(DANGER)
                                    } else {
                                        text
                                    })
                                    .wrap(),
                                );
                            });
                        }
                    });
            });
        if clear {
            self.messages.clear();
        }
        self.messages_open = open;
        // Keep the relative times fresh while the list is visible.
        ctx.request_repaint_after(Duration::from_secs(5));
    }
}

impl eframe::App for PieceOffApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }
}

impl PieceOffApp {
    /// One frame of the whole app: network, decoders, shortcuts, and every
    /// panel. `update` is this; tests call it directly because an
    /// `eframe::Frame` cannot be built outside a real window.
    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        if let Some(reconnect_at) = self.reconnect_at {
            if self.snapshot.is_none() || self.invite_code.trim().is_empty() {
                self.reconnect_at = None;
            } else if reconnect_at <= Instant::now() && self.network.is_none() {
                self.reconnect_at = None;
                let message = ClientMessage::JoinRoom {
                    invite_code: self.invite_code.clone(),
                    name: self.player_name.clone(),
                    media_sha256: self.media_hash.clone(),
                    media_extension: self.media_extension.clone(),
                    spectator: self.watch_only,
                    player_key: self.settings.player_key.clone(),
                };
                self.connect(message);
            } else if let Some(remaining) = reconnect_at.checked_duration_since(Instant::now()) {
                ctx.request_repaint_after(remaining);
            }
        }
        if let Some(path) = self.startup_media.take() {
            self.open_path(path, ctx);
        }
        if self.repaint.is_none() {
            self.repaint = Some(ctx.clone());
        }
        self.accept_dropped_files(ctx);
        self.poll_probe(ctx);
        self.process_network(ctx);
        self.show_transfer_progress();
        self.poll_queue_probe();
        self.hold_at_item_end();
        self.poll_library_jobs();
        if self.tools.is_empty() && self.tools_rx.is_none() {
            self.refresh_tools();
        }
        self.poll_tools();
        self.handle_video_shortcuts(ctx);
        self.handle_queue_shortcuts(ctx);
        self.handle_board_shortcuts(ctx);
        if let Some(errors) = &self.audio_errors {
            for error in errors.try_iter() {
                self.status = error;
            }
        }
        self.poll_decoder_errors();
        self.auto_recover_decoder(ctx);
        self.autoplay_for_host();
        self.drain_decoder_frames(ctx);
        if self.is_video && self.snapshot.as_ref().is_some_and(|s| s.playback.playing) {
            ctx.request_repaint_after(Duration::from_millis(40));
            if !self.seeking {
                if let Some(position) = self
                    .snapshot
                    .as_ref()
                    .map(|s| estimated_position_ms(&s.playback))
                {
                    self.video_position_ms = position;
                }
            }
            if self.last_drift_sync.elapsed() >= Duration::from_secs(3) {
                self.sync_decoder_drift();
                self.last_drift_sync = Instant::now();
            }
        }

        let in_room = self.snapshot.is_some();
        let is_host = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| self.client_id.as_deref() == Some(snapshot.host_id.as_str()));
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 7)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("PieceOff").size(19.0).strong().color(ACCENT));
                    ui.separator();
                    if in_room {
                        self.room_toolbar(ui, ctx, is_host);
                    } else {
                        ui.weak("Video-first multiplayer jigsaw puzzles");
                        ui.separator();
                    }
                    self.messages_button(ui);
                });
            });
        if in_room && self.is_video && self.media_ready {
            egui::TopBottomPanel::bottom("transport")
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show(ctx, |ui| self.render_transport(ui, ctx, is_host));
        }
        if !in_room && self.clip_plan.is_none() {
            egui::SidePanel::right("lobby")
                .resizable(false)
                .exact_width(340.0)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(12))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.lobby_panel(ui, ctx));
                });
        }
        if self.confirm_restart {
            egui::Window::new("Restart puzzle?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Every piece goes back to the tray and all progress is lost for everyone in the room.");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Restart").color(DANGER))
                            .clicked()
                        {
                            self.restart_puzzle();
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_restart = false;
                        }
                    });
                });
        }
        if self.help_open {
            self.help_window(ctx);
        }
        if self.show_queue {
            self.queue_window(ctx);
        }
        if self.controls_open {
            self.controls_window(ctx);
        }
        if self.messages_open {
            self.messages_window(ctx);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS))
            .show(ctx, |ui| {
                if let Some(plan) = self.clip_plan.as_mut() {
                    match plan.ui(ui) {
                        clip::ClipEvent::Idle => {}
                        clip::ClipEvent::Cancel => {
                            self.clip_plan = None;
                            self.clip_source_title = None;
                            self.status = "Choose an image or video, then create a room.".into();
                        }
                        clip::ClipEvent::Ready(path) => {
                            self.clip_plan = None;
                            self.begin_probe(path, true, ctx);
                        }
                        clip::ClipEvent::UseFull(path) => {
                            self.clip_plan = None;
                            self.begin_probe(path, true, ctx);
                        }
                    }
                } else if self.probe_rx.is_some() {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                    });
                } else if !in_room && self.media_path.is_none() {
                    self.library_view(ui, ctx);
                } else {
                    self.render_board(ui);
                }
            });
        if in_room {
            self.chat_overlay(ctx);
            self.replay_controls(ctx);
        }
        self.record_status(ctx.input(|input| input.time));
        if in_room || self.clip_plan.is_some() {
            self.show_toast(ctx);
        }
    }
}

impl PieceOffApp {
    /// The sound effects volume and mute, separate from the clip's audio.
    fn effects_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Effects")
                .on_hover_text("Snap, join, finish, chat and ping sounds");
            ui.spacing_mut().slider_width = 110.0;
            let mut volume = self.effects.volume;
            let slider = ui
                .add(egui::Slider::new(&mut volume, 0..=100).show_value(false))
                .on_hover_text(format!("Effects volume {volume}%"));
            if slider.changed() {
                self.effects.set_level(volume, false);
            }
            let released = slider.drag_stopped() || (slider.changed() && !slider.dragged());
            let silent = !self.effects.audible();
            let toggled = ui
                .button(RichText::new(if silent { "🔇" } else { "🔊" }).size(16.0))
                .on_hover_text(if self.effects.muted {
                    "Unmute effects"
                } else {
                    "Mute effects"
                })
                .clicked();
            if toggled {
                self.effects
                    .set_level(self.effects.volume, !self.effects.muted);
            }
            if released || toggled {
                self.settings.effects_volume = Some(self.effects.volume);
                self.settings.effects_muted = self.effects.muted;
                self.persist_settings();
                // Let the new level be heard.
                let now = ui.input(|input| input.time);
                self.effects.play(Effect::Snap, now);
            }
        });
    }
}
