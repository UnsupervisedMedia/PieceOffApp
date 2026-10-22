//! End-to-end checks of the first product slice, multiplayer looping video.
//!
//! Each test runs a real room server in this process and two or three real
//! app instances (`PieceOffApp::tick`, the same frame function the window
//! calls), with real FFmpeg decoding a generated, silent video. Players talk to
//! the server over real WebSockets and HTTP, so uploads, downloads, reconnects,
//! and server restarts are the production paths. Nothing here plays audio.
//!
//! What these cannot show: that two people with speakers hear the same moment,
//! and how any of it feels. Those need people (see docs/TESTING.md).

use super::*;
use pieceoff_server::{RunningServer, config::Config};
use std::hash::{Hash, Hasher};

const STEP: Duration = Duration::from_millis(4);

fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::package::transfer_test_lock()
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pieceoff-e2e-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A short silent test video (never a tone: see docs/TESTING.md).
fn make_video(dir: &Path, name: &str, filter: &str, seconds: u32) -> PathBuf {
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", filter])
        .args(["-t", &seconds.to_string(), "-an", "-c:v", "mpeg4"])
        .args(["-q:v", "3", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg could not make the test video");
    path
}

fn small_video(dir: &Path) -> PathBuf {
    make_video(dir, "loop.mp4", "testsrc2=size=320x180:rate=24", 4)
}

struct Player {
    name: String,
    app: PieceOffApp,
    ctx: egui::Context,
    time: f64,
    /// Distinct decoded frames seen since the counter was last cleared.
    frame_changes: u32,
    last_fingerprint: Option<u64>,
}

impl Player {
    fn new(name: &str, url: &str, root: &Path) -> Self {
        let ctx = egui::Context::default();
        // The default would take a command-line argument (here a test
        // filter) as a file to open, so `startup_media` is cleared.
        let mut app = PieceOffApp {
            startup_media: None,
            server_url: url.to_owned(),
            player_name: name.to_owned(),
            recent_rooms_path: root.join(format!("{name}-recent-rooms.json")),
            repaint: Some(ctx.clone()),
            ..PieceOffApp::default()
        };
        app.recent_rooms.clear();
        Self {
            name: name.to_owned(),
            app,
            ctx,
            time: 0.0,
            frame_changes: 0,
            last_fingerprint: None,
        }
    }

    fn frame(&mut self) {
        self.time += 0.004;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(self.time),
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run(input, |ctx| app.tick(ctx));
        let fingerprint = self.app.last_video_frame.as_ref().map(|image| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            image.pixels.hash(&mut hasher);
            hasher.finish()
        });
        if fingerprint.is_some() && fingerprint != self.last_fingerprint {
            if self.last_fingerprint.is_some() {
                self.frame_changes += 1;
            }
            self.last_fingerprint = fingerprint;
        }
    }

    fn snapshot(&self) -> &RoomSnapshot {
        self.app
            .snapshot
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no room", self.name))
    }

    fn open_video(&mut self, path: &Path) {
        self.app.begin_probe(path.to_path_buf(), true, &self.ctx);
    }

    fn in_room(&self) -> bool {
        self.app.connected && self.app.client_id.is_some() && self.app.snapshot.is_some()
    }
}

/// Run every player's frames until `done` holds, or fail with what each
/// player last said.
fn pump(
    players: &mut [&mut Player],
    timeout: Duration,
    what: &str,
    mut done: impl FnMut(&[&mut Player]) -> bool,
) {
    let started = Instant::now();
    loop {
        for player in players.iter_mut() {
            player.frame();
        }
        if done(players) {
            return;
        }
        if started.elapsed() > timeout {
            let report: Vec<String> = players
                .iter()
                .map(|player| {
                    format!(
                        "{}: \"{}\" (connected {}, uploading {}, media available {:?}, media ready {})",
                        player.name,
                        player.app.status,
                        player.app.connected,
                        player.app.media_uploading,
                        player.app.snapshot.as_ref().map(|s| s.media_available),
                        player.app.media_ready
                    )
                })
                .collect();
            panic!(
                "timed out after {timeout:?} waiting for {what}; {}",
                report.join("; ")
            );
        }
        thread::sleep(STEP);
    }
}

/// Keep everyone running for `duration` (to let frames and messages flow).
fn run_for(players: &mut [&mut Player], duration: Duration) {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        for player in players.iter_mut() {
            player.frame();
        }
        thread::sleep(STEP);
    }
}

