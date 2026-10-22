use eframe::egui::{
    self, Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
    epaint::{Mesh, Vertex},
};

use futures_util::{SinkExt, StreamExt};

use pieceoff_core::{
    ClientMessage, Piece, PlaybackState, RepeatMode, RoomSnapshot, ServerMessage, TrayArrangement,
    piece_outline, piece_tray_layout, valid_media_dimensions, valid_media_extension,
};

use serde::{Deserialize, Serialize};

use sha2::{Digest, Sha256};

use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

#[cfg(test)]
use std::{io::BufRead, net::TcpStream};

use tokio::{runtime::Builder, sync::mpsc as tokio_mpsc};

use tokio_tungstenite::{connect_async, tungstenite::Message};

mod audio;
mod board;
mod chat;
mod clip;
mod controls;
mod cursors;
mod decoder;
#[cfg(test)]
mod e2e;
mod holders;
mod library;
mod library_ui;
mod media_tools;
mod net;
mod package;
mod playback;
mod probe;
mod queue_ui;
mod server_addr;
mod sfx;
mod storage;
mod timelapse;
mod transport;
mod ui;

use audio::*;
use board::*;
use chat::*;
use controls::*;
use cursors::*;
use decoder::*;
use holders::*;
use library::{Library, LibrarySort};
use net::*;
use package::*;
use playback::*;
use probe::*;
use sfx::*;
use storage::*;
use timelapse::*;
use transport::*;
use ui::*;

fn main() -> eframe::Result {
    // winit's client-side Wayland frames fail to resize or maximize on
    // GNOME. XWayland gets compositor-drawn frames that behave, so prefer it
    // when available. Set PIECEOFF_WAYLAND=1 to opt back into native Wayland.
    if std::env::var_os("PIECEOFF_WAYLAND").is_none()
        && std::env::var_os("DISPLAY").is_some()
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PieceOff")
            .with_app_id("pieceoff")
            .with_resizable(true)
            .with_maximize_button(true)
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PieceOff",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(PieceOffApp::default()))
        }),
    )
}