fn config_in(dir: &Path) -> Config {
    let mut config = Config::defaults(dir.join("server-data"));
    config.bind = "127.0.0.1:0".parse().unwrap();
    // Players here reconnect repeatedly from one address.
    config.connections_per_minute = 600;
    config
}

fn host_creates_a_video_room(host: &mut Player, video: &Path) -> String {
    host.open_video(video);
    pump(
        &mut [host],
        Duration::from_secs(20),
        "the video to load",
        |p| p[0].app.media_path.is_some() && p[0].app.is_video && p[0].app.media_size.0 > 0,
    );
    host.app.create_room();
    pump(
        &mut [host],
        Duration::from_secs(30),
        "the room and its media upload",
        |p| p[0].in_room() && p[0].snapshot().media_available && p[0].app.media_ready,
    );
    host.snapshot().invite_code.clone()
}

fn guest_joins_without_media(guest: &mut Player, host: &mut Player, code: &str) {
    guest.app.invite_code = code.to_owned();
    guest.app.join_room();
    pump(
        &mut [guest, host],
        Duration::from_secs(30),
        "the guest to download and load the media",
        |p| p[0].in_room() && p[0].app.media_ready && p[0].app.texture.is_some(),
    );
}

/// Move every piece, alternating players, onto its place on the board.
fn solve_together(host: &mut Player, guest: &mut Player) {
    let (cw, ch) = {
        let snapshot = host.snapshot();
        (snapshot.cell_width(), snapshot.cell_height())
    };
    let pieces: Vec<(u32, u16, u16)> = host
        .snapshot()
        .pieces
        .iter()
        .map(|piece| (piece.id, piece.row, piece.column))
        .collect();
    for (id, row, column) in pieces {
        let player = if id % 2 == 0 { &mut *host } else { &mut *guest };
        let (x, y) = (f32::from(column) * cw, f32::from(row) * ch);
        player.app.send(ClientMessage::DragStartSelection {
            piece_ids: vec![id],
        });
        player.app.send(ClientMessage::MoveSelection {
            anchor_piece_id: id,
            x,
            y,
        });
        player.app.send(ClientMessage::DragEndSelection {
            anchor_piece_id: id,
            x,
            y,
        });
    }
    pump(
        &mut [host, guest],
        Duration::from_secs(20),
        "both players to see the finished puzzle",
        |p| p[0].snapshot().completed() && p[1].snapshot().completed(),
    );
    // Both see the same stopped clock and the pieces credited to whoever
    // dropped them.
    let total = host.snapshot().pieces.len() as u32;
    for player in [&*host, &*guest] {
        let stats = &player.snapshot().stats;
        assert!(
            stats.finished && !stats.running(),
            "{}: {stats:?}",
            player.name
        );
        assert_eq!(stats.players.iter().map(|p| p.pieces).sum::<u32>(), total);
        for name in [&host.name, &guest.name] {
            assert!(
                stats
                    .players
                    .iter()
                    .any(|p| &p.name == name && p.pieces > 0),
                "{name} is credited: {stats:?}"
            );
        }
    }
    assert_eq!(host.snapshot().stats, guest.snapshot().stats);
}