struct PieceOffApp {
    /// What the server field shows; connections use its normalised form.
    server_url: String,
    /// Where the starting `server_url` came from, shown on hover.
    server_url_source: &'static str,
    /// The server field was typed in since it was last committed.
    server_edit_dirty: bool,
    player_name: String,
    invite_code: String,
    recent_rooms: Vec<RecentRoom>,
    recent_rooms_path: PathBuf,
    recent_room_selection: Option<usize>,
    title: String,
    media_path: Option<PathBuf>,
    probe_rx: Option<mpsc::Receiver<ProbeResult>>,
    repaint: Option<egui::Context>,
    audio_launcher: Option<AudioLauncher>,
    autoplay_room: Option<String>,
    win_dismissed_room: Option<String>,
    confirm_restart: bool,
    startup_media: Option<PathBuf>,
    clip_plan: Option<clip::ClipPlan>,
    clip_source_title: Option<String>,
    media_hash: String,
    media_extension: String,
    /// Every queue item's local file, by SHA-256: our own picks and verified
    /// downloads. Shared with the network thread for uploads.
    media_files: Arc<Mutex<HashMap<String, PathBuf>>>,
    item_download_tx: mpsc::Sender<(String, Result<PathBuf, String>)>,
    item_download_rx: mpsc::Receiver<(String, Result<PathBuf, String>)>,
    downloads_in_flight: HashSet<String>,
    /// A file being probed and hashed for `QueueAdd` on a worker thread.
    queue_probe_rx: Option<mpsc::Receiver<(PathBuf, Result<MediaMetadata, String>)>>,
    /// Set while the decoder is parked at the end of the item, waiting for
    /// the server to advance the queue.
    holding_at_end: bool,
    media_ready: bool,
    media_uploading: bool,
    media_size: (u32, u32),
    media_duration_ms: u64,
    media_has_audio: bool,
    video_position_ms: u64,
    loop_start_ms: u64,
    loop_end_ms: u64,
    playback_rate: f32,
    audio_volume: u8,
    audio_muted: bool,
    seeking: bool,
    is_video: bool,
    texture: Option<TextureHandle>,
    last_video_frame: Option<ColorImage>,
    still_assist_texture: Option<TextureHandle>,
    decoder_frames: Option<mpsc::Receiver<DecodedFrame>>,
    decoder_errors: Option<mpsc::Receiver<String>>,
    decoder_playing: Option<Arc<AtomicBool>>,
    decoder_worker_alive: Option<Arc<AtomicBool>>,
    decoder_control: Option<mpsc::Sender<DecoderCommand>>,
    decoder_ffmpeg_program: OsString,
    decoder_restarts_after_error: u8,
    decoder_recovering: bool,
    audio_control: Option<mpsc::Sender<AudioCommand>>,
    audio_errors: Option<mpsc::Receiver<String>>,
    network: Option<Network>,
    snapshot: Option<RoomSnapshot>,
    pending_snapshot: Option<RoomSnapshot>,
    shape_cache_room: String,
    shape_cache: std::collections::HashMap<u32, CachedPieceShape>,
    client_id: Option<String>,
    connected: bool,
    status: String,
    rows: u16,
    columns: u16,
    dragging: Option<ActiveDrag>,
    selected_pieces: std::collections::HashSet<u32>,
    piece_filter: PieceFilter,
    show_reference: bool,
    show_queue: bool,
    settings: Settings,
    reconnect_at: Option<Instant>,
    reconnect_attempt: u8,
    pan_drag_last: Option<Pos2>,
    /// A selection box being dragged on empty space: (start, current) in
    /// screen coordinates.
    marquee: Option<(Pos2, Pos2)>,
    board_zoom: f32,
    board_pan: Vec2,
    view_command: Option<ViewCommand>,
    last_playback_send: Option<PlaybackState>,
    last_drift_sync: Instant,
    /// Other players currently in the room, by display name.
    peers: Vec<String>,
    /// Others watching the room, by display name.
    spectators: Vec<String>,
    /// Join rooms to watch rather than play (the join screen's checkbox).
    watch_only: bool,
    /// The server admitted us as a spectator: we see the room but cannot
    /// change it.
    spectating: bool,
    /// A timelapse was asked for and has not arrived yet.
    timelapse_requested: bool,
    /// The solve timelapse being replayed over the board, if any.
    replay: Option<Replay>,
    /// Other players' pointers, and what we last sent of our own.
    cursors: LiveCursors,
    /// Room chat and board pings.
    chat: RoomChat,
    /// Piece holders: the gesture in progress and the menu's label edits.
    holders: HolderTool,
    /// Snap, join, finish, chat and ping sounds, separate from the clip's audio.
    effects: SoundEffects,
    help_open: bool,
    /// Pieces already seen in place, so newly placed ones can flash.
    placed_seen: std::collections::HashSet<u32>,
    snap_flashes: Vec<(u32, f64)>,
    /// The current run's finish has been seen (and, for a library puzzle,
    /// recorded), so it is not counted twice.
    solve_recorded: bool,
    /// The status text last shown as a toast, and when it appeared.
    toast: (String, f64),
    toast_dismissed: bool,
    /// Every status message this session, oldest first.
    messages: std::collections::VecDeque<LogEntry>,
    messages_open: bool,
    /// Problems logged since the message list was last open.
    unseen_problems: usize,
    /// A join by code echoes our own arrival; skip it in the player list.
    own_join_pending: bool,
    /// Listener's audio delay in milliseconds, read live by the audio worker.
    audio_offset_ms: Arc<std::sync::atomic::AtomicI32>,
    timeline_drag: Option<TimelineDrag>,
    library: Library,
    /// Why the library index could not be read, if it could not.
    library_notice: Option<String>,
    library_search: String,
    library_sort: LibrarySort,
    library_jobs: Vec<library::LibraryJob>,
    library_thumbs: std::collections::HashMap<String, Option<TextureHandle>>,
    library_delete_pending: Option<String>,
    /// The library entry this room was resumed from; leaving saves progress to it.
    library_entry_id: Option<String>,
    controls: Controls,
    controls_open: bool,
    /// The action waiting for its new key in the Controls window.
    rebinding: Option<Action>,
    controls_notice: Option<String>,
    /// A pan button (middle or right) is held down over the board.
    button_panning: bool,
    /// Egui time of the last drag position sent to the server.
    last_move_sent_at: f64,
    /// The server said the room code is unknown while we were reconnecting:
    /// the room expired or the server lost it, so retrying cannot help.
    room_gone: bool,
    /// When the video decoder was first seen stopped, for automatic retries.
    decoder_dead_since: Option<Instant>,
    /// The "could not restart" message has been shown for this stoppage.
    decoder_gave_up_reported: bool,
    /// What was found for FFmpeg and FFprobe; filled in by a worker thread.
    tools: Vec<media_tools::ToolReport>,
    tools_rx: Option<mpsc::Receiver<Vec<media_tools::ToolReport>>>,
}

impl Default for PieceOffApp {
    fn default() -> Self {
        let (item_download_tx, item_download_rx) = mpsc::channel();
        let recent_rooms_path = recent_rooms_path();
        let recent_rooms = load_recent_rooms(&recent_rooms_path).unwrap_or_default();
        // Tests must not depend on this device's saved preferences.
        let mut settings = if cfg!(test) {
            Settings::default()
        } else {
            load_settings(&settings_path())
        };
        settings.mouse = settings.mouse.clone().sanitized();
        media_tools::set_chosen_directory(settings.ffmpeg_dir.clone().map(PathBuf::from));
        let audio_offset_ms = Arc::new(std::sync::atomic::AtomicI32::new(
            settings.audio_offset_ms.clamp(-500, 500),
        ));
        // Tests must not touch (or share) the real library.
        let library_dir = if cfg!(test) {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            std::env::temp_dir().join(format!(
                "pieceoff-test-library-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
        } else {
            library::library_dir()
        };
        let (library, library_notice) = Library::load(library_dir);
        let controls = Controls::from_settings(&settings.keybindings);
        let effects = SoundEffects::new(settings.effects_volume, settings.effects_muted);
        let (server_url, server_url_source) = server_addr::initial_server_url(
            settings.server_url.as_deref(),
            std::env::var("PIECEOFF_SERVER_URL").ok().as_deref(),
        );
        Self {
            server_url,
            server_url_source,
            server_edit_dirty: false,
            player_name: settings
                .player_name
                .clone()
                .unwrap_or_else(|| "Player".into()),
            invite_code: String::new(),
            recent_rooms,
            recent_rooms_path,
            recent_room_selection: None,
            title: "Untitled puzzle".into(),
            media_path: None,
            probe_rx: None,
            repaint: None,
            audio_launcher: None,
            autoplay_room: None,
            win_dismissed_room: None,
            confirm_restart: false,
            startup_media: std::env::args_os()
                .nth(1)
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("PIECEOFF_MEDIA").map(PathBuf::from)),
            clip_plan: None,
            clip_source_title: None,
            media_hash: String::new(),
            media_extension: String::new(),
            media_ready: false,
            media_uploading: false,
            media_size: (0, 0),
            media_duration_ms: 0,
            media_has_audio: false,
            video_position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
            playback_rate: 1.0,
            audio_volume: settings.audio_volume.unwrap_or(80).min(100),
            audio_muted: settings.audio_muted,
            seeking: false,
            is_video: false,
            texture: None,
            last_video_frame: None,
            still_assist_texture: None,
            decoder_frames: None,
            decoder_errors: None,
            decoder_playing: None,
            decoder_worker_alive: None,
            decoder_control: None,
            decoder_ffmpeg_program: media_tools::ffmpeg(),
            decoder_restarts_after_error: 0,
            decoder_recovering: false,
            audio_control: None,
            audio_errors: None,
            network: None,
            media_files: Arc::new(Mutex::new(HashMap::new())),
            item_download_tx,
            item_download_rx,
            downloads_in_flight: HashSet::new(),
            queue_probe_rx: None,
            holding_at_end: false,
            snapshot: None,
            pending_snapshot: None,
            shape_cache_room: String::new(),
            shape_cache: std::collections::HashMap::new(),
            client_id: None,
            connected: false,
            status: "Choose an image or video, then create a room or join by code.".into(),
            rows: 3,
            columns: 4,
            dragging: None,
            selected_pieces: std::collections::HashSet::new(),
            piece_filter: PieceFilter::All,
            show_reference: false,
            show_queue: false,
            settings,
            reconnect_at: None,
            reconnect_attempt: 0,
            pan_drag_last: None,
            marquee: None,
            board_zoom: 1.0,
            board_pan: Vec2::ZERO,
            view_command: None,
            last_playback_send: None,
            last_drift_sync: Instant::now(),
            peers: Vec::new(),
            spectators: Vec::new(),
            watch_only: false,
            spectating: false,
            timelapse_requested: false,
            replay: None,
            cursors: LiveCursors::default(),
            chat: RoomChat::default(),
            holders: HolderTool::default(),
            effects,
            help_open: false,
            placed_seen: std::collections::HashSet::new(),
            snap_flashes: Vec::new(),
            solve_recorded: false,
            toast: (String::new(), f64::NEG_INFINITY),
            toast_dismissed: false,
            messages: std::collections::VecDeque::new(),
            messages_open: false,
            unseen_problems: 0,
            own_join_pending: false,
            audio_offset_ms,
            timeline_drag: None,
            library,
            library_notice,
            library_search: String::new(),
            library_sort: LibrarySort::default(),
            library_jobs: Vec::new(),
            library_thumbs: std::collections::HashMap::new(),
            library_delete_pending: None,
            library_entry_id: None,
            controls,
            controls_open: false,
            rebinding: None,
            controls_notice: None,
            button_panning: false,
            last_move_sent_at: f64::NEG_INFINITY,
            room_gone: false,
            decoder_dead_since: None,
            decoder_gave_up_reported: false,
            tools: Vec::new(),
            tools_rx: None,
        }
    }
}

impl PieceOffApp {
    fn select_recent_room(&mut self, index: usize) {
        let Some(recent) = self.recent_rooms.get(index).cloned() else {
            return;
        };
        self.server_url.clone_from(&recent.server_url);
        self.server_edit_dirty = false;
        self.invite_code.clone_from(&recent.invite_code);
        self.player_name.clone_from(&recent.player_name);
        self.title.clone_from(&recent.title);
        self.recent_room_selection = Some(index);
        self.status = format!("Ready to rejoin {} ({}).", recent.title, recent.invite_code);
    }