#[test]
fn two_players_solve_a_looping_video_leave_rejoin_and_survive_a_server_restart() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("slice");
    let video = small_video(&root);
    let config = config_in(&root);
    let server = RunningServer::start(&config).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut guest = Player::new("Guest", &url, &root);

    // Create, upload, join, download.
    let code = host_creates_a_video_room(&mut host, &video);
    assert_eq!(code.len(), 8);
    guest_joins_without_media(&mut guest, &mut host, &code);
    assert!(!guest.app.is_host());
    assert!(host.app.is_host());
    assert_eq!(guest.app.media_hash, host.app.media_hash, "verified copy");

    // The host sets a loop; the guest follows it.
    host.app.loop_start_ms = 500;
    host.app.loop_end_ms = 2500;
    host.app.publish_playback(true, 500);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the guest to receive the loop",
        |p| {
            let playback = &p[1].snapshot().playback;
            playback.playing && playback.loop_start_ms == 500 && playback.loop_end_ms == 2500
        },
    );

    // Both clients decode and show moving frames; pausing freezes both.
    host.frame_changes = 0;
    guest.frame_changes = 0;
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(1500));
    assert!(
        host.frame_changes >= 8,
        "host saw {} frames",
        host.frame_changes
    );
    assert!(
        guest.frame_changes >= 8,
        "guest saw {} frames",
        guest.frame_changes
    );
    host.app.set_playing(false);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the pause to reach the guest",
        |p| !p[1].snapshot().playback.playing,
    );
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(500));
    guest.frame_changes = 0;
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(600));
    assert!(
        guest.frame_changes <= 1,
        "paused video kept moving: {} changes",
        guest.frame_changes
    );

    // Seek, then play on from there.
    host.app.publish_playback(true, 1200);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the seek to reach the guest",
        |p| p[1].snapshot().playback.playing && p[1].snapshot().playback.position_ms == 1200,
    );

    // Solve it together.
    solve_together(&mut host, &mut guest);
    assert_eq!(host.snapshot().progress_percent(), 100);

    // The guest leaves and rejoins from its recent rooms; nothing is lost.
    guest.app.leave_room();
    guest.frame();
    assert!(guest.app.snapshot.is_none());
    guest.app.select_recent_room(0);
    guest.app.join_room();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the guest to rejoin",
        |p| p[1].in_room(),
    );
    assert!(guest.snapshot().completed(), "progress survived the rejoin");
    let playback = guest.snapshot().playback.clone();
    assert_eq!((playback.loop_start_ms, playback.loop_end_ms), (500, 2500));
    assert!(guest.app.media_ready, "its local copy is still in use");

    // The server restarts with both players connected.
    server.stop();
    let mut restarted = config.clone();
    restarted.bind = server_address(&url);
    let server = RunningServer::start(&restarted).unwrap();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(40),
        "both players to reconnect after the restart",
        |p| p[0].in_room() && p[1].in_room(),
    );
    for player in [&host, &guest] {
        let snapshot = player.snapshot();
        assert!(snapshot.completed(), "{} lost progress", player.name);
        assert!(
            snapshot.stats.finished,
            "{} lost the solve stats",
            player.name
        );
        assert_eq!(
            (
                snapshot.playback.loop_start_ms,
                snapshot.playback.loop_end_ms
            ),
            (500, 2500),
            "{} lost the loop",
            player.name
        );
        assert!(player.app.media_ready, "{} lost its media", player.name);
    }
    // Playback still works after the restart.
    host.app.publish_playback(true, 600);
    host.frame_changes = 0;
    guest.frame_changes = 0;
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(1500));
    assert!(host.frame_changes >= 8, "host: {}", host.frame_changes);
    assert!(guest.frame_changes >= 8, "guest: {}", guest.frame_changes);
    drop(server);
}

fn server_address(url: &str) -> std::net::SocketAddr {
    url.trim_start_matches("ws://")
        .trim_end_matches("/ws")
        .parse()
        .unwrap()
}

#[test]
fn a_server_crash_keeps_every_dropped_piece_and_frees_the_one_in_flight() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("crash");
    let video = small_video(&root);
    let config = config_in(&root);
    let server = RunningServer::start(&config).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut guest = Player::new("Guest", &url, &root);
    let code = host_creates_a_video_room(&mut host, &video);
    guest_joins_without_media(&mut guest, &mut host, &code);

    let (cw, ch) = (host.snapshot().cell_width(), host.snapshot().cell_height());
    let place = |player: &mut Player, id: u32| {
        let piece = player.snapshot().piece(id).unwrap().clone();
        let (x, y) = (f32::from(piece.column) * cw, f32::from(piece.row) * ch);
        player.app.send(ClientMessage::DragStartSelection {
            piece_ids: vec![id],
        });
        player.app.send(ClientMessage::DragEndSelection {
            anchor_piece_id: id,
            x,
            y,
        });
    };
    for id in [0, 1, 2] {
        place(&mut host, id);
    }
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "three pieces to be placed",
        |p| {
            p[1].snapshot()
                .pieces
                .iter()
                .filter(|piece| piece.placed)
                .count()
                == 3
        },
    );
    // The guest picks up piece 7 and carries it, but never lets go.
    guest
        .app
        .send(ClientMessage::DragStartSelection { piece_ids: vec![7] });
    guest.app.send(ClientMessage::MoveSelection {
        anchor_piece_id: 7,
        x: 3.0 * cw,
        y: 2.0 * ch,
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the carried piece to be held",
        |p| p[0].snapshot().piece(7).unwrap().held_by.is_some(),
    );

    server.crash();
    let mut restarted = config.clone();
    restarted.bind = server_address(&url);
    let server = RunningServer::start(&restarted).unwrap();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(40),
        "both players to reconnect after the crash",
        |p| p[0].in_room() && p[1].in_room(),
    );
    for player in [&host, &guest] {
        let snapshot = player.snapshot();
        for id in [0, 1, 2] {
            assert!(
                snapshot.piece(id).unwrap().placed,
                "{}: piece {id}",
                player.name
            );
        }
        let carried = snapshot.piece(7).unwrap();
        assert!(
            !carried.placed,
            "{}: the unreleased piece is not placed",
            player.name
        );
        assert!(
            carried.held_by.is_none(),
            "{}: its lease is gone",
            player.name
        );
    }
    // And it can be picked up again by someone else.
    host.app
        .send(ClientMessage::DragStartSelection { piece_ids: vec![7] });
    // Piece 7 belongs at column 3, row 1.
    host.app.send(ClientMessage::DragEndSelection {
        anchor_piece_id: 7,
        x: 3.0 * cw,
        y: ch,
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "piece 7 to be placed after the crash",
        |p| p[1].snapshot().piece(7).unwrap().placed,
    );
    drop(server);
}

#[test]
fn a_room_the_server_no_longer_has_stops_the_reconnect_loop_and_keeps_the_work_local() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("gone");
    let video = small_video(&root);
    let config = config_in(&root);
    let server = RunningServer::start(&config).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    host_creates_a_video_room(&mut host, &video);

    // The server comes back with an empty data directory: the room expired.
    server.stop();
    std::fs::remove_dir_all(&config.data_dir).unwrap();
    let mut restarted = config.clone();
    restarted.bind = server_address(&url);
    let server = RunningServer::start(&restarted).unwrap();

    pump(
        &mut [&mut host],
        Duration::from_secs(30),
        "the player to learn the room is gone",
        |p| p[0].app.room_gone && p[0].app.network.is_none(),
    );
    assert!(
        host.app.reconnect_at.is_none(),
        "no further retries are scheduled"
    );
    assert!(
        host.app.status.contains("no longer on this server"),
        "{}",
        host.app.status
    );
    // Nothing comes back by itself, and the local puzzle is still there.
    run_for(&mut [&mut host], Duration::from_secs(4));
    assert!(host.app.network.is_none() && host.app.reconnect_at.is_none());
    assert!(
        host.app.snapshot.is_some(),
        "the player keeps its progress to save"
    );
    assert!(host.app.media_path.is_some());
    drop(server);
}

/// Forwards connections to `upstream`, slowing the client-to-server direction
/// to `bytes_per_second` so a media upload takes long enough to interrupt.
struct SlowProxy {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    streams: Arc<std::sync::Mutex<Vec<std::net::TcpStream>>>,
    forwarded: Arc<std::sync::atomic::AtomicU64>,
}