    fn forget_selected_recent_room(&mut self) {
        let Some(index) = self.recent_room_selection else {
            return;
        };
        match forget_recent_room(&self.recent_rooms_path, &mut self.recent_rooms, index) {
            Ok(()) => {
                self.recent_room_selection = None;
                self.status = "Removed room from recent rooms.".into();
            }
            Err(error) => self.status = format!("Could not remove recent room: {error}"),
        }
    }

    /// Write preferences to disk. Tests never touch the real settings file.
    /// The address to connect to, or why the field can't be used.
    fn resolved_server_url(&self) -> Result<String, String> {
        server_addr::normalize_server_url(&self.server_url)
    }

    /// Save the server field as this device's default server. An unusable
    /// address is reported and left in the field for the player to fix.
    fn commit_server_url(&mut self) {
        self.server_edit_dirty = false;
        match self.resolved_server_url() {
            Ok(url) => {
                self.server_url.clone_from(&url);
                if self.settings.server_url.as_deref() != Some(url.as_str()) {
                    self.settings.server_url = Some(url);
                    self.persist_settings();
                }
                self.server_url_source = "saved setting";
            }
            Err(message) => self.status = message,
        }
    }

    fn commit_server_url_if_edited(&mut self) {
        if self.server_edit_dirty {
            self.commit_server_url();
        }
    }

    /// Forget the saved server and go back to the local one.
    fn reset_server_url(&mut self) {
        self.server_url = server_addr::DEFAULT_SERVER_URL.into();
        self.server_url_source = "built-in default";
        self.server_edit_dirty = false;
        if self.settings.server_url.take().is_some() {
            self.persist_settings();
        }
    }

    fn persist_settings(&self) {
        if !cfg!(test) {
            save_settings(&settings_path(), &self.settings);
        }
    }

    /// Change how much detail the video decoder produces, restarting it at
    /// the shared position so playback carries on.
    fn set_video_quality(&mut self, quality: VideoQuality, ctx: &egui::Context) {
        if self.settings.video_quality == quality {
            return;
        }
        self.settings.video_quality = quality;
        self.persist_settings();
        let Some(path) = self.media_path.clone() else {
            return;
        };
        if !self.is_video || self.media_extension == "webp" {
            return;
        }
        let audio_enabled = self.media_has_audio && self.audio_usable();
        self.start_video_decoder_using(
            path,
            audio_enabled,
            ctx.clone(),
            self.decoder_ffmpeg_program.clone(),
        );
        self.sync_decoder_playback(ctx);
        self.status = format!("Video quality: {}.", quality.label());
    }

    fn clear_active_gesture(&mut self) {
        self.dragging = None;
        self.pan_drag_last = None;
        self.marquee = None;
        self.holders.drag = None;
    }

    fn choose_media(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images and video",
                &[
                    "png", "apng", "jpg", "jpeg", "webp", "gif", "mp4", "m4v", "mov", "mkv",
                    "webm", "avi", "mpg", "mpeg", "ogv", "flv", "3gp", "wmv", "ts", "mts", "m2ts",
                ],
            )
            .pick_file()
        else {
            return;
        };
        self.begin_probe(path, false, ctx);
    }

    /// Inspect (and hash) a file on a worker thread so a multi-GB pick never
    /// freezes the window.
    fn begin_probe(&mut self, path: PathBuf, skip_setup: bool, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let worker_path = path.clone();
        self.status = format!("Reading {}…", file_label(&path));
        self.clip_plan = None;
        thread::spawn(move || {
            let outcome = probe_media(&worker_path, skip_setup);
            let _ = tx.send(ProbeResult {
                path: worker_path,
                outcome,
            });
            repaint.request_repaint();
        });
        self.probe_rx = Some(rx);
    }

    fn poll_probe(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.probe_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.probe_rx = None;
        match result.outcome {
            Ok(ProbeOutcome::Ready(metadata)) => {
                if let Err(error) = self.apply_media(result.path, metadata, ctx) {
                    self.status = error;
                }
            }
            Ok(ProbeOutcome::ChooseSetup {
                duration_ms,
                has_audio,
                size,
            }) => {
                let plan = clip::ClipPlan::new(result.path, duration_ms, has_audio, size);
                self.clip_source_title = Some(plan.source_stem());
                self.status = "Choose how to play this video.".into();
                self.clip_plan = Some(plan);
            }
            Err(error) => self.status = error,
        }
    }