impl SlowProxy {
    fn start(upstream: std::net::SocketAddr, bytes_per_second: u64) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let streams: Arc<std::sync::Mutex<Vec<std::net::TcpStream>>> = Arc::default();
        let forwarded: Arc<std::sync::atomic::AtomicU64> = Arc::default();
        let (stop_flag, kept, counter) = (stop.clone(), streams.clone(), forwarded.clone());
        thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                let Ok((client, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                client.set_nonblocking(false).unwrap();
                let Ok(server) = std::net::TcpStream::connect(upstream) else {
                    continue;
                };
                for stream in [&client, &server] {
                    kept.lock().unwrap().push(stream.try_clone().unwrap());
                }
                let (mut from_client, mut to_server) =
                    (client.try_clone().unwrap(), server.try_clone().unwrap());
                let counter = counter.clone();
                thread::spawn(move || {
                    let mut buffer = [0u8; 8192];
                    while let Ok(count) = from_client.read(&mut buffer) {
                        if count == 0 || to_server.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                        counter.fetch_add(count as u64, Ordering::Relaxed);
                        thread::sleep(Duration::from_secs_f64(
                            count as f64 / bytes_per_second as f64,
                        ));
                    }
                    let _ = to_server.shutdown(std::net::Shutdown::Both);
                });
                let (mut from_server, mut to_client) = (server, client);
                thread::spawn(move || {
                    let mut buffer = [0u8; 8192];
                    while let Ok(count) = from_server.read(&mut buffer) {
                        if count == 0 || to_client.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                    }
                    let _ = to_client.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        Self {
            address,
            stop,
            streams,
            forwarded,
        }
    }

    fn url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    /// Cut every connection at once, like a dropped network.
    fn cut(&self) {
        self.stop.store(true, Ordering::Relaxed);
        for stream in self.streams.lock().unwrap().iter() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[test]
fn the_host_dropping_mid_upload_hands_the_upload_to_a_guest_who_has_the_file() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("handoff");
    // Noise keeps this large enough to take a while through the slow proxy.
    let video = make_video(
        &root,
        "big.mp4",
        "testsrc2=size=480x270:rate=24,noise=alls=40:allf=t",
        4,
    );
    let size = std::fs::metadata(&video).unwrap().len();
    assert!(size > 4_000_000, "the test video is only {size} bytes");

    let config = config_in(&root);
    let server = RunningServer::start(&config).unwrap();
    let proxy = SlowProxy::start(server.address(), 1_500_000);
    let mut host = Player::new("Host", &proxy.url(), &root);
    let mut guest = Player::new("Guest", &server.url(), &root);

    host.open_video(&video);
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the host's video",
        |p| p[0].app.media_path.is_some() && p[0].app.media_size.0 > 0,
    );
    host.app.create_room();
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the upload to start",
        |p| p[0].in_room() && p[0].app.media_uploading,
    );
    let code = host.snapshot().invite_code.clone();
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "bytes to flow",
        |_| proxy.forwarded.load(Ordering::Relaxed) > 800_000,
    );
    assert!(!host.snapshot().media_available, "still uploading");

    // A guest who already has the same file joins while it uploads.
    guest.open_video(&video);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the guest's file",
        |p| p[1].app.media_path.is_some() && p[1].app.media_size.0 > 0,
    );
    guest.app.invite_code = code.clone();
    guest.app.join_room();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the guest to join",
        |p| p[1].in_room(),
    );
    assert!(!guest.app.is_host());

    // The host's connection dies mid-upload.
    proxy.cut();
    drop(host);
    pump(
        &mut [&mut guest],
        Duration::from_secs(30),
        "the guest to take over the upload and finish it",
        |p| p[0].app.is_host() && p[0].snapshot().media_available && !p[0].app.media_uploading,
    );

    // Someone who joins afterwards downloads a verified copy.
    let mut late = Player::new("Late", &server.url(), &root);
    late.app.invite_code = code;
    late.app.join_room();
    pump(
        &mut [&mut guest, &mut late],
        Duration::from_secs(30),
        "the late joiner to download the media",
        |p| p[1].in_room() && p[1].app.media_ready,
    );
    assert_eq!(late.app.media_hash, guest.app.media_hash);

    // The interrupted upload left nothing behind.
    let media_dir = config.data_dir.join("media");
    let names: Vec<String> = std::fs::read_dir(&media_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1, "media directory holds {names:?}");
    assert!(!names[0].ends_with(".part"), "{names:?}");
    drop(server);
}

/// Replace the host's video decoder with one that runs FFmpeg through a shim
/// script. The shim notes each process id in `pids`, and exits with an error
/// instead of running FFmpeg while `broken` exists.
#[cfg(unix)]
fn decode_through_shim(player: &mut Player, dir: &Path) -> PathBuf {
    let shim = dir.join("ffmpeg-shim");
    crate::tests::write_ffmpeg_shim(
        &shim,
        &format!(
            "echo $$ >> '{pids}'\nif [ -e '{broken}' ]; then exit 1; fi\nexec ffmpeg \"$@\"\n",
            pids = dir.join("pids").display(),
            broken = dir.join("broken").display(),
        ),
    );
    let path = player.app.media_path.clone().unwrap();
    player.app.decoder_ffmpeg_program = shim.clone().into_os_string();
    let ctx = player.ctx.clone();
    player
        .app
        .start_video_decoder_using(path, false, ctx.clone(), shim.clone().into_os_string());
    player.app.sync_decoder_playback(&ctx);
    shim
}

#[cfg(unix)]
fn last_pid(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join("pids"))
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn ffmpeg_killed_mid_playback_is_restarted_and_the_video_keeps_playing() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("kill");
    let video = small_video(&root);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let mut host = Player::new("Host", &server.url(), &root);
    host_creates_a_video_room(&mut host, &video);
    decode_through_shim(&mut host, &root);
    host.app.publish_playback(true, 0);
    pump(
        &mut [&mut host],
        Duration::from_secs(10),
        "frames from the shim",
        |_| root.join("pids").is_file(),
    );
    host.frame_changes = 0;
    run_for(&mut [&mut host], Duration::from_millis(800));
    assert!(
        host.frame_changes >= 5,
        "playing before the kill: {}",
        host.frame_changes
    );

    // Kill FFmpeg (not the worker) twice, a moment apart.
    for _ in 0..2 {
        let pid = last_pid(&root);
        Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap();
        run_for(&mut [&mut host], Duration::from_millis(900));
    }
    host.frame_changes = 0;
    run_for(&mut [&mut host], Duration::from_millis(1200));
    assert!(
        host.frame_changes >= 5,
        "frames resumed after the kills: {}",
        host.frame_changes
    );
    assert!(
        host.app
            .decoder_worker_alive
            .as_ref()
            .is_some_and(|alive| alive.load(Ordering::Acquire)),
        "the worker survived: {}",
        host.app.status
    );
    drop(server);
}