    /// Show the room's current queue item: reuse the local file if we have
    /// it, otherwise wait for the download `fetch_missing_items` started.
    pub(crate) fn switch_to_current_item(&mut self, ctx: &egui::Context) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(item) = snapshot.current_item() else {
            return;
        };
        if self.media_hash == item.sha256 && self.media_path.is_some() {
            return;
        }
        let room_title = snapshot.title.clone();
        let path = self.media_files.lock().unwrap().get(&item.sha256).cloned();
        let Some(path) = path else {
            self.texture = None;
            self.last_video_frame = None;
            self.still_assist_texture = None;
            self.media_ready = false;
            self.status = "Waiting for the next video to download…".into();
            return;
        };
        // Cached files were hashed when they arrived; trust that rather than
        // reading a multi-gigabyte file again on the UI thread.
        let known_hash = item.sha256.clone();
        let loaded = inspect_media_basic(&path).and_then(|mut metadata| {
            metadata.sha256 = known_hash;
            self.apply_media(path, metadata, ctx)
        });
        match loaded {
            Ok(()) => {
                self.title = room_title;
                self.media_ready = true;
                self.sync_decoder_playback(ctx);
            }
            Err(error) => self.status = format!("Could not load the video: {error}"),
        }
    }

    fn load_media(&mut self, path: PathBuf, ctx: &egui::Context) -> Result<(), String> {
        let metadata = inspect_media(&path)?;
        self.apply_media(path, metadata, ctx)
    }

    fn apply_media(
        &mut self,
        path: PathBuf,
        metadata: MediaMetadata,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let extension = supported_media_extension(&path)?;
        let audio_enabled = metadata.has_audio && !audio_disabled();
        self.title = self.clip_source_title.take().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("Untitled puzzle")
                .to_owned()
        });
        self.media_size = metadata.size;
        self.media_duration_ms = metadata.duration_ms;
        self.media_has_audio = metadata.has_audio;
        self.video_position_ms = 0;
        self.loop_start_ms = 0;
        self.loop_end_ms = metadata.duration_ms;
        self.playback_rate = 1.0;
        self.is_video = metadata.is_video;
        self.media_hash = metadata.sha256;
        self.media_files
            .lock()
            .unwrap()
            .insert(self.media_hash.clone(), path.clone());
        self.media_extension = extension;
        self.media_path = Some(path.clone());
        self.pending_snapshot = None;
        self.library_entry_id = None;
        self.texture = None;
        self.last_video_frame = None;
        self.still_assist_texture = None;
        self.decoder_frames = None;
        self.decoder_errors = None;
        self.decoder_playing = None;
        self.decoder_worker_alive = None;
        self.decoder_control = None;
        self.decoder_ffmpeg_program = media_tools::ffmpeg();
        self.decoder_restarts_after_error = 0;
        self.decoder_recovering = false;
        self.audio_control = None;
        self.audio_errors = None;
        if !metadata.is_video {
            match image::open(&path) {
                Ok(image) => {
                    self.texture = Some(ctx.load_texture(
                        "puzzle-media",
                        image_to_color(image),
                        TextureOptions::LINEAR,
                    ))
                }
                Err(error) => self.status = format!("Could not load image: {error}"),
            }
        } else if self.media_extension == "webp" {
            self.start_animated_webp_decoder(path, ctx.clone());
        } else {
            self.start_video_decoder(path, audio_enabled, ctx.clone());
        }
        if metadata.is_video && metadata.has_audio && !audio_enabled {
            self.status = "Video selected. Audio is turned off for this run.".into();
        } else {
            self.status = format!(
                "Selected {} ({}×{}).",
                file_label(self.media_path.as_ref().unwrap()),
                metadata.size.0,
                metadata.size.1
            );
        }
        self.media_ready = false;
        Ok(())
    }

    fn start_video_decoder(&mut self, path: PathBuf, audio_enabled: bool, repaint: egui::Context) {
        self.start_video_decoder_using(path, audio_enabled, repaint, media_tools::ffmpeg());
    }

    fn start_animated_webp_decoder(&mut self, path: PathBuf, repaint: egui::Context) {
        let (tx, rx) = mpsc::sync_channel(1);
        let (error_tx, error_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let playing = Arc::new(AtomicBool::new(false));
        let running = playing.clone();
        let worker_alive = Arc::new(AtomicBool::new(true));
        let (decode_width, decode_height) =
            video_decode_dimensions(self.media_size.0, self.media_size.1);
        let duration_ms = self.media_duration_ms;
        let _ = AnimatedWebpDecoderBackend.start(DecoderStartup {
            path,
            duration_ms,
            width: decode_width as usize,
            height: decode_height as usize,
            fps: DEFAULT_VIDEO_FPS,
            control_rx,
            frames: tx,
            errors: error_tx,
            repaint,
            running,
            worker_alive: worker_alive.clone(),
        });
        self.decoder_frames = Some(rx);
        self.decoder_errors = Some(error_rx);
        self.decoder_playing = Some(playing);
        self.decoder_worker_alive = Some(worker_alive);
        self.decoder_control = Some(control_tx);
        self.audio_control = None;
        self.audio_errors = None;
    }

    fn start_video_decoder_using(
        &mut self,
        path: PathBuf,
        audio_enabled: bool,
        repaint: egui::Context,
        ffmpeg_program: OsString,
    ) {
        let (tx, rx) = mpsc::sync_channel(1);
        let (decoder_error_tx, decoder_error_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel::<DecoderCommand>();
        let playing = Arc::new(AtomicBool::new(false));
        let running = playing.clone();
        let worker_alive = Arc::new(AtomicBool::new(true));
        self.decoder_ffmpeg_program.clone_from(&ffmpeg_program);
        if audio_enabled {
            let (audio_tx, audio_rx) = mpsc::channel::<AudioCommand>();
            let (error_tx, error_rx) = mpsc::channel();
            let audio_path = path.clone();
            let launcher = self.audio_launcher.clone();
            let offset = self.audio_offset_ms.clone();
            thread::spawn(move || {
                run_audio_player(audio_path, audio_rx, error_tx, launcher, offset)
            });
            self.audio_control = Some(audio_tx);
            self.audio_errors = Some(error_rx);
        } else {
            self.audio_control = None;
            self.audio_errors = None;
        }
        let (max_size, fps) = self.settings.video_quality.limits();
        let (width, height) = fit_video_dimensions(self.media_size.0, self.media_size.1, max_size);
        let width = width as usize;
        let height = height as usize;
        let _ = FfmpegDecoderBackend::new(ffmpeg_program).start(DecoderStartup {
            path,
            duration_ms: self.media_duration_ms,
            width,
            height,
            fps,
            control_rx,
            frames: tx,
            errors: decoder_error_tx,
            repaint,
            running,
            worker_alive: worker_alive.clone(),
        });
        self.decoder_frames = Some(rx);
        self.decoder_errors = Some(decoder_error_rx);
        self.decoder_playing = Some(playing);
        self.decoder_worker_alive = Some(worker_alive);
        self.decoder_control = Some(control_tx);
    }

    fn create_room(&mut self) {
        if !self.require_media() {
            return;
        }
        self.reconnect_at = None;
        self.reconnect_attempt = 0;
        let title = clean_room_title(&self.title);
        let message = ClientMessage::CreateRoom {
            name: self.player_name.clone(),
            title: title.clone(),
            media_sha256: self.media_hash.clone(),
            media_extension: self.media_extension.clone(),
            // A reopened package keeps its original board shape even when the
            // item on screen has another aspect ratio.
            width: self
                .pending_snapshot
                .as_ref()
                .map_or(self.media_size.0, |snapshot| snapshot.width),
            height: self
                .pending_snapshot
                .as_ref()
                .map_or(self.media_size.1, |snapshot| snapshot.height),
            rows: self.rows,
            columns: self.columns,
            initial_snapshot: self.pending_snapshot.clone().map(|mut snapshot| {
                snapshot.title = title;
                Box::new(snapshot)
            }),
            duration_ms: self.media_duration_ms,
        };
        self.media_ready = false;
        self.media_uploading = true;
        self.connect(message);
    }

    fn open_package(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PieceOff puzzle package", &["pieceoff"])
            .pick_file()
        else {
            return;
        };
        self.open_package_at(&path, ctx);
    }

    fn open_package_at(&mut self, path: &Path, ctx: &egui::Context) {
        match read_puzzle_package_with_extras(path) {
            Ok((mut snapshot, media_path, extras)) => match self.load_media(media_path, ctx) {
                Ok(()) => {
                    self.media_files.lock().unwrap().extend(extras);
                    // The board keeps the first video's shape; the item on
                    // screen is the package's current one.
                    let (expected_width, expected_height) = snapshot
                        .current_item()
                        .map_or((snapshot.width, snapshot.height), |item| {
                            (item.width, item.height)
                        });
                    if expected_width != self.media_size.0 || expected_height != self.media_size.1 {
                        self.status = "Package puzzle dimensions do not match its media.".into();
                        return;
                    }
                    if self.media_duration_ms > 0 {
                        snapshot.playback.loop_start_ms =
                            snapshot.playback.loop_start_ms.min(self.media_duration_ms);
                        snapshot.playback.loop_end_ms =
                            snapshot.playback.loop_end_ms.min(self.media_duration_ms);
                        if snapshot.playback.loop_end_ms <= snapshot.playback.loop_start_ms {
                            snapshot.playback.loop_start_ms = 0;
                            snapshot.playback.loop_end_ms = self.media_duration_ms;
                        }
                        let loop_span = snapshot
                            .playback
                            .loop_end_ms
                            .saturating_sub(snapshot.playback.loop_start_ms);
                        snapshot.playback.position_ms = snapshot.playback.loop_start_ms
                            + snapshot
                                .playback
                                .position_ms
                                .saturating_sub(snapshot.playback.loop_start_ms)
                                % loop_span;
                    }
                    self.title.clone_from(&snapshot.title);
                    self.rows = snapshot.rows;
                    self.columns = snapshot.columns;
                    self.pending_snapshot = Some(snapshot);
                    self.status =
                        "Package loaded. Create a room to share this puzzle and its progress."
                            .into();
                }
                Err(error) => self.status = error,
            },
            Err(error) => self.status = format!("Could not open puzzle package: {error}"),
        }
    }

    fn save_package(&mut self) {
        let (Some(snapshot), Some(media_path)) = (&self.snapshot, &self.media_path) else {
            self.status = "Join a room with its media loaded before saving a package.".into();
            return;
        };
        let default_name = format!("{}.pieceoff", snapshot.title);
        let Some(destination) = rfd::FileDialog::new()
            .add_filter("PieceOff puzzle package", &["pieceoff"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        let mut paused_snapshot = snapshot.clone();
        paused_snapshot.playback.position_ms = estimated_position_ms(&snapshot.playback);
        paused_snapshot.playback.playing = false;
        paused_snapshot.playback.changed_at_ms = 0;
        let extras: Vec<(String, PathBuf)> = {
            let files = self.media_files.lock().unwrap();
            snapshot
                .queue
                .iter()
                .filter(|item| item.sha256 != snapshot.media_sha256)
                .filter_map(|item| {
                    files
                        .get(&item.sha256)
                        .map(|path| (item.sha256.clone(), path.clone()))
                })
                .collect()
        };
        match write_puzzle_package_with_extras(&destination, media_path, &paused_snapshot, &extras)
        {
            Ok(()) => {
                self.status = format!(
                    "Saved portable puzzle package to {}.",
                    destination.display()
                )
            }
            Err(error) => self.status = format!("Could not save puzzle package: {error}"),
        }
    }

    fn join_room(&mut self) {
        self.reconnect_at = None;
        self.reconnect_attempt = 0;
        if self.network.is_some() {
            self.status = "Already connected to a room.".into();
            return;
        }
        if self.invite_code.trim().is_empty() {
            self.status = "Enter a room code.".into();
            return;
        }
        let message = ClientMessage::JoinRoom {
            invite_code: self.invite_code.clone(),
            name: self.player_name.clone(),
            media_sha256: self.media_hash.clone(),
            media_extension: self.media_extension.clone(),
            spectator: self.watch_only,
        };
        self.connect(message);
    }

    fn require_media(&mut self) -> bool {
        if self.media_path.is_none() {
            self.status = "Choose an image or video first.".into();
            false
        } else if self.network.is_some() {
            self.status = "Already connected to a room.".into();
            false
        } else {
            true
        }
    }

    fn poll_decoder_errors(&mut self) {
        if let Some(errors) = &self.decoder_errors {
            for error in errors.try_iter() {
                self.status = error;
                self.decoder_recovering = false;
            }
        }
    }

    /// Show the newest decoded frame; the first one after a restart clears
    /// the recovery status.
    fn drain_decoder_frames(&mut self, ctx: &egui::Context) {
        let Some(frames) = &self.decoder_frames else {
            return;
        };
        let latest = frames.try_iter().last();
        if let Some(frame) = latest {
            self.note_decoder_frame_received();
            let image =
                ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
            self.last_video_frame = Some(image.clone());
            if let Some(texture) = &mut self.texture {
                texture.set(image, TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("puzzle-video", image, TextureOptions::LINEAR));
            }
            ctx.request_repaint();
        }
    }

    fn note_decoder_frame_received(&mut self) {
        self.decoder_restarts_after_error = 0;
        self.decoder_gave_up_reported = false;
        if self.decoder_recovering {
            self.decoder_recovering = false;
            self.status = "Video playback recovered.".into();
        }
    }

    /// Host: scatter every piece back into the tray for a fresh game.
    fn restart_puzzle(&mut self) {
        self.selected_pieces.clear();
        self.dragging = None;
        self.win_dismissed_room = None;
        self.confirm_restart = false;
        self.send(ClientMessage::RestartPuzzle);
        self.status = "Puzzle restarted. Good luck!".into();
    }

    /// Look for FFmpeg and FFprobe on a worker thread (running them to read
    /// their versions can take a moment).
    fn refresh_tools(&mut self) {
        let (tx, rx) = mpsc::channel();
        let repaint = self.repaint.clone();
        thread::spawn(move || {
            let reports = vec![
                media_tools::report(media_tools::Tool::Ffmpeg),
                media_tools::report(media_tools::Tool::Ffprobe),
            ];
            let _ = tx.send(reports);
            if let Some(repaint) = repaint {
                repaint.request_repaint();
            }
        });
        self.tools_rx = Some(rx);
    }

    fn poll_tools(&mut self) {
        if let Some(rx) = &self.tools_rx {
            if let Ok(reports) = rx.try_recv() {
                self.tools = reports;
                self.tools_rx = None;
            }
        }
    }

    /// Use the FFmpeg in `dir` (or the default search with `None`).
    fn choose_tools_directory(&mut self, dir: Option<PathBuf>) {
        self.settings.ffmpeg_dir = dir.as_ref().map(|dir| dir.to_string_lossy().into_owned());
        media_tools::set_chosen_directory(dir);
        self.persist_settings();
        self.decoder_ffmpeg_program = media_tools::ffmpeg();
        self.refresh_tools();
    }

    /// Drop the chosen media and return to the library shelf.
    fn back_to_library(&mut self) {
        if let Some(decoder) = &self.decoder_playing {
            decoder.store(false, Ordering::Relaxed);
        }
        self.media_path = None;
        self.media_hash.clear();
        self.media_extension.clear();
        self.media_ready = false;
        self.is_video = false;
        self.texture = None;
        self.last_video_frame = None;
        self.still_assist_texture = None;
        self.decoder_frames = None;
        self.decoder_errors = None;
        self.decoder_playing = None;
        self.decoder_worker_alive = None;
        self.decoder_control = None;
        self.audio_control = None;
        self.audio_errors = None;
        self.pending_snapshot = None;
        self.library_entry_id = None;
        self.clip_source_title = None;
        self.status = "Choose an image or video, then create a room or join by code.".into();
    }

    fn is_host(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| self.client_id.as_deref() == Some(snapshot.host_id.as_str()))
    }

    /// Lay the loose pieces back into the tray. Open to every player; the
    /// server leaves assembled and held pieces alone.
    fn arrange_tray(&mut self, arrangement: TrayArrangement) {
        if self.snapshot.is_none() || self.dragging.is_some() {
            return;
        }
        self.selected_pieces.clear();
        self.send(ClientMessage::RearrangeTray { arrangement });
        self.status = match arrangement {
            TrayArrangement::EdgesFirst => "Loose pieces laid out: corners, edges, then the rest.",
            TrayArrangement::Shuffle => "Loose pieces shuffled.",
        }
        .into();
    }

    fn leave_room(&mut self) {
        let playback = self.snapshot.as_ref().map(|snapshot| &snapshot.playback);
        let position_ms = playback
            .map(estimated_position_ms)
            .unwrap_or(self.video_position_ms);
        let loop_start_ms = playback.map_or(self.loop_start_ms, |state| state.loop_start_ms);
        let loop_end_ms = playback.map_or(self.loop_end_ms, |state| state.loop_end_ms);
        if let Some(control) = &self.decoder_control {
            let _ = control.send(DecoderCommand {
                playing: false,
                position_ms,
                rate: self.playback_rate,
                loop_start_ms,
                loop_end_ms,
                resync: false,
                issued_at: Instant::now(),
            });
        }
        if let Some(control) = &self.audio_control {
            let _ = control.send(AudioCommand {
                playing: false,
                position_ms,
                rate: self.playback_rate,
                loop_start_ms,
                loop_end_ms,
                volume: self.audio_volume,
                muted: self.audio_muted,
                resync: false,
                issued_at: Instant::now(),
            });
        }
        if self.pending_snapshot.is_some() {
            self.pending_snapshot = self.snapshot.clone();
        }
        // A room resumed from the library keeps its progress there.
        if self.library_entry_id.is_some() && self.media_ready {
            self.save_to_library();
        }
        self.network = None;
        self.snapshot = None;
        self.peers.clear();
        self.spectators.clear();
        self.spectating = false;
        self.timelapse_requested = false;
        self.replay = None;
        self.cursors.reset();
        self.chat.reset();
        self.media_ready = false;
        self.media_uploading = false;
        self.client_id = None;
        self.connected = false;
        self.clear_active_gesture();
        self.selected_pieces.clear();
        self.show_reference = false;
        self.reconnect_at = None;
        self.reconnect_attempt = 0;
        self.room_gone = false;
        self.decoder_restarts_after_error = 0;
        self.decoder_recovering = false;
        if let Some(decoder) = &self.decoder_playing {
            decoder.store(false, Ordering::Relaxed);
        }
        self.status = "Left the room. Choose media or join another room.".into();
    }
}

/// A path's file name, for messages that should not spell out directories.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn clean_room_title(title: &str) -> String {
    let title: String = title
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect();
    if title.trim().is_empty() {
        "Untitled puzzle".into()
    } else {
        title.trim().to_owned()
    }
}

const MAX_PUZZLE_PIECES: u16 = 400;

fn max_grid_dimension_for_other(other_dimension: u16) -> u16 {
    (MAX_PUZZLE_PIECES / other_dimension.max(1)).clamp(2, 30)
}

#[cfg(test)]
mod tests;