#[cfg(unix)]
#[test]
fn a_decoder_that_gave_up_is_started_again_by_the_app_when_ffmpeg_works_again() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("revive");
    let video = small_video(&root);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let mut host = Player::new("Host", &server.url(), &root);
    host_creates_a_video_room(&mut host, &video);

    // FFmpeg fails every time: the worker exhausts its retries and stops.
    std::fs::write(root.join("broken"), b"").unwrap();
    decode_through_shim(&mut host, &root);
    host.app.publish_playback(true, 0);
    pump(
        &mut [&mut host],
        Duration::from_secs(10),
        "the worker to give up",
        |p| {
            p[0].app
                .decoder_worker_alive
                .as_ref()
                .is_some_and(|alive| !alive.load(Ordering::Acquire))
        },
    );

    // FFmpeg works again; nobody touches the playback controls. The app
    // restarts the decoder on its own and the picture moves again.
    std::fs::remove_file(root.join("broken")).unwrap();
    host.frame_changes = 0;
    pump(
        &mut [&mut host],
        Duration::from_secs(12),
        "the app to restart the decoder",
        |p| p[0].frame_changes >= 5,
    );
    assert!(
        host.app
            .decoder_worker_alive
            .as_ref()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
    );
    drop(server);
}

#[test]
fn queue_swap_advance_and_lock_across_two_clients() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("queue");
    let video_a = small_video(&root);
    // A different shape and length, so the swap has to cover-fit.
    let video_b = make_video(&root, "second.mp4", "testsrc2=size=240x180:rate=24", 3);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut guest = Player::new("Guest", &url, &root);

    let code = host_creates_a_video_room(&mut host, &video_a);
    guest_joins_without_media(&mut guest, &mut host, &code);
    let hash_a = host.app.media_hash.clone();

    // One piece goes home before anything is swapped.
    let (x, y) = {
        let snapshot = host.snapshot();
        let piece = snapshot.piece(0).unwrap();
        (
            f32::from(piece.column) * snapshot.cell_width(),
            f32::from(piece.row) * snapshot.cell_height(),
        )
    };
    host.app
        .send(ClientMessage::DragStartSelection { piece_ids: vec![0] });
    host.app.send(ClientMessage::MoveSelection {
        anchor_piece_id: 0,
        x,
        y,
    });
    host.app.send(ClientMessage::DragEndSelection {
        anchor_piece_id: 0,
        x,
        y,
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "both players to see piece 0 placed",
        |p| p[0].snapshot().piece(0).unwrap().placed && p[1].snapshot().piece(0).unwrap().placed,
    );
    let positions = |player: &Player| -> Vec<(u32, i32, i32)> {
        player
            .snapshot()
            .pieces
            .iter()
            .map(|piece| (piece.id, piece.x.round() as i32, piece.y.round() as i32))
            .collect()
    };
    let before = positions(&host);

    // Queue the second video: it uploads, then the guest prefetches it.
    host.app.send_queue_add_from_file(video_b.clone());
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(40),
        "the second video to upload and reach the guest",
        |p| {
            let snapshot = p[1].snapshot();
            snapshot.queue.len() == 2
                && snapshot.queue[1].available
                && p[1]
                    .app
                    .media_files
                    .lock()
                    .unwrap()
                    .contains_key(&snapshot.queue[1].sha256)
        },
    );
    let hash_b = host.snapshot().queue[1].sha256.clone();
    assert_ne!(hash_a, hash_b);

    // Hot swap: both clients change video; every piece stays put.
    host.app.send(ClientMessage::SelectItem { index: 1 });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "both players to show the second video",
        |p| {
            p[0].app.media_hash == hash_b
                && p[1].app.media_hash == hash_b
                && p[0].app.media_ready
                && p[1].app.media_ready
        },
    );
    assert_eq!(positions(&host), before);
    assert_eq!(positions(&guest), before);
    assert!(host.snapshot().piece(0).unwrap().placed);

    // Repeat All: near the end of the last video the room goes back to the
    // first one by itself, on both clients.
    host.app.send(ClientMessage::SetRepeat {
        mode: pieceoff_core::RepeatMode::All,
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "repeat All to reach both players",
        |p| p[1].snapshot().repeat == pieceoff_core::RepeatMode::All,
    );
    host.app.publish_playback(true, 2_500);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the room to advance to the first video",
        |p| {
            p[0].snapshot().current == 0
                && p[1].snapshot().current == 0
                && p[0].app.media_hash == hash_a
                && p[1].app.media_hash == hash_a
                && p[1].snapshot().playback.playing
        },
    );

    // Locked: the host's own swap is refused.
    host.app.send(ClientMessage::SetVideoLock { locked: true });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the lock to reach both players",
        |p| p[0].snapshot().video_lock && p[1].snapshot().video_lock,
    );
    host.app.send(ClientMessage::SelectItem { index: 1 });
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(800));
    assert_eq!(host.snapshot().current, 0);
    assert_eq!(guest.snapshot().current, 0);
    assert!(
        host.app.status.contains("locked"),
        "status was {:?}",
        host.app.status
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_guest_cannot_take_a_piece_from_the_hosts_holder_until_it_is_removed() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("holders");
    let video = small_video(&root);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut guest = Player::new("Guest", &url, &root);
    let code = host_creates_a_video_room(&mut host, &video);
    guest_joins_without_media(&mut guest, &mut host, &code);

    // The host makes a holder beside the board and puts piece 0 in it.
    let (width, height, cw, ch) = {
        let snapshot = host.snapshot();
        let (cw, ch) = (snapshot.cell_width(), snapshot.cell_height());
        (4.0 * cw, 3.0 * ch, cw, ch)
    };
    let (x, y) = host
        .snapshot()
        .free_holder_spot(width, height, -width, 0.0)
        .expect("free table space");
    host.app.send(ClientMessage::CreateHolder {
        x,
        y,
        width,
        height,
        label: "Sky".into(),
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "both players to see the holder",
        |p| p[0].snapshot().holders.len() == 1 && p[1].snapshot().holders.len() == 1,
    );
    let holder_id = guest.snapshot().holders[0].id;
    assert_eq!(guest.snapshot().holders[0].owner, "Host");
    let (px, py) = (x + width / 2.0 - cw / 2.0, y + height / 2.0 - ch / 2.0);
    for message in [
        ClientMessage::DragStartSelection { piece_ids: vec![0] },
        ClientMessage::DragEndSelection {
            anchor_piece_id: 0,
            x: px,
            y: py,
        },
    ] {
        host.app.send(message);
    }
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the piece to land in the holder on both screens",
        |p| {
            p[1].snapshot()
                .piece(0)
                .is_some_and(|piece| piece.x == px && piece.held_by.is_none())
        },
    );

    // The guest's grab is refused by the server, and nothing moves.
    guest
        .app
        .send(ClientMessage::DragStartSelection { piece_ids: vec![0] });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the guest to be told the piece is reserved",
        |p| p[1].app.status.contains("Host's holder"),
    );
    guest.app.send(ClientMessage::DragEndSelection {
        anchor_piece_id: 0,
        x: 0.0,
        y: 0.0,
    });
    run_for(&mut [&mut host, &mut guest], Duration::from_millis(400));
    for player in [&host, &guest] {
        let piece = player.snapshot().piece(0).unwrap();
        assert_eq!((piece.x, piece.y), (px, py), "{}", player.name);
        assert!(piece.held_by.is_none() && !piece.placed);
    }

    // Once the host removes the holder, the piece is anyone's where it lies.
    host.app.send(ClientMessage::DeleteHolder { holder_id });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the holder to go",
        |p| p[1].snapshot().holders.is_empty(),
    );
    guest
        .app
        .send(ClientMessage::DragStartSelection { piece_ids: vec![0] });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the guest to pick the piece up",
        |p| p[0].snapshot().piece(0).unwrap().held_by.as_deref() == p[1].app.client_id.as_deref(),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_spectator_sees_the_moves_cannot_make_one_and_does_not_keep_the_clock_running() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("spectate");
    let video = small_video(&root);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut watcher = Player::new("Watcher", &url, &root);
    let code = host_creates_a_video_room(&mut host, &video);
    watcher.app.watch_only = true;
    guest_joins_without_media(&mut watcher, &mut host, &code);
    assert!(watcher.app.spectating);
    pump(
        &mut [&mut host, &mut watcher],
        Duration::from_secs(10),
        "the host to see the spectator",
        |p| p[0].app.spectators == ["Watcher"] && p[0].app.peers.is_empty(),
    );
    assert_eq!(watcher.app.peers, ["Host"]);

    // The host's move reaches the spectator.
    let (x, y) = {
        let piece = host.snapshot().piece(0).unwrap();
        (piece.x + 40.0, piece.y + 30.0)
    };
    for message in [
        ClientMessage::DragStartSelection { piece_ids: vec![0] },
        ClientMessage::DragEndSelection {
            anchor_piece_id: 0,
            x,
            y,
        },
    ] {
        host.app.send(message);
    }
    pump(
        &mut [&mut host, &mut watcher],
        Duration::from_secs(10),
        "the spectator to see the move",
        |p| {
            p[1].snapshot()
                .piece(0)
                .is_some_and(|piece| piece.x == x && piece.y == y)
        },
    );
    assert!(watcher.snapshot().stats.running());

    // A spectator's grab, forced past the client's own guard, is refused.
    watcher
        .app
        .network
        .as_ref()
        .unwrap()
        .outgoing
        .send(ClientMessage::DragStartSelection { piece_ids: vec![1] })
        .unwrap();
    pump(
        &mut [&mut host, &mut watcher],
        Duration::from_secs(10),
        "the spectator to be told no",
        |p| p[1].app.status.contains("watching"),
    );
    run_for(&mut [&mut host, &mut watcher], Duration::from_millis(300));
    assert!(host.snapshot().piece(1).unwrap().held_by.is_none());

    // With only the spectator left, the clock stops, on the spectator's
    // screen too.
    host.app.leave_room();
    pump(
        &mut [&mut host, &mut watcher],
        Duration::from_secs(10),
        "the spectator to see the clock pause",
        |p| p[1].app.peers.is_empty() && !p[1].snapshot().stats.running(),
    );
    let paused = watcher.snapshot().stats.elapsed_ms;
    run_for(&mut [&mut watcher], Duration::from_millis(300));
    assert_eq!(watcher.snapshot().stats.elapsed_ms, paused);
    assert_ne!(
        watcher.snapshot().host_id,
        watcher.app.client_id.clone().unwrap(),
        "a spectator never hosts"
    );

    // The spectator can still replay what happened.
    watcher.app.request_timelapse();
    pump(
        &mut [&mut watcher],
        Duration::from_secs(10),
        "the timelapse to open",
        |p| p[0].app.replay.is_some(),
    );
    assert_eq!(watcher.app.replay.as_ref().unwrap().steps(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn two_players_join_and_solve_a_hexagon_puzzle() {
    let _serial = serial();
    if !ffmpeg_available() {
        eprintln!("Skipping: ffmpeg is not installed.");
        return;
    }
    let root = scratch("hexagons");
    let video = small_video(&root);
    let server = RunningServer::start(&config_in(&root)).unwrap();
    let url = server.url();
    let mut host = Player::new("Host", &url, &root);
    let mut guest = Player::new("Guest", &url, &root);
    host.app.cut_style = CutStyle::Hexagon;
    (host.app.rows, host.app.columns) = (3, 4);
    let code = host_creates_a_video_room(&mut host, &video);
    guest_joins_without_media(&mut guest, &mut host, &code);
    assert_eq!(guest.snapshot().cut_style, CutStyle::Hexagon);
    // Both clients derive the same contours.
    for id in [0, 5, 11] {
        let (a, b) = (host.snapshot(), guest.snapshot());
        assert_eq!(
            piece_outline(a, a.piece(id).unwrap()),
            piece_outline(b, b.piece(id).unwrap())
        );
    }

    // The host lays (0,1) on the table; the guest drops (1,0), its
    // neighbour down and to the left, close to it, and they join.
    let (cw, ch) = {
        let snapshot = host.snapshot();
        (snapshot.cell_width(), snapshot.cell_height())
    };
    let (x, y) = (host.snapshot().width as f32 * 0.5, -3.0 * ch);
    for (player, id, x, y) in [
        (&mut host, 1, x, y),
        (&mut guest, 4, x - cw + 6.0, y + ch - 5.0),
    ] {
        player.app.send(ClientMessage::DragStartSelection {
            piece_ids: vec![id],
        });
        player.app.send(ClientMessage::DragEndSelection {
            anchor_piece_id: id,
            x,
            y,
        });
        pump(
            &mut [&mut *player],
            Duration::from_secs(10),
            "the drop to land",
            |p| {
                let piece = p[0].snapshot().piece(id).unwrap();
                piece.held_by.is_none() && (piece.y - y).abs() < ch
            },
        );
    }
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "both players to see the hexagons join",
        |p| {
            p.iter().all(|player| {
                let snapshot = player.snapshot();
                snapshot.piece(1).unwrap().group_id == snapshot.piece(4).unwrap().group_id
            })
        },
    );
    solve_together(&mut host, &mut guest);
    assert_eq!(guest.snapshot().progress_percent(), 100);
}
