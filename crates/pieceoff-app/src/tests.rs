use super::*;

#[test]
fn focus_view_centers_and_fits_the_requested_content_region() {
    let canvas = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(1000.0, 600.0));
    let content_size = Vec2::new(1200.0, 900.0);
    let region = Rect::from_min_size(Pos2::new(100.0, 600.0), Vec2::new(1000.0, 250.0));
    let base_scale = (canvas.width() * 0.92 / content_size.x)
        .min(canvas.height() * 0.92 / content_size.y)
        .max(0.05);

    let (zoom, pan) = focus_content_region(canvas, content_size, region, base_scale);
    let scale = zoom * base_scale;
    let visible_center = canvas.center().to_vec2() + pan + region.center().to_vec2() * scale
        - content_size * (scale * 0.5);

    assert!((visible_center - canvas.center().to_vec2()).length() < 0.001);
    assert!((region.width() * scale - canvas.width() * 0.9).abs() < 0.001);
}

#[test]
fn tray_filters_separate_edge_and_interior_pieces() {
    let edge = Piece {
        id: 0,
        group_id: 0,
        row: 0,
        column: 2,
        x: 900.0,
        y: 100.0,
        placed: false,
        held_by: None,
    };
    let interior = Piece {
        row: 1,
        column: 1,
        ..edge.clone()
    };

    assert!(PieceFilter::All.includes(&edge, 4, 4));
    assert!(PieceFilter::Edges.includes(&edge, 4, 4));
    assert!(!PieceFilter::Interior.includes(&edge, 4, 4));
    assert!(!PieceFilter::Edges.includes(&interior, 4, 4));
    assert!(PieceFilter::Interior.includes(&interior, 4, 4));
    assert!(piece_visible_for_filter(
        PieceFilter::Edges,
        &Piece {
            x: 100.0,
            ..interior.clone()
        },
        800,
        4,
        4,
    ));
}

#[test]
fn recent_rooms_persist_deduplicate_limit_and_forget_entries() {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-recent-rooms-{}-{stamp}",
        std::process::id()
    ));
    let path = directory.join("recent-rooms.json");
    let mut rooms = Vec::new();
    remember_recent_room(
        &path,
        &mut rooms,
        RecentRoom {
            title: "First room".into(),
            server_url: " ws://LOCALHOST:8787/ws/ ".into(),
            invite_code: "abc123".into(),
            player_name: " Player ".into(),
            last_joined_ms: 1,
        },
    )
    .unwrap();
    assert_eq!(rooms[0].invite_code, "ABC123");
    assert_eq!(rooms[0].server_url, "ws://LOCALHOST:8787/ws");
    assert_eq!(rooms[0].player_name, "Player");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    for index in 0..MAX_RECENT_ROOMS + 2 {
        remember_recent_room(
            &path,
            &mut rooms,
            RecentRoom {
                title: format!("Room {index}"),
                server_url: "ws://localhost:8787/ws".into(),
                invite_code: format!("ROOM{index}"),
                player_name: "Player".into(),
                last_joined_ms: index as u64 + 2,
            },
        )
        .unwrap();
    }
    assert_eq!(rooms.len(), MAX_RECENT_ROOMS);
    assert_eq!(
        rooms[0].invite_code,
        format!("ROOM{}", MAX_RECENT_ROOMS + 1)
    );
    assert_eq!(load_recent_rooms(&path).unwrap(), rooms);

    forget_recent_room(&path, &mut rooms, 0).unwrap();
    assert_eq!(rooms.len(), MAX_RECENT_ROOMS - 1);
    assert_eq!(load_recent_rooms(&path).unwrap(), rooms);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn accepted_room_welcome_is_saved_and_can_refill_the_rejoin_form() {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-welcome-history-{}-{stamp}",
        std::process::id()
    ));
    let path = directory.join("recent-rooms.json");
    let (incoming_tx, incoming) = mpsc::channel();
    let (outgoing, _outgoing_rx) = tokio_mpsc::unbounded_channel();
    let mut app = PieceOffApp {
        server_url: "ws://game.example:8787/ws".into(),
        player_name: "Puzzle Friend".into(),
        recent_rooms: Vec::new(),
        recent_rooms_path: path.clone(),
        network: Some(Network { outgoing, incoming }),
        ..PieceOffApp::default()
    };
    let snapshot = RoomSnapshot::create(
        "history-room-id".into(),
        "PUZZLE42".into(),
        "Moving Moon".into(),
        "a".repeat(64),
        "mp4".into(),
        32,
        24,
        2,
        2,
        "host-id".into(),
    )
    .unwrap();
    incoming_tx
        .send(ServerMessage::Welcome {
            client_id: "guest-id".into(),
            name: "Puzzle Friend".into(),
            peers: Vec::new(),
            snapshot,
            media_upload_token: None,
            spectator: false,
            spectators: Vec::new(),
            server_now_ms: 0,
        })
        .unwrap();

    app.process_network(&egui::Context::default());

    assert_eq!(app.recent_rooms.len(), 1);
    assert_eq!(app.recent_rooms[0].invite_code, "PUZZLE42");
    assert_eq!(app.recent_rooms[0].title, "Moving Moon");
    assert_eq!(app.recent_room_selection, Some(0));
    let mut reopened = PieceOffApp {
        recent_rooms: load_recent_rooms(&path).unwrap(),
        ..PieceOffApp::default()
    };
    reopened.select_recent_room(0);
    assert_eq!(reopened.server_url, "ws://game.example:8787/ws");
    assert_eq!(reopened.invite_code, "PUZZLE42");
    assert_eq!(reopened.player_name, "Puzzle Friend");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn portable_package_preserves_puzzle_state_and_verifies_embedded_media() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("pieceoff-package-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let media = format!("portable test image {stamp}").into_bytes();
    let media_path = directory.join("source.png");
    std::fs::write(&media_path, &media).unwrap();
    let media_hash = format!("{:x}", Sha256::digest(&media));
    let mut snapshot = RoomSnapshot::create(
        "room-id".into(),
        "ROOM42".into(),
        "Portable puzzle".into(),
        media_hash.clone(),
        "png".into(),
        32,
        24,
        2,
        2,
        "host-id".into(),
    )
    .unwrap();
    snapshot.pieces[0].placed = true;
    snapshot.pieces[0].held_by = Some("stale-player".into());
    snapshot.playback.playing = true;
    snapshot.playback.position_ms = 500;
    snapshot.media_available = true;
    // A run in progress: 40 s banked, and the clock still running.
    snapshot.stats.started = true;
    snapshot.stats.elapsed_ms = 40_000;
    snapshot.stats.running_since_ms = Some(board::wall_clock_ms());
    snapshot.stats.credit("Ada", 3);
    let package_path = directory.join("puzzle.pieceoff");

    write_puzzle_package(&package_path, &media_path, &snapshot).unwrap();
    let (restored, restored_media) = read_puzzle_package(&package_path).unwrap();
    assert_eq!(restored.title, snapshot.title);
    // Solve stats travel with the puzzle, stopped and with the time banked.
    assert!(restored.stats.started && !restored.stats.running());
    assert!((40_000..50_000).contains(&restored.stats.elapsed_ms));
    assert_eq!(restored.stats.players, snapshot.stats.players);
    assert_eq!(restored.media_sha256, media_hash);
    assert!(restored.pieces[0].placed);
    assert_eq!(restored.pieces[0].held_by, None);
    assert!(!restored.playback.playing);
    assert_eq!(restored.playback.position_ms, 500);
    assert_eq!(restored.id, "");
    assert_eq!(restored.invite_code, "");
    assert_eq!(std::fs::read(&restored_media).unwrap(), media);

    let mut package_bytes = std::fs::read(&package_path).unwrap();
    *package_bytes.last_mut().unwrap() ^= 0xff;
    std::fs::write(&package_path, package_bytes).unwrap();
    assert!(
        read_puzzle_package(&package_path)
            .unwrap_err()
            .contains("SHA-256")
    );

    let _ = std::fs::remove_file(restored_media);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn missing_ffmpeg_is_reported_without_spinning_the_decoder() {
    let mut app = PieceOffApp {
        media_size: (32, 24),
        ..PieceOffApp::default()
    };
    app.start_video_decoder_using(
        PathBuf::from("missing-video.mp4"),
        false,
        egui::Context::default(),
        OsString::from("pieceoff-test-ffmpeg-does-not-exist"),
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !app.status.starts_with("Could not start FFmpeg.") {
        app.poll_decoder_errors();
        thread::sleep(Duration::from_millis(5));
    }

    assert!(
        app.status.starts_with("Could not start FFmpeg."),
        "decoder startup failure should reach the player status: {}",
        app.status
    );
}

#[cfg(unix)]
/// Write an executable shell script that stands in for ffmpeg.
///
/// A script written moments ago can fail to start with "Text file busy" when
/// another test thread forks between the write and the close, so wait until it
/// runs. The probe argument makes the script exit before doing anything.
#[cfg(unix)]
pub(crate) fn write_ffmpeg_shim(program: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let body = body.strip_prefix("#!/bin/sh\n").unwrap_or(body);
    std::fs::write(
        program,
        format!("#!/bin/sh\n[ \"$1\" = --shim-probe ] && exit 0\n{body}"),
    )
    .unwrap();
    std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let give_up = Instant::now() + Duration::from_secs(5);
    loop {
        match Command::new(program).arg("--shim-probe").status() {
            Ok(status) if status.success() => return,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < give_up =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            other => panic!("the ffmpeg shim cannot run: {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn ffmpeg_decoder_recovers_from_one_transient_decode_failure() {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-decoder-recovery-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("failed-once");
    let program = directory.join("ffmpeg-shim");
    write_ffmpeg_shim(
        &program,
        &format!(
            "if [ ! -e '{}' ]; then\n  : > '{}'\n  printf 'bad'\n  exit 1\nfi\nprintf 'abcdefghijklmnop'\nexit 0\n",
            marker.display(),
            marker.display()
        ),
    );

    let (control_tx, control_rx) = mpsc::channel();
    let (frames_tx, frames_rx) = mpsc::sync_channel(1);
    let (errors_tx, errors_rx) = mpsc::channel();
    let worker =
        FfmpegDecoderBackend::new(program.clone().into_os_string()).start(DecoderStartup {
            path: directory.join("video.mp4"),
            duration_ms: 1000,
            width: 2,
            height: 2,
            fps: DEFAULT_VIDEO_FPS,
            control_rx,
            frames: frames_tx,
            errors: errors_tx,
            repaint: egui::Context::default(),
            running: Arc::new(AtomicBool::new(false)),
            worker_alive: Arc::new(AtomicBool::new(true)),
        });

    let frame = frames_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("decoder should recover and send a complete frame");
    assert_eq!(frame.rgba, b"abcdefghijklmnop");
    assert!(
        marker.exists(),
        "the first process should fail exactly once"
    );
    assert!(errors_rx.try_recv().is_err());
    drop(control_tx);
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

/// Start the FFmpeg decoder worker on a shim script and return its channels.
#[cfg(unix)]
fn start_shim_decoder(
    name: &str,
    script_body: &str,
    stall_timeout: Duration,
) -> (
    thread::JoinHandle<()>,
    mpsc::Receiver<DecodedFrame>,
    mpsc::Receiver<String>,
    mpsc::Sender<DecoderCommand>,
    PathBuf,
) {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-decoder-{name}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let program = directory.join("ffmpeg-shim");
    write_ffmpeg_shim(&program, script_body);
    let (control_tx, control_rx) = mpsc::channel();
    let (frames_tx, frames_rx) = mpsc::sync_channel(1);
    let (errors_tx, errors_rx) = mpsc::channel();
    let worker = FfmpegDecoderBackend {
        program: program.into_os_string(),
        stall_timeout,
    }
    .start(DecoderStartup {
        path: directory.join("video.mp4"),
        duration_ms: 1000,
        width: 2,
        height: 2,
        fps: DEFAULT_VIDEO_FPS,
        control_rx,
        frames: frames_tx,
        errors: errors_tx,
        repaint: egui::Context::default(),
        running: Arc::new(AtomicBool::new(false)),
        worker_alive: Arc::new(AtomicBool::new(true)),
    });
    (worker, frames_rx, errors_rx, control_tx, directory)
}

#[cfg(unix)]
#[test]
fn a_hung_ffmpeg_is_killed_retried_and_finally_reported_without_freezing_the_worker() {
    // `exec` makes the shim itself the sleeping process, so the watchdog's
    // kill reaches it directly.
    let (worker, frames_rx, errors_rx, control_tx, directory) =
        start_shim_decoder("stall", "exec sleep 30\n", Duration::from_millis(300));
    let started = Instant::now();
    let error = errors_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("a stuck FFmpeg must end in an error, not a frozen worker");
    assert!(error.contains("stopped responding"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "gave up after {:?}",
        started.elapsed()
    );
    assert!(frames_rx.try_recv().is_err(), "nothing was ever decoded");
    drop(control_tx);
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn a_video_that_decodes_to_nothing_is_reported_instead_of_restarting_forever() {
    // Exits successfully without writing a frame: to the worker that looks
    // like "the file ended", which it would otherwise loop on forever.
    let (worker, frames_rx, errors_rx, control_tx, directory) =
        start_shim_decoder("empty", "exit 0\n", Duration::from_secs(10));
    let error = errors_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("an empty video must be reported");
    assert!(error.contains("could not decode any frames"), "{error}");
    assert!(frames_rx.try_recv().is_err());
    drop(control_tx);
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn a_slow_but_healthy_ffmpeg_is_not_mistaken_for_a_stall() {
    // The first frame arrives after 0.4 s, longer than a frame interval but
    // well inside the stall limit.
    let (worker, frames_rx, errors_rx, control_tx, directory) = start_shim_decoder(
        "slow",
        "sleep 0.4\nprintf 'abcdefghijklmnop'\nexit 0\n",
        Duration::from_secs(5),
    );
    let frame = frames_rx
        .recv_timeout(Duration::from_secs(4))
        .expect("the slow frame arrives");
    assert_eq!(frame.rgba, b"abcdefghijklmnop");
    assert!(errors_rx.try_recv().is_err(), "no false stall report");
    drop(control_tx);
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn shared_playback_update_restarts_a_stopped_video_decoder() {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-decoder-restart-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let program = directory.join("ffmpeg-shim");
    write_ffmpeg_shim(&program, "printf 'abcdefghijklmnop'\nexit 0\n");

    let (incoming_tx, incoming) = mpsc::channel();
    let (outgoing, _outgoing_rx) = tokio_mpsc::unbounded_channel();
    let alive = Arc::new(AtomicBool::new(false));
    let snapshot = RoomSnapshot::create(
        "restart-room".into(),
        "RETRY42".into(),
        "Recovery puzzle".into(),
        "a".repeat(64),
        "mp4".into(),
        2,
        2,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    let mut app = PieceOffApp {
        media_path: Some(directory.join("video.mp4")),
        media_extension: "mp4".into(),
        media_size: (2, 2),
        media_duration_ms: 1000,
        is_video: true,
        decoder_worker_alive: Some(alive.clone()),
        decoder_ffmpeg_program: program.into_os_string(),
        network: Some(Network { outgoing, incoming }),
        snapshot: Some(snapshot),
        ..PieceOffApp::default()
    };
    incoming_tx
        .send(ServerMessage::PlaybackChanged {
            playback: PlaybackState {
                playing: false,
                position_ms: 0,
                rate: 1.0,
                loop_start_ms: 0,
                loop_end_ms: 1000,
                changed_at_ms: 0,
            },
            revision: 1,
        })
        .unwrap();

    app.process_network(&egui::Context::default());

    assert_eq!(app.decoder_restarts_after_error, 1);
    let restarted_alive = app.decoder_worker_alive.as_ref().unwrap().clone();
    assert!(!Arc::ptr_eq(&alive, &restarted_alive));
    let frame = app
        .decoder_frames
        .as_ref()
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .expect("restarted decoder should provide a frame");
    assert_eq!(frame.rgba, b"abcdefghijklmnop");
    assert!(restarted_alive.load(Ordering::Acquire));
    app.note_decoder_frame_received();
    assert!(!app.decoder_recovering);
    assert_eq!(app.status, "Video playback recovered.");

    drop(app);
    let deadline = Instant::now() + Duration::from_secs(1);
    while restarted_alive.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!restarted_alive.load(Ordering::Acquire));
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn audio_resyncs_to_the_shared_position_after_a_decoder_restart() {
    use std::sync::Mutex;

    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-audio-resync-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let program = directory.join("ffmpeg-shim");
    write_ffmpeg_shim(&program, "printf 'abcdefghijklmnop'\nexit 0\n");

    let launches: Arc<Mutex<Vec<(bool, u64, f32)>>> = Arc::default();
    let recorder = launches.clone();
    let launcher: AudioLauncher = Arc::new(move |_path, command| {
        recorder
            .lock()
            .unwrap()
            .push((command.playing, command.position_ms, command.rate));
        None
    });
    let (incoming_tx, incoming) = mpsc::channel();
    let (outgoing, _outgoing_rx) = tokio_mpsc::unbounded_channel();
    let (old_audio_tx, old_audio_rx) = mpsc::channel::<AudioCommand>();
    let snapshot = RoomSnapshot::create(
        "audio-room".into(),
        "AUDIO42".into(),
        "Audio puzzle".into(),
        "a".repeat(64),
        "mp4".into(),
        2,
        2,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    let mut app = PieceOffApp {
        media_path: Some(directory.join("video.mp4")),
        media_extension: "mp4".into(),
        media_size: (2, 2),
        media_duration_ms: 60_000,
        media_has_audio: true,
        is_video: true,
        decoder_worker_alive: Some(Arc::new(AtomicBool::new(false))),
        decoder_ffmpeg_program: program.into_os_string(),
        audio_launcher: Some(launcher),
        audio_control: Some(old_audio_tx),
        network: Some(Network { outgoing, incoming }),
        snapshot: Some(snapshot),
        ..PieceOffApp::default()
    };
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    incoming_tx
        .send(ServerMessage::PlaybackChanged {
            playback: PlaybackState {
                playing: true,
                position_ms: 20_000,
                rate: 1.0,
                loop_start_ms: 0,
                loop_end_ms: 60_000,
                changed_at_ms: now,
            },
            revision: 1,
        })
        .unwrap();

    let ctx = egui::Context::default();
    app.process_network(&ctx);

    // The stopped decoder was restarted, flagged as recovering, and the
    // previous audio worker lost its control channel (so it stops playing).
    assert!(app.decoder_recovering);
    assert!(matches!(
        old_audio_rx.recv_timeout(Duration::from_secs(1)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    // The fresh audio worker launches at the shared position, not at 0.
    let deadline = Instant::now() + Duration::from_secs(3);
    let first = loop {
        if let Some(first) = launches.lock().unwrap().first().copied() {
            break first;
        }
        assert!(Instant::now() < deadline, "audio never relaunched");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(first.0, "audio resumes playing");
    assert!(
        (20_000..22_000).contains(&first.1),
        "audio resynced to ~20s of shared time, got {}ms",
        first.1
    );
    assert_eq!(first.2, 1.0);

    // A frame from the restarted decoder clears the recovery status via
    // the same path the UI uses every update.
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.decoder_recovering && Instant::now() < deadline {
        app.drain_decoder_frames(&ctx);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.decoder_recovering);
    assert_eq!(app.status, "Video playback recovered.");
    assert!(app.last_video_frame.is_some());

    drop(app);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn decoder_errors_are_shown_in_the_app_status() {
    let (error_tx, error_rx) = mpsc::channel();
    let mut app = PieceOffApp {
        decoder_errors: Some(error_rx),
        ..PieceOffApp::default()
    };
    error_tx
        .send("FFmpeg stopped while decoding the video.".into())
        .unwrap();

    app.poll_decoder_errors();

    assert_eq!(app.status, "FFmpeg stopped while decoding the video.");
}

#[test]
fn native_decoder_retries_transient_operations_and_bounds_persistent_failures() {
    let mut attempts = 0;
    let recovered = retry_decoder_operation(|| {
        attempts += 1;
        if attempts == 1 {
            Err("temporary decode error".into())
        } else {
            Ok(42)
        }
    });
    assert_eq!(recovered.unwrap(), 42);
    assert_eq!(attempts, 2);

    let mut attempts = 0;
    let failed: Result<(), _> = retry_decoder_operation(|| {
        attempts += 1;
        Err("persistent decode error".into())
    });
    assert_eq!(failed.unwrap_err(), "persistent decode error");
    assert_eq!(attempts, usize::from(MAX_DECODER_RECOVERY_ATTEMPTS) + 1);
}

#[test]
fn leaving_a_room_resets_decoder_retry_budget_and_keeps_local_media() {
    let path = PathBuf::from("local-video.mp4");
    let mut app = PieceOffApp {
        media_path: Some(path.clone()),
        decoder_restarts_after_error: MAX_DECODER_RECOVERY_ATTEMPTS,
        ..PieceOffApp::default()
    };

    app.leave_room();

    assert_eq!(app.decoder_restarts_after_error, 0);
    assert_eq!(app.media_path.as_ref(), Some(&path));
}

#[test]
fn reconnect_retry_uses_bounded_exponential_backoff() {
    assert_eq!(reconnect_delay(0), Duration::from_secs(1));
    assert_eq!(reconnect_delay(1), Duration::from_secs(2));
    assert_eq!(reconnect_delay(3), Duration::from_secs(8));
    assert_eq!(reconnect_delay(5), Duration::from_secs(30));
    assert_eq!(reconnect_delay(u8::MAX), Duration::from_secs(30));
}

#[test]
fn reconnect_clears_stale_drag_and_pan_gestures() {
    let mut app = PieceOffApp {
        dragging: Some(ActiveDrag {
            anchor_piece_id: 7,
            pointer_offset: Vec2::new(3.0, 5.0),
            origins: vec![(7, 10.0, 12.0)],
        }),
        pan_drag_last: Some(Pos2::new(21.0, 34.0)),
        ..Default::default()
    };

    app.clear_active_gesture();

    assert!(app.dragging.is_none());
    assert!(app.pan_drag_last.is_none());
}

#[test]
fn grid_dimension_controls_respect_the_server_piece_limit() {
    for other in 2..=30 {
        let max = max_grid_dimension_for_other(other);
        assert!((2..=30).contains(&max));
        assert!(max * other <= MAX_PUZZLE_PIECES);
        if max < 30 {
            assert!((max + 1) * other > MAX_PUZZLE_PIECES);
        }
    }
    assert_eq!(max_grid_dimension_for_other(2), 30);
    assert_eq!(max_grid_dimension_for_other(30), 13);
}

#[test]
fn media_preview_fits_source_aspect_ratio_inside_the_board() {
    let bounds = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(500.0, 400.0));
    let fitted = fit_media_rect(bounds, Vec2::new(1920.0, 1080.0));
    assert!(bounds.contains(fitted.min));
    assert!(bounds.contains(fitted.max));
    assert!((fitted.width() / fitted.height() - 1920.0 / 1080.0).abs() < 0.001);
    assert_eq!(fitted.center(), bounds.center());
}

#[test]
fn still_image_inspection_returns_dimensions_and_fingerprint() {
    let path = std::env::temp_dir().join(format!("pieceoff-media-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(17, 11, image::Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();
    assert!(!png_is_animated(&path).unwrap());
    let metadata = inspect_media(&path).unwrap();
    assert_eq!(metadata.size, (17, 11));
    assert_eq!(metadata.duration_ms, 0);
    assert!(!metadata.is_video);
    assert!(!metadata.has_audio);
    assert_eq!(metadata.sha256.len(), 64);
    remove_file_when_released(&path);
}

#[test]
fn oversized_gif_is_rejected_before_frame_decode() {
    let path = std::env::temp_dir().join(format!("pieceoff-oversized-{}.gif", std::process::id()));
    let mut encoded = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut encoded)
        .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([20, 40, 60, 255]),
        )))
        .unwrap();
    encoded[6..8].copy_from_slice(&8000_u16.to_le_bytes());
    encoded[8..10].copy_from_slice(&6000_u16.to_le_bytes());
    std::fs::write(&path, encoded).unwrap();

    let error = inspect_media(&path).unwrap_err();
    assert!(error.contains("40 megapixels"), "unexpected error: {error}");
    remove_file_when_released(&path);
}

#[test]
fn animated_gif_is_detected_for_looping_video_playback() {
    let path = std::env::temp_dir().join(format!("pieceoff-animated-{}.gif", std::process::id()));
    let frames = [[255, 0, 0, 255], [0, 0, 255, 255]]
        .into_iter()
        .map(|color| {
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(4, 4, image::Rgba(color)),
                0,
                0,
                image::Delay::from_numer_denom_ms(100, 1),
            )
        });
    image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap())
        .encode_frames(frames)
        .unwrap();
    assert!(gif_is_animated(&path).unwrap());
    remove_file_when_released(&path);
}

#[test]
fn animated_webp_native_decoder_seeks_and_loops() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("Skipping animated WebP integration test: ffmpeg is not installed.");
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "pieceoff-animated-native-{}.webp",
        std::process::id()
    ));
    if !ffmpeg_has_encoder("libwebp_anim") {
        eprintln!("skipping: this ffmpeg build has no libwebp_anim encoder");
        return;
    }
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=4x4:r=5:d=0.4",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=4x4:r=5:d=0.4",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0,format=bgra[v]",
            "-map",
            "[v]",
            "-c:v",
            "libwebp_anim",
            "-lossless",
            "1",
            "-loop",
            "0",
            "-y",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg should generate an animated WebP");
    assert!(generated.success(), "ffmpeg failed to generate WebP");

    let (animated, dimensions, duration_ms) = webp_animation_metadata(&path).unwrap();
    assert!(animated);
    assert_eq!(dimensions, (4, 4));
    assert!(duration_ms >= 600, "duration was {duration_ms} ms");
    let metadata = inspect_media(&path).unwrap();
    assert!(metadata.is_video);
    assert_eq!(metadata.size, dimensions);
    assert_eq!(metadata.duration_ms, duration_ms);

    let (control_tx, control_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    let (error_tx, error_rx) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(false));
    let worker_running = running.clone();
    let worker_path = path.clone();
    let worker = AnimatedWebpDecoderBackend.start(DecoderStartup {
        path: worker_path,
        duration_ms,
        width: 4,
        height: 4,
        fps: DEFAULT_VIDEO_FPS,
        control_rx,
        frames: frame_tx,
        errors: error_tx,
        repaint: egui::Context::default(),
        running: worker_running,
        worker_alive: Arc::new(AtomicBool::new(true)),
    });

    let first = frame_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        first.rgba[0] > 180 && first.rgba[2] < 80,
        "first frame is red"
    );
    control_tx
        .send(DecoderCommand {
            playing: true,
            position_ms: duration_ms.saturating_sub(20),
            rate: 1.0,
            loop_start_ms: 0,
            loop_end_ms: duration_ms,
            resync: false,
            issued_at: Instant::now(),
        })
        .unwrap();
    let last = frame_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        last.rgba[2] > 180 && last.rgba[0] < 80,
        "last frame is blue"
    );
    let wrapped = frame_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        wrapped.rgba[0] > 180 && wrapped.rgba[2] < 80,
        "loop returns to red"
    );
    assert!(error_rx.try_recv().is_err());
    drop(control_tx);
    worker.join().unwrap();
    remove_file_when_released(&path);
}

#[test]
fn all_bundled_puzzle_loops_are_valid_seamless_native_animated_webp() {
    use image::AnimationDecoder;

    for (name, bytes) in [
        (
            "Slow Orbit",
            include_bytes!("../assets/builtin/Slow Orbit.webp").as_slice(),
        ),
        (
            "Tidal Glass",
            include_bytes!("../assets/builtin/Tidal Glass.webp").as_slice(),
        ),
        (
            "Aurora Veil",
            include_bytes!("../assets/builtin/Aurora Veil.webp").as_slice(),
        ),
        (
            "Dune Light",
            include_bytes!("../assets/builtin/Dune Light.webp").as_slice(),
        ),
        (
            "Leaf Garden",
            include_bytes!("../assets/builtin/Leaf Garden.webp").as_slice(),
        ),
        (
            "Glass Garden",
            include_bytes!("../assets/builtin/Glass Garden.webp").as_slice(),
        ),
        (
            "Deep Sea Bloom",
            include_bytes!("../assets/builtin/Deep Sea Bloom.webp").as_slice(),
        ),
        (
            "Geometric Bloom",
            include_bytes!("../assets/builtin/Geometric Bloom.webp").as_slice(),
        ),
    ] {
        let path = std::env::temp_dir().join(format!(
            "pieceoff-builtin-{}-{}.webp",
            name.replace(' ', "-"),
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let (animated, dimensions, duration_ms) = webp_animation_metadata(&path).unwrap();
        assert!(animated, "{name} should loop as animated WebP");
        assert_eq!(dimensions, (480, 320));
        assert!(
            (5_900..=6_100).contains(&duration_ms),
            "{name} duration: {duration_ms} ms"
        );

        let file = std::fs::File::open(&path).unwrap();
        let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(file)).unwrap();
        let frames: Vec<_> = decoder.into_frames().map(Result::unwrap).collect();
        assert!(frames.len() >= 60, "{name} frame count: {}", frames.len());
        let first = frames.first().unwrap().buffer().as_raw();
        let second = frames.get(1).unwrap().buffer().as_raw();
        let last = frames.last().unwrap().buffer().as_raw();
        let mean_difference = |a: &[u8], b: &[u8]| {
            a.iter()
                .zip(b)
                .map(|(left, right)| left.abs_diff(*right) as u64)
                .sum::<u64>() as f64
                / a.len() as f64
        };
        let seam_step = mean_difference(last, first);
        let normal_step = mean_difference(first, second);
        assert!(seam_step < 20.0, "{name} loop seam jumps by {seam_step:.2}");
        assert!(
            seam_step < normal_step * 2.5 + 1.0,
            "{name} seam ({seam_step:.2}) is much larger than a normal step ({normal_step:.2})"
        );
        remove_file_when_released(&path);
    }
}

#[test]
fn animated_png_is_detected_for_looping_video_playback() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("Skipping APNG integration test: ffmpeg is not installed.");
        return;
    }
    let path = std::env::temp_dir().join(format!("pieceoff-animated-{}.png", std::process::id()));
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=4x4:r=5:d=0.4",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=4x4:r=5:d=0.4",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0,format=rgba[v]",
            "-map",
            "[v]",
            "-plays",
            "0",
            "-f",
            "apng",
            "-y",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg should generate a test APNG");
    assert!(generated.success(), "ffmpeg failed to generate APNG");
    assert!(png_is_animated(&path).unwrap());
    let metadata = inspect_media(&path).unwrap();
    assert!(metadata.is_video);
    assert_eq!(metadata.size, (4, 4));
    assert!(supported_media_extension(&path).is_ok());
    let timeline = DecoderCommand {
        playing: true,
        position_ms: 0,
        rate: 1.0,
        loop_start_ms: 0,
        loop_end_ms: metadata.duration_ms,
        resync: false,
        issued_at: Instant::now(),
    };
    let process = start_ffmpeg(&path, 4, 4, &timeline)
        .expect("PieceOff's FFmpeg path should decode APNG frames");
    let mut process = Some(process);
    let mut frame = vec![0; 4 * 4 * 4];
    process.as_mut().unwrap().1.read_exact(&mut frame).unwrap();
    assert!(frame[0] > 180 && frame[2] < 80, "first APNG frame is red");
    let mut saw_blue = false;
    for _ in 0..24 {
        process.as_mut().unwrap().1.read_exact(&mut frame).unwrap();
        if frame[2] > 180 && frame[0] < 80 {
            saw_blue = true;
            break;
        }
    }
    stop_ffmpeg(&mut process);
    assert!(
        saw_blue,
        "PieceOff's FFmpeg path reaches the second APNG frame"
    );
    remove_file_when_released(&path);
}

#[test]
fn media_import_accepts_common_ffmpeg_video_containers() {
    for extension in [
        "mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "ogv", "flv", "3gp", "wmv", "ts",
        "mts", "m2ts",
    ] {
        assert_eq!(supported_extension(extension), Some(extension));
        let path = PathBuf::from(format!("clip.{extension}"));
        assert_eq!(supported_media_extension(&path).unwrap(), extension);
    }
    assert_eq!(supported_extension("apng"), Some("apng"));
    assert_eq!(
        supported_media_extension(Path::new("animation.APNG")).unwrap(),
        "apng"
    );
    assert_eq!(
        supported_media_extension(Path::new("clip.M4V")).unwrap(),
        "m4v"
    );
    assert!(supported_extension("../mp4").is_none());
    assert!(supported_extension("exe").is_none());
}

#[test]
fn video_decode_resolution_preserves_aspect_and_bounds_frame_memory() {
    assert_eq!(video_decode_dimensions(1280, 720), (1280, 720));
    assert_eq!(video_decode_dimensions(3840, 2160), (1920, 1080));
    assert_eq!(video_decode_dimensions(2160, 3840), (607, 1080));
    assert_eq!(video_decode_dimensions(8192, 1), (1920, 1));
    assert_eq!(video_decode_dimensions(0, 0), (0, 0));
}

#[test]
fn playback_clock_applies_rate_and_wraps_inside_loop_segment() {
    let playback = PlaybackState {
        playing: true,
        position_ms: 2500,
        rate: 2.0,
        loop_start_ms: 1000,
        loop_end_ms: 3000,
        changed_at_ms: 5000,
    };
    assert_eq!(estimated_position_at(&playback, 6000), 2500);
}

#[test]
fn video_seek_shortcuts_stay_inside_the_active_loop() {
    assert_eq!(seek_loop_by(12_000, -5_000, 10_000, 30_000), 10_000);
    assert_eq!(seek_loop_by(12_000, 5_000, 10_000, 30_000), 17_000);
    assert_eq!(seek_loop_by(29_000, 5_000, 10_000, 30_000), 29_999);
    assert_eq!(seek_loop_by(0, 5_000, 10_000, 10_000), 10_000);
}

#[test]
fn drift_measurement_treats_loop_edges_as_adjacent() {
    assert_eq!(looped_position_distance_ms(4990, 1010, 1000, 5000), 20);
    assert_eq!(looped_position_distance_ms(2200, 2500, 1000, 5000), 300);
    assert_eq!(looped_position_distance_ms(200, 500, 0, 0), 300);
}

#[test]
fn audio_player_arguments_follow_the_shared_loop_and_speed_with_local_volume() {
    let command = AudioCommand {
        playing: true,
        position_ms: 1500,
        rate: 0.25,
        loop_start_ms: 1000,
        loop_end_ms: 4500,
        volume: 75,
        muted: false,
        resync: false,
        issued_at: Instant::now(),
    };
    let (seek, args) = audio_decode_args(&command, command.position_ms, 48_000, 2);
    assert_eq!(seek, "1.500");
    // 3000 ms of media at quarter speed is 12 s of output.
    assert!(args.windows(2).any(|pair| pair == ["-t", "12.000"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-af", "atempo=0.5,atempo=0.500"])
    );
    assert!(args.windows(2).any(|pair| pair == ["-ar", "48000"]));
    assert!(args.windows(2).any(|pair| pair == ["-ac", "2"]));
    // Past the loop end, the next pass starts at the loop start.
    assert_eq!(audio_decode_args(&command, 9000, 48_000, 2).0, "1.000");
    assert_eq!(volume_gain(75, true), 0.0);
    assert!(volume_gain(100, false) > volume_gain(50, false));
    assert_eq!(atempo_filter(1.5), "atempo=1.500");
}

#[test]
fn room_media_http_transfer_uploads_and_downloads_verified_bytes() {
    let _serial = crate::package::transfer_test_lock();
    let content = b"PieceOff room media transfer test".to_vec();
    let hash = format!("{:x}", Sha256::digest(&content));
    let source = std::env::temp_dir().join(format!("pieceoff-upload-{hash}.png"));
    std::fs::write(&source, &content).unwrap();
    let cached = std::env::temp_dir()
        .join("pieceoff-media-cache")
        .join(format!("{hash}.png"));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_content = content.clone();
    let server_hash = hash.clone();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let (request, length) = read_test_request(&mut reader);
        assert!(request.starts_with("PUT /media/"));
        assert!(request.contains(&server_hash));
        assert!(request.contains("token=upload-token"));
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        assert_eq!(body, server_content);
        reader
            .get_mut()
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();

        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let (request, _) = read_test_request(&mut reader);
        assert!(request.starts_with("GET /media/"));
        assert!(request.contains(&server_hash));
        assert!(request.contains("code=JOIN42"));
        write!(
            reader.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            server_content.len()
        )
        .unwrap();
        reader.get_mut().write_all(&server_content).unwrap();
    });
    let url = format!("ws://{address}/ws");
    upload_room_media(&url, &hash, "upload-token", &source).unwrap();
    let downloaded = download_room_media(&url, "JOIN42", &hash, "png").unwrap();
    assert_eq!(std::fs::read(&downloaded).unwrap(), content);
    server.join().unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(cached).unwrap();
}

#[test]
fn secure_websocket_url_maps_to_https_media_endpoint() {
    assert_eq!(
        http_media_url("wss://puzzle.example.com/ws", "/media/abc?code=JOIN42").unwrap(),
        "https://puzzle.example.com/media/abc?code=JOIN42"
    );
    assert_eq!(
        http_media_url("ws://[::1]:8787/ws", "/health").unwrap(),
        "http://[::1]:8787/health"
    );
    assert!(http_media_url("https://puzzle.example.com/ws", "/health").is_err());
    assert!(http_media_url("ws://user@host/ws", "/health").is_err());
}

fn read_test_request(reader: &mut BufReader<TcpStream>) -> (String, usize) {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let request = std::mem::take(&mut line);
    let mut length = 0;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap();
        }
    }
    (request, length)
}

#[test]
fn ffprobe_metadata_finds_audio_when_it_precedes_the_video_stream() {
    let value = serde_json::json!({
        "streams": [
            {"codec_type": "audio"},
            {"codec_type": "video", "width": 1920, "height": 1080}
        ],
        "format": {"duration": "12.375"}
    });
    assert_eq!(
        parse_video_metadata(&value).unwrap(),
        ((1920, 1080), 12_375, true)
    );
}

#[test]
fn ffmpeg_probes_decodes_loops_and_seeks_a_generated_video() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("Skipping FFmpeg smoke test: ffmpeg is not installed.");
        return;
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pieceoff-ffmpeg-smoke-{}-{nonce}.mp4",
        std::process::id()
    ));
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=32x24:r=20:d=0.5",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=32x24:r=20:d=0.5",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p[v]",
            "-map",
            "[v]",
            "-c:v",
            "mpeg4",
            "-threads",
            "1",
            "-y",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg should generate a smoke-test clip");
    assert!(generated.success(), "ffmpeg failed to generate a clip");

    let metadata = inspect_media(&path).unwrap();
    assert_eq!(metadata.size, (32, 24));
    assert!(metadata.duration_ms >= 900);
    assert!(metadata.is_video);
    assert!(!metadata.has_audio);
    assert_eq!(metadata.sha256.len(), 64);

    let timeline = DecoderCommand {
        playing: true,
        position_ms: 0,
        rate: 2.0,
        loop_start_ms: 0,
        loop_end_ms: metadata.duration_ms,
        resync: false,
        issued_at: Instant::now(),
    };
    let mut process = start_ffmpeg(&path, 32, 24, &timeline)
        .expect("ffmpeg should start decoding the generated clip");
    let mut frame = vec![0; 32 * 24 * 4];
    process.1.read_exact(&mut frame).unwrap();
    stop_ffmpeg(&mut Some(process));
    assert!(
        frame[0] > 200,
        "decoded frame should be red: {:?}",
        &frame[..4]
    );
    assert!(
        frame[1] < 60,
        "decoded frame should be red: {:?}",
        &frame[..4]
    );
    assert!(
        frame[2] < 60,
        "decoded frame should be red: {:?}",
        &frame[..4]
    );

    let mut paused_app = PieceOffApp {
        media_size: metadata.size,
        ..PieceOffApp::default()
    };
    paused_app.start_video_decoder(path.clone(), false, egui::Context::default());
    let paused_frame = paused_app
        .decoder_frames
        .as_ref()
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .expect("paused video should publish a frame for the puzzle board");
    assert_eq!((paused_frame.width, paused_frame.height), (32, 24));
    assert!(paused_frame.rgba[0] > 200);
    let frames = paused_app.decoder_frames.as_ref().unwrap();
    let controls = paused_app.decoder_control.as_ref().unwrap();
    controls
        .send(DecoderCommand {
            playing: true,
            position_ms: 0,
            rate: 2.0,
            loop_start_ms: 0,
            loop_end_ms: metadata.duration_ms,
            resync: false,
            issued_at: Instant::now(),
        })
        .unwrap();

    let mut saw_blue = false;
    let mut looped_back_to_red = false;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Ok(frame) = frames.recv_timeout(Duration::from_millis(250)) {
            let pixel = &frame.rgba[..4];
            let is_blue = pixel[2] > 180 && pixel[0] < 80;
            let is_red = pixel[0] > 180 && pixel[2] < 80;
            saw_blue |= is_blue;
            if saw_blue && is_red {
                looped_back_to_red = true;
                break;
            }
        }
    }
    assert!(saw_blue, "decoder should reach the blue part of the clip");
    assert!(
        looped_back_to_red,
        "decoder should restart at the loop-in point"
    );

    while frames.try_recv().is_ok() {}
    controls
        .send(DecoderCommand {
            playing: false,
            position_ms: 600,
            rate: 1.0,
            loop_start_ms: 0,
            loop_end_ms: metadata.duration_ms,
            resync: false,
            issued_at: Instant::now(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut seek_showed_blue = false;
    while Instant::now() < deadline {
        if let Ok(frame) = frames.recv_timeout(Duration::from_millis(250)) {
            let pixel = &frame.rgba[..4];
            if pixel[2] > 180 && pixel[0] < 80 {
                seek_showed_blue = true;
                break;
            }
        }
    }
    assert!(
        seek_showed_blue,
        "seeking to 600 ms should show the blue clip segment"
    );
    assert!(
        paused_app
            .decoder_errors
            .as_ref()
            .unwrap()
            .try_recv()
            .is_err()
    );
    drop(paused_app);
    remove_file_when_released(&path);
}

#[test]
fn still_frame_capture_is_immutable_when_live_frames_advance() {
    let mut live = ColorImage::new([1, 1], vec![Color32::from_rgb(10, 20, 30)]);
    let frozen = capture_still_frame(&live);
    live.pixels[0] = Color32::from_rgb(200, 210, 220);
    assert_eq!(frozen.pixels[0], Color32::from_rgb(10, 20, 30));
    assert_eq!(live.pixels[0], Color32::from_rgb(200, 210, 220));
}

#[test]
fn board_zoom_stays_anchored_to_the_pointer_and_clamps() {
    let center = Pos2::new(500.0, 350.0);
    let pointer = Pos2::new(620.0, 290.0);
    let pan = Vec2::new(14.0, -8.0);
    let (zoom, next_pan) = zoom_at_pointer(1.0, pan, center, pointer, 100.0);
    let old_local = (pointer - (center + pan)) / 1.0;
    let new_local = (pointer - (center + next_pan)) / zoom;
    assert!((old_local.x - new_local.x).abs() < 0.001);
    assert!((old_local.y - new_local.y).abs() < 0.001);
    assert_eq!(zoom_at_pointer(8.0, pan, center, pointer, 100.0).0, 8.0);
    assert_eq!(
        zoom_at_pointer(MIN_ZOOM, pan, center, pointer, -100.0).0,
        MIN_ZOOM
    );
}

#[test]
fn ghost_setting_defaults_off_and_persists() {
    let dir = std::env::temp_dir().join(format!("pieceoff-settings-{}", std::process::id()));
    let path = dir.join("settings.json");
    assert!(!load_settings(&path).show_ghost, "missing file means off");
    save_settings(
        &path,
        &Settings {
            show_ghost: true,
            ..Settings::default()
        },
    );
    assert!(load_settings(&path).show_ghost);
    std::fs::write(&path, b"{}").unwrap();
    assert!(!load_settings(&path).show_ghost, "absent key means off");
    std::fs::write(&path, b"not json").unwrap();
    assert!(!load_settings(&path).show_ghost, "corrupt file means off");
    let _ = std::fs::remove_dir_all(dir);
}

/// Drives the real board UI with synthetic pointer events, with the core
/// room logic standing in for the server.
struct DragRig {
    ctx: egui::Context,
    app: PieceOffApp,
    room: pieceoff_core::RoomRuntime,
    outgoing_rx: tokio_mpsc::UnboundedReceiver<ClientMessage>,
    incoming_tx: mpsc::Sender<ServerMessage>,
    time: f64,
    modifiers: egui::Modifiers,
    board_min: Pos2,
    scale: f32,
    cw: f32,
    ch: f32,
    messages: usize,
    /// The host clock the room is given, in ms since the Unix epoch.
    room_clock_ms: u64,
    /// Our pointer as sent to the room; `None` entries are "left the table".
    cursors_sent: Vec<Option<Pos2>>,
    chats_sent: Vec<String>,
    pings_sent: Vec<Pos2>,
}

impl DragRig {
    fn new(rows: u16, columns: u16) -> Self {
        use pieceoff_core::{RoomRuntime, piece_outline};
        let ctx = egui::Context::default();
        let snapshot = RoomSnapshot::create(
            "room".into(),
            "INVITE".into(),
            "Drag test".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            rows,
            columns,
            "host".into(),
        )
        .unwrap();
        let mut room = RoomRuntime::default();
        room.snapshot = Some(snapshot.clone());
        room.host_id = Some("host".into());
        room.names.insert("host".into(), "Host".into());
        let (outgoing, outgoing_rx) = tokio_mpsc::unbounded_channel();
        let (incoming_tx, incoming) = mpsc::channel();
        let texture = ctx.load_texture(
            "test",
            ColorImage::new([2, 2], vec![Color32::WHITE; 4]),
            TextureOptions::LINEAR,
        );
        let app = PieceOffApp {
            snapshot: Some(snapshot.clone()),
            client_id: Some("host".into()),
            connected: true,
            texture: Some(texture),
            network: Some(Network { outgoing, incoming }),
            ..PieceOffApp::default()
        };
        let mut rig = Self {
            ctx,
            app,
            room,
            outgoing_rx,
            incoming_tx,
            time: 0.0,
            modifiers: egui::Modifiers::NONE,
            board_min: Pos2::ZERO,
            scale: 1.0,
            cw: snapshot.cell_width(),
            ch: snapshot.cell_height(),
            messages: 0,
            room_clock_ms: board::wall_clock_ms(),
            cursors_sent: Vec::new(),
            chats_sent: Vec::new(),
            pings_sent: Vec::new(),
        };
        // Learn the on-screen mapping from piece 0's mesh.
        let output = rig.frame(Vec::new());
        let mesh = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                // Untextured meshes are drop shadows.
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.clone())
                }
                _ => None,
            })
            .expect("a piece mesh");
        let bounds = |points: &mut dyn Iterator<Item = Pos2>| {
            let mut rect = Rect::NOTHING;
            points.for_each(|point| rect.extend_with(point));
            rect
        };
        let mesh_bounds = bounds(&mut mesh.vertices.iter().map(|v| v.pos));
        let first = snapshot.piece(0).unwrap();
        let outline = bounds(
            &mut piece_outline(&snapshot, first)
                .into_iter()
                .map(|p| Pos2::new(p[0], p[1])),
        );
        rig.scale = mesh_bounds.width() / outline.width();
        rig.board_min = mesh_bounds.min
            - Vec2::new(first.x + outline.min.x, first.y + outline.min.y) * rig.scale;
        assert!(rig.scale > 0.1, "scale {}", rig.scale);
        rig
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(self.time),
            modifiers: self.modifiers,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.render_board(ui));
        })
    }

    fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        self.board_min + Vec2::new(x, y) * self.scale
    }

    fn piece(&self, id: u32) -> Piece {
        self.app
            .snapshot
            .as_ref()
            .unwrap()
            .piece(id)
            .unwrap()
            .clone()
    }

    /// Pass the app's pending messages to the room and its answers back.
    fn relay(&mut self) {
        while let Ok(message) = self.outgoing_rx.try_recv() {
            match message {
                // Live cursors, chat and pings are relayed, not room changes.
                ClientMessage::Cursor { x, y } => {
                    self.cursors_sent.push(Some(Pos2::new(x, y)));
                    continue;
                }
                ClientMessage::CursorHidden => {
                    self.cursors_sent.push(None);
                    continue;
                }
                ClientMessage::Chat { text } => {
                    self.chats_sent.push(text);
                    continue;
                }
                ClientMessage::Ping { x, y } => {
                    self.pings_sent.push(Pos2::new(x, y));
                    continue;
                }
                // The host answers this to the asker alone.
                ClientMessage::RequestTimelapse => {
                    let timelapse = self.room.timelapse.clone();
                    self.incoming_tx
                        .send(ServerMessage::Timelapse { timelapse })
                        .unwrap();
                    continue;
                }
                _ => {}
            }
            let outcome = self
                .room
                .apply("host", message, self.room_clock_ms)
                .expect("room accepts the message");
            self.incoming_tx.send(outcome.event).unwrap();
            self.messages += 1;
        }
        self.app.process_network(&self.ctx.clone());
    }

    fn button(&mut self, pos: Pos2, pressed: bool, modifiers: egui::Modifiers) {
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }]);
    }

    /// Press on the centre of `grab`'s cell, drag to `(x, y)` in source
    /// pixels (where that cell's top-left should end up), release.
    fn drag(&mut self, grab: u32, x: f32, y: f32, modifiers: egui::Modifiers) {
        self.modifiers = modifiers;
        let piece = self.piece(grab);
        let start = self.to_screen(piece.x + self.cw / 2.0, piece.y + self.ch / 2.0);
        let end = self.to_screen(x + self.cw / 2.0, y + self.ch / 2.0);
        self.frame(vec![egui::Event::PointerMoved(start)]);
        self.button(start, true, modifiers);
        for step in 1..=10 {
            let pos = start + (end - start) * (step as f32 / 10.0);
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.relay();
        }
        self.button(end, false, modifiers);
        self.relay();
        self.modifiers = egui::Modifiers::NONE;
        self.frame(Vec::new());
    }

    fn drag_to_target(&mut self, id: u32) {
        let piece = self.piece(id);
        self.drag(
            id,
            piece.column as f32 * self.cw,
            piece.row as f32 * self.ch,
            egui::Modifiers::NONE,
        );
    }

    fn click_select(&mut self, id: u32) {
        let piece = self.piece(id);
        let pos = self.to_screen(piece.x + self.cw / 2.0, piece.y + self.ch / 2.0);
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        let ctrl = egui::Modifiers::COMMAND;
        self.modifiers = ctrl;
        self.button(pos, true, ctrl);
        self.button(pos, false, ctrl);
        self.modifiers = egui::Modifiers::NONE;
    }
}

#[test]
fn dragging_every_tray_piece_onto_its_target_solves_the_puzzle() {
    let mut rig = DragRig::new(3, 4);
    for id in 0..12 {
        rig.drag_to_target(id);
        assert!(rig.piece(id).placed, "piece {id} did not snap into place");
    }
    assert!(rig.messages >= 12 * 3, "got {} messages", rig.messages);
    assert!(rig.app.snapshot.as_ref().unwrap().completed());
}

#[test]
fn host_can_play_again_after_finishing() {
    let mut rig = DragRig::new(3, 4);
    for id in 0..12 {
        rig.drag_to_target(id);
    }
    let snapshot = rig.app.snapshot.as_ref().unwrap();
    assert!(snapshot.completed());
    assert!(snapshot.stats.finished, "the solve clock is stopped");
    // Click the real "Play again" button on the completion card, below the
    // one player row.
    let again = Pos2::new(675.0, 161.0);
    rig.frame(vec![egui::Event::PointerMoved(again)]);
    rig.frame(Vec::new());
    rig.button(again, true, egui::Modifiers::NONE);
    rig.button(again, false, egui::Modifiers::NONE);
    rig.frame(Vec::new());
    rig.relay();

    let snapshot = rig.app.snapshot.as_ref().unwrap();
    assert!(!snapshot.completed(), "everything is scattered again");
    assert!(snapshot.pieces.iter().all(|p| !p.placed));
    assert!(rig.app.win_dismissed_room.is_none());
    rig.frame(Vec::new());
    let stats = &rig.app.snapshot.as_ref().unwrap().stats;
    assert!(
        !stats.started && stats.players.is_empty(),
        "a restart begins a new run"
    );
    // And it is playable again.
    rig.drag_to_target(0);
    assert!(rig.piece(0).placed);
}

#[test]
fn arranging_the_tray_relays_loose_pieces_and_keeps_placed_ones() {
    let mut rig = DragRig::new(3, 4);
    rig.drag_to_target(0);
    rig.drag_to_target(5);
    assert!(rig.piece(0).placed && rig.piece(5).placed);
    let placed_at = (rig.piece(0).x, rig.piece(0).y);
    rig.app.selected_pieces.insert(3);

    rig.app.arrange_tray(TrayArrangement::EdgesFirst);
    rig.relay();
    rig.frame(Vec::new());

    assert!(
        rig.app.selected_pieces.is_empty(),
        "arranging clears the selection"
    );
    assert_eq!((rig.piece(0).x, rig.piece(0).y), placed_at);
    assert!(rig.piece(0).placed && rig.piece(5).placed);
    let loose: Vec<Piece> = rig
        .app
        .snapshot
        .as_ref()
        .unwrap()
        .pieces
        .iter()
        .filter(|piece| !piece.placed)
        .cloned()
        .collect();
    assert_eq!(loose.len(), 10);
    // Corners come first: piece 3 (top-right corner) sits in an earlier slot
    // than the interior piece 6.
    let slot = |piece: &Piece| (piece.y as i32, piece.x as i32);
    assert!(
        slot(rig.app.snapshot.as_ref().unwrap().piece(3).unwrap())
            < slot(rig.app.snapshot.as_ref().unwrap().piece(6).unwrap())
    );
}

#[test]
fn the_pointer_is_shared_in_board_coordinates_throttled_and_hidden_off_the_board() {
    let mut rig = DragRig::new(3, 4);
    rig.frame(vec![egui::Event::PointerMoved(rig.to_screen(50.0, 60.0))]);
    rig.relay();
    let sent = rig
        .cursors_sent
        .last()
        .copied()
        .flatten()
        .expect("a cursor");
    assert!(
        (sent.x - 50.0).abs() < 0.5 && (sent.y - 60.0).abs() < 0.5,
        "board coordinates, not screen: {sent:?}"
    );

    // A pointer that moves every 50 ms frame is sent at most 15 times a second.
    rig.cursors_sent.clear();
    for step in 0..20 {
        let pos = rig.to_screen(50.0 + step as f32, 60.0);
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
        rig.relay();
    }
    let sends = rig.cursors_sent.len();
    assert!((8..=16).contains(&sends), "sent {sends} updates in 1 s");
    // Holding still: the last held-back position goes out, then nothing.
    for _ in 0..3 {
        rig.frame(Vec::new());
        rig.relay();
    }
    let last = rig.cursors_sent.last().copied().flatten().unwrap();
    assert!((last.x - 69.0).abs() < 0.5, "the newest position: {last:?}");
    let settled = rig.cursors_sent.len();
    for _ in 0..5 {
        rig.frame(Vec::new());
        rig.relay();
    }
    assert_eq!(rig.cursors_sent.len(), settled, "a still pointer is quiet");

    // Leaving the window says so, once.
    rig.cursors_sent.clear();
    for _ in 0..4 {
        rig.frame(vec![egui::Event::PointerGone]);
        rig.relay();
    }
    assert_eq!(rig.cursors_sent, vec![None]);
}

#[test]
fn the_completion_card_shows_the_solve_time_and_who_placed_what() {
    let mut rig = DragRig::new(3, 4);
    rig.room.names.insert("guest".into(), "Bea".into());
    // Bea places two pieces from her own client.
    for id in [10, 11] {
        let piece = rig.piece(id);
        let (x, y) = (piece.column as f32 * rig.cw, piece.row as f32 * rig.ch);
        for message in [
            ClientMessage::DragStart { piece_id: id },
            ClientMessage::DragEnd { piece_id: id, x, y },
        ] {
            let outcome = rig.room.apply("guest", message, rig.room_clock_ms).unwrap();
            rig.incoming_tx.send(outcome.event).unwrap();
        }
    }
    rig.app.process_network(&rig.ctx.clone());
    assert!(rig.app.snapshot.as_ref().unwrap().stats.running());
    for id in 0..10 {
        if id == 9 {
            rig.room_clock_ms += 83_000;
        }
        rig.drag_to_target(id);
    }
    let output = rig.frame(Vec::new());
    let texts = painted_texts(&output);
    assert!(
        texts
            .iter()
            .any(|text| text == "All 12 pieces joined in 01:23"),
        "{texts:?}"
    );
    // The HUD clock is stopped at the same time.
    assert!(texts.iter().any(|text| text == "01:23"), "{texts:?}");
    let host = texts.iter().position(|text| text == "Host").unwrap();
    let bea = texts.iter().position(|text| text == "Bea").unwrap();
    assert!(host < bea, "most pieces first: {texts:?}");
    assert_eq!(texts[host + 1], "10 pieces");
    assert_eq!(texts[bea + 1], "2 pieces");
    let lines = board::completion_lines(
        rig.app.snapshot.as_ref().unwrap(),
        Some(Duration::from_secs(83)),
    );
    assert_eq!(lines.players[0].2, player_color("Host"));
    assert_eq!(lines.players[1].2, player_color("Bea"));

    // The timer setting hides the HUD clock but not the card.
    rig.app.settings.hide_solve_timer = true;
    let texts = painted_texts(&rig.frame(Vec::new()));
    assert!(!texts.iter().any(|text| text == "01:23"), "{texts:?}");
    assert!(texts.iter().any(|text| text.ends_with("in 01:23")));
}

#[test]
fn the_hud_clock_waits_for_the_first_pickup_then_runs_from_the_room_clock() {
    let mut rig = DragRig::new(3, 4);
    let texts = painted_texts(&rig.frame(Vec::new()));
    assert!(!texts.iter().any(|text| text.contains(':')), "{texts:?}");
    // The room clock started 65 s ago on the host.
    rig.room_clock_ms = board::wall_clock_ms() - 65_000;
    rig.drag(0, 2_000.0, 0.0, egui::Modifiers::NONE);
    let texts = painted_texts(&rig.frame(Vec::new()));
    assert!(texts.iter().any(|text| text == "01:05"), "{texts:?}");
}

#[test]
fn a_finished_library_puzzle_records_one_solve() {
    let mut rig = DragRig::new(3, 4);
    let mut entry = library::LibraryEntry {
        id: "0123456789abcdef".into(),
        title: "Shelf".into(),
        rows: 3,
        columns: 4,
        is_video: false,
        media_sha256: "a".repeat(64),
        progress_percent: 0,
        added_ms: 0,
        updated_ms: 0,
        package_bytes: 0,
        best_time_ms: Some(500_000),
        completions: 1,
    };
    std::fs::create_dir_all(&rig.app.library.dir).unwrap();
    std::fs::write(rig.app.library.package_path(&entry.id), b"").unwrap();
    rig.app.library.upsert(entry.clone()).unwrap();
    rig.app.library_entry_id = Some(entry.id.clone());
    for id in 0..12 {
        if id == 11 {
            rig.room_clock_ms += 90_000;
        }
        rig.drag_to_target(id);
    }
    rig.frame(Vec::new());
    rig.frame(Vec::new());
    entry = rig.app.library.entries[0].clone();
    assert_eq!(entry.completions, 2, "counted once");
    assert_eq!(entry.best_time_ms, Some(90_000));
    assert_eq!(entry.status_line(), "0% assembled · best 01:30 · solved 2×");
}

fn painted_texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn other_players_cursors_are_drawn_with_their_names_then_fade_or_hide() {
    let mut rig = DragRig::new(3, 4);
    let cursor = |client_id: &str, name: &str, x: f32| ServerMessage::Cursor {
        client_id: client_id.into(),
        name: name.into(),
        x,
        y: 40.0,
    };
    rig.incoming_tx.send(cursor("ada", "Ada", 30.0)).unwrap();
    // Our own pointer is never drawn, even if the server sent it.
    rig.incoming_tx.send(cursor("host", "Me", 30.0)).unwrap();
    rig.relay();
    let texts = painted_texts(&rig.frame(Vec::new()));
    assert!(texts.iter().any(|text| text == "Ada"), "{texts:?}");
    assert!(!texts.iter().any(|text| text == "Me"));
    assert_eq!(rig.app.cursors.remote.len(), 1);

    // It glides to a new position rather than jumping.
    let now = rig.time;
    rig.incoming_tx.send(cursor("ada", "Ada", 130.0)).unwrap();
    rig.relay();
    let ada = &rig.app.cursors.remote["ada"];
    let midway = ada.position(now + 0.03).x;
    assert!(midway > 30.0 && midway < 130.0, "{midway}");
    assert_eq!(ada.position(now + 0.5).x, 130.0);
    assert_eq!(ada.color, player_color("Ada"), "a stable colour per player");
    assert_ne!(player_color("ada"), player_color("bob"));

    // The local setting hides them without forgetting them.
    let shows_ada =
        |rig: &mut DragRig| painted_texts(&rig.frame(Vec::new())).contains(&"Ada".to_owned());
    rig.app.settings.hide_peer_cursors = true;
    assert!(!shows_ada(&mut rig));
    rig.app.settings.hide_peer_cursors = false;
    assert!(shows_ada(&mut rig));

    // Three seconds without news and it is gone.
    rig.time += 3.0;
    assert!(!shows_ada(&mut rig));
    assert!(rig.app.cursors.remote.is_empty());

    // "Left the board" removes one at once.
    rig.incoming_tx.send(cursor("bob", "Bob", 10.0)).unwrap();
    rig.relay();
    assert!(painted_texts(&rig.frame(Vec::new())).contains(&"Bob".to_owned()));
    rig.incoming_tx
        .send(ServerMessage::CursorHidden {
            client_id: "bob".into(),
        })
        .unwrap();
    rig.relay();
    assert!(rig.app.cursors.remote.is_empty());
}

#[test]
fn other_players_cursors_setting_defaults_on_and_persists() {
    let dir = std::env::temp_dir().join(format!("pieceoff-cursor-settings-{}", std::process::id()));
    let path = dir.join("settings.json");
    assert!(
        !load_settings(&path).hide_peer_cursors,
        "missing file shows them"
    );
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, b"{}").unwrap();
    assert!(
        !load_settings(&path).hide_peer_cursors,
        "absent key shows them"
    );
    save_settings(
        &path,
        &Settings {
            hide_peer_cursors: true,
            ..Settings::default()
        },
    );
    assert!(load_settings(&path).hide_peer_cursors);
    let _ = std::fs::remove_dir_all(dir);
}

fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

impl DragRig {
    /// One frame in the order the app runs it: shortcuts, the board, then
    /// the chat box over it.
    fn room_frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(self.time),
            modifiers: self.modifiers,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        self.ctx.run(input, |ctx| {
            app.handle_board_shortcuts(ctx);
            egui::CentralPanel::default().show(ctx, |ui| app.render_board(ui));
            app.chat_overlay(ctx);
        })
    }

    /// Run one frame that delivers `events` to the shortcut handler.
    fn press_for_shortcuts(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run(input, |ctx| app.handle_board_shortcuts(ctx));
    }
}

#[test]
fn board_shortcuts_follow_the_key_map_and_a_rebound_key_replaces_the_old_one() {
    let mut rig = DragRig::new(3, 4);
    rig.press_for_shortcuts(vec![key_event(egui::Key::T, egui::Modifiers::NONE)]);
    assert_eq!(rig.app.view_command.take(), Some(ViewCommand::LoosePieces));

    let mut controls = rig.app.controls.clone();
    controls.set(Action::FocusTray, vec![Binding::plain(egui::Key::K)]);
    rig.app.apply_controls(controls);
    rig.press_for_shortcuts(vec![key_event(egui::Key::T, egui::Modifiers::NONE)]);
    assert_eq!(rig.app.view_command, None, "the old key is free now");
    rig.press_for_shortcuts(vec![key_event(egui::Key::K, egui::Modifiers::NONE)]);
    assert_eq!(rig.app.view_command.take(), Some(ViewCommand::LoosePieces));

    // The saved form is what reaches the settings file.
    assert_eq!(rig.app.settings.keybindings["focus_tray"], vec!["K"]);
    assert_eq!(rig.app.settings.keybindings.len(), 1);
}

#[test]
fn zoom_keys_use_the_configured_step_and_unbound_actions_do_nothing() {
    let mut rig = DragRig::new(3, 4);
    rig.app.settings.mouse.zoom_step = 1.5;
    rig.press_for_shortcuts(vec![key_event(egui::Key::Minus, egui::Modifiers::NONE)]);
    match rig.app.view_command.take() {
        Some(ViewCommand::ZoomBy(factor)) => assert!((factor - 1.0 / 1.5).abs() < 1e-6),
        other => panic!("expected a zoom, got {other:?}"),
    }
    // Tidy and shuffle ship unbound, so E and S must not rearrange anyone's tray.
    rig.press_for_shortcuts(vec![
        key_event(egui::Key::E, egui::Modifiers::NONE),
        key_event(egui::Key::S, egui::Modifiers::NONE),
    ]);
    assert_eq!(rig.outgoing_rx.try_recv().ok().map(|_| ()), None);
    // Once bound, the key sends the rearrange request.
    let mut controls = rig.app.controls.clone();
    controls.set(Action::TidyTray, vec![Binding::plain(egui::Key::E)]);
    rig.app.apply_controls(controls);
    rig.press_for_shortcuts(vec![key_event(egui::Key::E, egui::Modifiers::NONE)]);
    assert!(matches!(
        rig.outgoing_rx.try_recv(),
        Ok(ClientMessage::RearrangeTray {
            arrangement: TrayArrangement::EdgesFirst
        })
    ));
}

#[test]
fn rebinding_captures_the_next_key_and_refuses_one_that_is_taken() {
    let mut rig = DragRig::new(3, 4);
    rig.app.rebinding = Some(Action::FocusTray);
    let press = |rig: &mut DragRig, key| {
        rig.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(rig.time),
            events: vec![key_event(key, egui::Modifiers::NONE)],
            ..Default::default()
        };
        let app = &mut rig.app;
        let _ = rig.ctx.run(input, |ctx| app.controls_window(ctx));
    };
    // B already focuses the board: refused, and still waiting.
    press(&mut rig, egui::Key::B);
    assert_eq!(rig.app.rebinding, Some(Action::FocusTray));
    assert!(
        rig.app
            .controls_notice
            .as_deref()
            .unwrap()
            .contains("Focus the board")
    );
    assert!(rig.app.settings.keybindings.is_empty());
    // A free key is accepted.
    press(&mut rig, egui::Key::K);
    assert_eq!(rig.app.rebinding, None);
    assert!(rig.app.controls_notice.is_none());
    assert_eq!(
        rig.app.controls.bindings_for(Action::FocusTray),
        [Binding::plain(egui::Key::K)]
    );
    // Escape cancels without changing anything.
    rig.app.rebinding = Some(Action::FocusBoard);
    press(&mut rig, egui::Key::Escape);
    assert_eq!(rig.app.rebinding, None);
    assert_eq!(
        rig.app.controls.bindings_for(Action::FocusBoard),
        [Binding::plain(egui::Key::B)]
    );
}

#[test]
fn the_multi_select_modifier_is_configurable() {
    let mut rig = DragRig::new(3, 4);
    rig.app.settings.mouse.select_modifier = SelectModifier::Alt;
    let click = |rig: &mut DragRig, id: u32, modifiers: egui::Modifiers| {
        let piece = rig.piece(id);
        let pos = rig.to_screen(piece.x + rig.cw / 2.0, piece.y + rig.ch / 2.0);
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
        rig.modifiers = modifiers;
        rig.button(pos, true, modifiers);
        rig.button(pos, false, modifiers);
        rig.modifiers = egui::Modifiers::NONE;
    };
    click(&mut rig, 1, egui::Modifiers::NONE);
    click(&mut rig, 2, egui::Modifiers::COMMAND);
    assert_eq!(
        rig.app.selected_pieces,
        std::collections::HashSet::from([2]),
        "Ctrl no longer extends the selection"
    );
    click(&mut rig, 3, egui::Modifiers::ALT);
    assert_eq!(
        rig.app.selected_pieces,
        std::collections::HashSet::from([2, 3])
    );
}

#[test]
fn dragging_empty_space_can_be_switched_off() {
    let mut rig = DragRig::new(3, 4);
    rig.app.settings.mouse.drag_empty_pans = false;
    let before = rig.app.board_pan;
    let start = Pos2::new(60.0, 600.0);
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    for step in 1..=8 {
        rig.frame(vec![egui::Event::PointerMoved(
            start + Vec2::new(12.0 * step as f32, 0.0),
        )]);
    }
    rig.button(start + Vec2::new(96.0, 0.0), false, egui::Modifiers::NONE);
    assert_eq!(rig.app.board_pan, before, "the board must not move");

    rig.app.settings.mouse.drag_empty_pans = true;
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    for step in 1..=8 {
        rig.frame(vec![egui::Event::PointerMoved(
            start + Vec2::new(12.0 * step as f32, 0.0),
        )]);
    }
    rig.button(start + Vec2::new(96.0, 0.0), false, egui::Modifiers::NONE);
    assert_ne!(rig.app.board_pan, before, "the same drag pans when enabled");
}

#[test]
fn a_quick_flick_still_grabs_the_piece_under_the_press() {
    // One huge pointer jump per frame: the pointer has left the piece by
    // the time egui reports the drag.
    let mut rig = DragRig::new(3, 4);
    let piece = rig.piece(0);
    let start = rig.to_screen(piece.x + rig.cw / 2.0, piece.y + rig.ch / 2.0);
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    rig.frame(vec![egui::Event::PointerMoved(
        start + Vec2::new(-400.0, -200.0),
    )]);
    rig.relay();
    rig.button(
        start + Vec2::new(-400.0, -200.0),
        false,
        egui::Modifiers::NONE,
    );
    rig.relay();
    assert!(rig.messages >= 2, "flick should still move the piece");
    assert!(
        (rig.piece(0).x - piece.x).abs() > 10.0,
        "piece should have moved"
    );
}

#[test]
fn two_neighbours_dropped_together_join_into_one_group() {
    let mut rig = DragRig::new(3, 4);
    // Put piece 0 (row 0, col 0) at an open spot away from its target,
    // then bring its right-hand neighbour (piece 1) to the matching
    // offset: they should merge without being placed.
    let (cw, ch) = (rig.cw, rig.ch);
    rig.drag(0, 2.0 * cw + 20.0, 1.0 * ch, egui::Modifiers::NONE);
    let anchor = rig.piece(0);
    assert!(!anchor.placed);
    rig.drag(
        1,
        anchor.x + cw + 6.0,
        anchor.y - 5.0,
        egui::Modifiers::NONE,
    );
    assert_eq!(
        rig.piece(0).group_id,
        rig.piece(1).group_id,
        "neighbours should have joined"
    );
    assert!(!rig.piece(0).placed);
    assert!(
        (rig.piece(1).x - rig.piece(0).x - cw).abs() < 0.5,
        "snapped to mating offset"
    );
}

#[test]
fn a_dropped_piece_far_from_anything_stays_loose() {
    let mut rig = DragRig::new(3, 4);
    let (cw, ch) = (rig.cw, rig.ch);
    rig.drag(5, 3.0 * cw, 2.0 * ch, egui::Modifiers::NONE);
    let piece = rig.piece(5);
    assert!(!piece.placed);
    assert!(piece.held_by.is_none(), "lease released after the drop");
}

#[test]
fn ctrl_selected_pieces_move_together() {
    let mut rig = DragRig::new(3, 4);
    rig.click_select(2);
    rig.click_select(7);
    let (a, b) = (rig.piece(2), rig.piece(7));
    rig.drag(7, b.x - 60.0, b.y + 40.0, egui::Modifiers::COMMAND);
    let (a2, b2) = (rig.piece(2), rig.piece(7));
    let moved_a = (a2.x - a.x, a2.y - a.y);
    let moved_b = (b2.x - b.x, b2.y - b.y);
    assert!(
        (moved_a.0 - moved_b.0).abs() < 1.0 && (moved_a.1 - moved_b.1).abs() < 1.0,
        "both pieces move by the same delta: {moved_a:?} vs {moved_b:?}"
    );
    assert!(moved_b.0.abs() > 20.0, "selection actually moved");
}

#[test]
fn network_deliveries_wake_the_ui() {
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ctx = egui::Context::default();
    let counter = requests.clone();
    ctx.set_request_repaint_callback(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let (tx, rx) = mpsc::channel();
    let sender = WakingSender {
        tx,
        repaint: Some(ctx),
    };
    let before = requests.load(Ordering::SeqCst);
    sender.clone().send(7).unwrap();
    assert_eq!(rx.recv().unwrap(), 7);
    assert!(
        requests.load(Ordering::SeqCst) > before,
        "a delivery must request a repaint"
    );
}

#[test]
fn shaped_piece_triangulates_and_hit_testing_respects_its_tabs() {
    let snapshot = RoomSnapshot::create(
        "room".into(),
        "JOIN42".into(),
        "Puzzle".into(),
        "hash".into(),
        "png".into(),
        900,
        900,
        3,
        3,
        "host".into(),
    )
    .unwrap();
    let piece = snapshot.piece(4).unwrap();
    let outline = piece_outline(&snapshot, piece);
    let triangles = triangulate_polygon(&outline).expect("jigsaw outline should tessellate");
    assert_eq!(triangles.len(), outline.len() - 2);
    assert!(
        triangles
            .iter()
            .flatten()
            .all(|index| *index < outline.len())
    );

    for piece in &snapshot.pieces {
        let outline = piece_outline(&snapshot, piece);
        let triangles = triangulate_polygon(&outline)
            .unwrap_or_else(|| panic!("piece {} should tessellate", piece.id));
        assert_eq!(triangles.len(), outline.len() - 2);
    }

    assert!(point_in_polygon(Pos2::new(150.0, 150.0), &outline));
    assert!(!point_in_polygon(Pos2::new(-10.0, -10.0), &outline));
}

#[test]
fn dragging_empty_board_space_pans_without_moving_pieces() {
    let mut rig = DragRig::new(3, 4);
    let start = rig.to_screen(40.0, -80.0);
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    for step in 1..=5 {
        let pos = start + Vec2::new(12.0, 6.0) * step as f32;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
        rig.relay();
    }
    rig.button(start + Vec2::new(60.0, 30.0), false, egui::Modifiers::NONE);
    rig.relay();
    assert_eq!(rig.messages, 0, "no piece was leased or moved");
    assert!(rig.app.board_pan.x > 30.0 && rig.app.board_pan.y > 15.0);
    assert!(rig.app.pan_drag_last.is_none(), "the pan ends on release");
}

#[test]
fn held_and_selected_pieces_paint_above_loose_and_placed_ones() {
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "INVITE".into(),
        "Layers".into(),
        "a".repeat(64),
        "png".into(),
        400,
        300,
        3,
        4,
        "host".into(),
    )
    .unwrap();
    snapshot.piece_mut(7).unwrap().placed = true;
    snapshot.piece_mut(2).unwrap().held_by = Some("guest".into());
    let selected = std::collections::HashSet::from([4]);
    let order = paint_order(&snapshot, &selected);
    assert_eq!(order.len(), 12);
    assert_eq!(
        snapshot.pieces[order[0]].id, 7,
        "placed pieces lie at the back"
    );
    assert_eq!(snapshot.pieces[order[10]].id, 4, "then the selection");
    assert_eq!(snapshot.pieces[order[11]].id, 2, "a held piece is on top");
    let loose: Vec<u32> = order[1..10]
        .iter()
        .map(|&i| snapshot.pieces[i].id)
        .collect();
    assert!(loose.is_sorted(), "untouched pieces keep a stable order");
}

#[test]
fn piece_presets_follow_the_media_shape_and_respect_the_limit() {
    assert_eq!(grid_for_piece_count(12, 4.0 / 3.0), (3, 4));
    let (rows, columns) = grid_for_piece_count(48, 16.0 / 9.0);
    assert!(columns > rows, "wide media gets more columns");
    assert!((40..=56).contains(&(rows * columns)));
    let (rows, columns) = grid_for_piece_count(108, 9.0 / 16.0);
    assert!(rows > columns, "tall media gets more rows");
    for aspect in [0.05, 0.5, 1.0, 2.4, 40.0, f32::NAN] {
        for target in [0, 12, 192, 400, u16::MAX] {
            let (rows, columns) = grid_for_piece_count(target, aspect);
            assert!(rows >= 2 && columns >= 2);
            assert!(rows * columns <= MAX_PUZZLE_PIECES);
            assert!(rows <= max_grid_dimension_for_other(columns));
        }
    }
}

#[test]
fn elapsed_time_is_formatted_as_a_clock() {
    assert_eq!(format_elapsed(Duration::from_secs(0)), "00:00");
    assert_eq!(format_elapsed(Duration::from_secs(75)), "01:15");
    assert_eq!(format_elapsed(Duration::from_secs(3725)), "1:02:05");
}

#[test]
fn player_list_tracks_peers_and_skips_our_own_join_echo() {
    let (incoming_tx, incoming) = mpsc::channel();
    let (outgoing, _outgoing_rx) = tokio_mpsc::unbounded_channel();
    let directory = std::env::temp_dir().join(format!("pieceoff-peers-{}", std::process::id()));
    let mut app = PieceOffApp {
        player_name: "Me".into(),
        recent_rooms: Vec::new(),
        recent_rooms_path: directory.join("recent-rooms.json"),
        network: Some(Network { outgoing, incoming }),
        own_join_pending: true,
        ..PieceOffApp::default()
    };
    let snapshot = RoomSnapshot::create(
        "peer-room".into(),
        "PEERS1".into(),
        "Peers".into(),
        "a".repeat(64),
        "png".into(),
        32,
        24,
        2,
        2,
        "host-id".into(),
    )
    .unwrap();
    let joined = |name: &str| ServerMessage::PeerJoined {
        name: name.into(),
        spectator: false,
    };
    for message in [
        ServerMessage::Welcome {
            client_id: "guest-id".into(),
            name: "Me".into(),
            peers: vec!["Ana".into()],
            snapshot,
            media_upload_token: None,
            spectator: false,
            spectators: Vec::new(),
            server_now_ms: 0,
        },
        joined("Me"),
        joined("Bo"),
        joined("Me"),
        ServerMessage::PeerLeft {
            name: "Ana".into(),
            spectator: false,
        },
    ] {
        incoming_tx.send(message).unwrap();
    }
    app.process_network(&egui::Context::default());
    assert_eq!(app.peers, ["Bo", "Me"], "only the first echo is ours");
    app.leave_room();
    assert!(app.peers.is_empty());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn status_messages_are_logged_and_problems_are_counted_until_viewed() {
    let mut app = PieceOffApp::default();
    app.record_status(0.0);
    assert_eq!(app.messages.len(), 1, "the opening hint is logged once");
    app.record_status(1.0);
    assert_eq!(app.messages.len(), 1, "an unchanged status is not repeated");

    app.status = "Could not save puzzle package: disk full".into();
    app.record_status(2.0);
    app.status = "Lost joined the puzzle.".into();
    app.record_status(3.0);
    assert_eq!(app.messages.len(), 3);
    assert_eq!(app.unseen_problems, 1, "a name cannot flag a join notice");
    assert!(app.messages[1].is_problem && !app.messages[2].is_problem);

    app.messages_open = true;
    app.status = "Connection lost. Reconnecting to room ABC in 2s…".into();
    app.toast_dismissed = true;
    app.record_status(4.0);
    assert_eq!(
        app.unseen_problems, 1,
        "problems seen in the open list are not counted"
    );
    assert!(!app.toast_dismissed, "a new message shows a fresh toast");

    for index in 0..300 {
        app.status = format!("Message {index}");
        app.record_status(5.0 + f64::from(index));
    }
    assert_eq!(app.messages.len(), 200, "history is bounded");
    assert_eq!(app.messages.back().unwrap().text, "Message 299");
}

#[test]
fn failures_are_recognised_as_problems_and_routine_notices_are_not() {
    for problem in [
        "Could not open puzzle package: bad signature",
        "Connection to the room server closed.",
        "Room media dimensions exceed this client's limits.",
        "Another player is moving that piece.",
        "Could not open the audio output (no device). Video playback continues.",
    ] {
        assert!(message_is_problem(problem), "{problem}");
    }
    for notice in [
        "Room media is ready. Share invite code ABC123.",
        "Puzzle restarted. Good luck!",
        "Invite code copied.",
        "Error joined the puzzle.",
        "Closed left. Their pieces are free to move.",
    ] {
        assert!(!message_is_problem(notice), "{notice}");
    }
}

#[test]
fn dragging_one_joined_piece_moves_its_whole_group_locally() {
    let mut rig = DragRig::new(3, 4);
    let (cw, ch) = (rig.cw, rig.ch);
    // Join pieces 5 and 6 away from their targets.
    rig.drag(5, 2.0 * cw - 30.0, 3.4 * ch, egui::Modifiers::NONE);
    rig.drag(6, 3.0 * cw - 30.0, 3.4 * ch, egui::Modifiers::NONE);
    assert_eq!(rig.piece(5).group_id, rig.piece(6).group_id, "joined");
    let gap = rig.piece(6).x - rig.piece(5).x;

    // Mid-drag, before the server has answered, both already move together.
    let grabbed = rig.piece(6);
    let start = rig.to_screen(grabbed.x + cw / 2.0, grabbed.y + ch / 2.0);
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    for step in 1..=4 {
        let pos = start + Vec2::new(-15.0, 10.0) * step as f32;
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    let (left, right) = (rig.piece(5), rig.piece(6));
    assert!(
        right.x < grabbed.x - 1.0,
        "the grabbed piece followed the pointer"
    );
    assert!(
        (right.x - left.x - gap).abs() < 0.01,
        "its neighbour came along"
    );
    assert_eq!(left.y, right.y);
    rig.button(start + Vec2::new(-60.0, 40.0), false, egui::Modifiers::NONE);
    rig.relay();
}

#[test]
fn media_clock_follows_rate_and_measures_lead_around_the_loop() {
    let anchor = Instant::now();
    let clock = MediaClock::new(anchor, 9_000, 2.0, 1_000, 10_000);
    let at = |ms: u64| anchor + Duration::from_millis(ms);
    assert_eq!(clock.position_at(at(0)), 9_000.0);
    assert_eq!(clock.position_at(at(250)), 9_500.0);
    // 1 s at double speed runs 2 s of media: 1 s to the loop end, 1 s past its start.
    assert_eq!(clock.position_at(at(1_000)), 2_000.0);
    assert_eq!(clock.lead_ms(9_100.0, at(0)), 100.0);
    assert_eq!(clock.lead_ms(8_900.0, at(0)), -100.0);
    // Just after the wrap, media from just before it is slightly late, and
    // media just after the loop start is slightly early for a clock near the end.
    assert_eq!(clock.lead_ms(9_900.0, at(1_000)), -1_100.0);
    assert_eq!(clock.lead_ms(1_050.0, at(450)), 150.0);
    let unlooped = MediaClock::new(anchor, 500, 1.0, 0, 0);
    assert_eq!(unlooped.position_at(at(700)), 1_200.0);
    assert_eq!(unlooped.lead_ms(1_000.0, at(700)), -200.0);
}

#[test]
fn audio_is_corrected_only_when_it_leaves_the_sync_tolerance() {
    assert_eq!(sync_action(0.0), SyncAction::Feed);
    assert_eq!(sync_action(AUDIO_SYNC_TOLERANCE_MS), SyncAction::Feed);
    assert_eq!(sync_action(-AUDIO_SYNC_TOLERANCE_MS), SyncAction::Feed);
    assert_eq!(sync_action(90.0), SyncAction::Wait, "ahead: hold back");
    assert_eq!(sync_action(-250.0), SyncAction::Skip(250.0), "behind: skip");
    assert_eq!(sync_action(-9_000.0), SyncAction::Reseek);
    assert_eq!(sync_action(9_000.0), SyncAction::Reseek);
    // Right after a start the delay is known exactly, so line up precisely.
    assert_eq!(initial_alignment(-40.0), SyncAction::Skip(40.0));
    assert_eq!(initial_alignment(30.0), SyncAction::Wait);
    assert_eq!(initial_alignment(2.0), SyncAction::Feed);
}

#[test]
fn timeline_presses_grab_a_loop_handle_only_when_they_land_on_it() {
    let (start, end, head) = (100.0, 400.0, 250.0);
    assert!(timeline_drag_target(103.0, start, end, head) == TimelineDrag::LoopStart);
    assert!(timeline_drag_target(395.0, start, end, head) == TimelineDrag::LoopEnd);
    assert!(timeline_drag_target(200.0, start, end, head) == TimelineDrag::Playhead);
    // With the playhead parked on the loop start, the playhead wins.
    assert!(timeline_drag_target(101.0, start, end, 101.0) == TimelineDrag::Playhead);
}

#[test]
fn video_quality_levels_bound_the_decoded_frame() {
    assert_eq!(
        fit_video_dimensions(3840, 2160, VideoQuality::Low.limits().0),
        (853, 480)
    );
    assert_eq!(
        fit_video_dimensions(3840, 2160, VideoQuality::Medium.limits().0),
        (1280, 720)
    );
    assert_eq!(
        fit_video_dimensions(640, 360, VideoQuality::Ultra.limits().0),
        (640, 360)
    );
    assert_eq!(Settings::default().video_quality, VideoQuality::High);
    assert!(VideoQuality::Ultra.limits().1 > VideoQuality::Low.limits().1);
}

/// Each frame of the generated clip is a flat grey whose brightness encodes
/// its media time, so a decoded frame says exactly when in the clip it is.
#[test]
fn video_frames_follow_the_room_clock_through_startup_and_the_loop() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("Skipping FFmpeg sync test: ffmpeg is not installed.");
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("pieceoff-sync-{}-{nonce}.mp4", std::process::id()));
    let generated = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=gray:s=64x48:r=24:d=2,format=yuv420p,geq=lum=60+70*T:cb=128:cr=128")
        .args([
            "-c:v", "mpeg4", "-q:v", "2", "-g", "6", "-threads", "1", "-y",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg should generate the ramp clip");
    assert!(generated.success());

    let mut app = PieceOffApp {
        media_size: (64, 48),
        media_duration_ms: 2_000,
        ..PieceOffApp::default()
    };
    app.start_video_decoder(path.clone(), false, egui::Context::default());
    let frames = app.decoder_frames.as_ref().unwrap();
    let started = Instant::now();
    app.decoder_control
        .as_ref()
        .unwrap()
        .send(DecoderCommand {
            playing: true,
            position_ms: 0,
            rate: 1.0,
            loop_start_ms: 0,
            loop_end_ms: 2_000,
            resync: false,
            issued_at: Instant::now(),
        })
        .unwrap();
    let mut errors_ms = Vec::new();
    let mut wrapped = false;
    while started.elapsed() < Duration::from_millis(3_200) {
        let Ok(frame) = frames.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let pixel = (frame.rgba.len() / 2) & !3;
        let luma = f64::from(frame.rgba[pixel]) * 219.0 / 255.0 + 16.0;
        let media_ms = (luma - 60.0) / 70.0 * 1000.0;
        let clock_ms = elapsed_ms % 2_000.0;
        let mut error = media_ms - clock_ms;
        if error > 1_000.0 {
            error -= 2_000.0;
        } else if error < -1_000.0 {
            error += 2_000.0;
        }
        // Allow the first moments for the decoder to start and catch up.
        if elapsed_ms > 600.0 {
            errors_ms.push(error);
            wrapped |= elapsed_ms > 2_100.0;
        }
    }
    drop(app);
    remove_file_when_released(&path);
    assert!(
        wrapped && errors_ms.len() > 20,
        "frames kept arriving: {}",
        errors_ms.len()
    );
    let worst = errors_ms
        .iter()
        .fold(0.0_f64, |worst, error| worst.max(error.abs()));
    let mean = errors_ms.iter().sum::<f64>() / errors_ms.len() as f64;
    eprintln!(
        "video sync: mean {mean:+.0} ms, worst {worst:.0} ms over {} frames",
        errors_ms.len()
    );
    assert!(
        worst < 160.0 && mean.abs() < 80.0,
        "frames should track the clock: mean {mean:.0} ms, worst {worst:.0} ms, {errors_ms:.0?}"
    );
}

#[test]
fn drag_position_updates_are_rate_limited_but_the_release_is_exact() {
    let mut rig = DragRig::new(3, 4);
    let piece = rig.piece(0);
    let start = rig.to_screen(piece.x + rig.cw / 2.0, piece.y + rig.ch / 2.0);
    rig.frame(vec![egui::Event::PointerMoved(start)]);
    rig.button(start, true, egui::Modifiers::NONE);
    // 60 pointer moves, 5 ms apart: 0.3 s of dragging.
    for step in 1..=60 {
        rig.time -= 0.045;
        rig.frame(vec![egui::Event::PointerMoved(
            start + Vec2::new(2.0 * step as f32, 0.0),
        )]);
    }
    let end = start + Vec2::new(120.0, 0.0);
    rig.time -= 0.045;
    rig.button(end, false, egui::Modifiers::NONE);

    let mut moves = 0;
    let mut released = None;
    while let Ok(message) = rig.outgoing_rx.try_recv() {
        match message {
            ClientMessage::MoveSelection { .. } => moves += 1,
            ClientMessage::DragEndSelection { x, y, .. } => released = Some((x, y)),
            _ => {}
        }
    }
    assert!(
        (2..=12).contains(&moves),
        "{moves} move updates for 0.3 s of dragging"
    );
    let (x, y) = released.expect("the release is always sent");
    let expected = rig.app.snapshot.as_ref().unwrap().piece(0).unwrap();
    assert!(
        (x - expected.x).abs() < 0.01 && (y - expected.y).abs() < 0.01,
        "released at ({x}, {y}) but the piece is at ({}, {})",
        expected.x,
        expected.y
    );
}

fn polygon_area(points: &[[f32; 2]]) -> f32 {
    (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        .abs()
        / 2.0
}

fn sample_shape() -> (CachedPieceShape, f32, f32) {
    let snapshot = RoomSnapshot::create(
        "room".into(),
        "INVITE".into(),
        "LOD".into(),
        "a".repeat(64),
        "png".into(),
        800,
        600,
        3,
        4,
        "host".into(),
    )
    .unwrap();
    // An interior piece has a tab or blank on every side.
    let piece = snapshot.piece(5).unwrap();
    (
        CachedPieceShape::new(piece_outline(&snapshot, piece)),
        snapshot.cell_width(),
        snapshot.cell_height(),
    )
}

#[test]
fn smaller_pieces_are_drawn_from_thinner_outlines_that_keep_their_area_and_corners() {
    let (shape, cw, ch) = sample_shape();
    let full = shape.outline.len();
    let sizes = [4.0, 20.0, 60.0, 120.0, 240.0, 480.0, 2000.0];
    let counts: Vec<usize> = sizes
        .iter()
        .map(|perimeter| shape.detail(*perimeter).unwrap().outline.len())
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] <= pair[1]),
        "{counts:?}"
    );
    assert!(
        counts[0] <= full / 16,
        "specks use the coarsest outline: {counts:?}"
    );
    assert_eq!(*counts.last().unwrap(), full, "zoomed in uses every sample");

    let full_area = polygon_area(&shape.outline);
    for perimeter in sizes {
        let detail = shape.detail(perimeter).unwrap();
        // The triangulation fills the thinned outline exactly...
        let triangle_area: f32 = detail
            .triangles
            .iter()
            .map(|[a, b, c]| {
                polygon_area(&[detail.outline[*a], detail.outline[*b], detail.outline[*c]])
            })
            .sum();
        let outline_area = polygon_area(&detail.outline);
        assert!(
            (triangle_area - outline_area).abs() < outline_area * 0.001,
            "perimeter {perimeter}: triangles {triangle_area} vs outline {outline_area}"
        );
        // ...and thinning barely changes the piece's shape.
        assert!(
            (outline_area - full_area).abs() < full_area * 0.04,
            "perimeter {perimeter}: {outline_area} vs {full_area}"
        );
        // The four cell corners must survive, or neighbours would not meet.
        for corner in [[0.0, 0.0], [cw, 0.0], [cw, ch], [0.0, ch]] {
            assert!(
                detail
                    .outline
                    .iter()
                    .any(|p| (p[0] - corner[0]).abs() < 0.01 && (p[1] - corner[1]).abs() < 0.01),
                "perimeter {perimeter} lost corner {corner:?}"
            );
        }
    }
}

#[test]
fn a_400_piece_table_stays_within_a_vertex_budget() {
    // Full detail for every piece is about 770k vertices; thinned outlines
    // and unstroked specks must keep the whole table far below that.
    let mut rig = DragRig::new(20, 20);
    rig.app.view_command = Some(ViewCommand::Fit);
    rig.frame(Vec::new());
    let output = rig.frame(Vec::new());
    let vertices: usize = rig
        .ctx
        .tessellate(output.shapes, output.pixels_per_point)
        .iter()
        .map(|clipped| match &clipped.primitive {
            egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
            _ => 0,
        })
        .sum();
    assert!(
        vertices > 20_000,
        "the table should draw something: {vertices}"
    );
    assert!(vertices < 260_000, "too many vertices: {vertices}");
}

/// Frame-cost report for a 400-piece board. Run in release mode:
/// `cargo test --release -p pieceoff-app large_board_frame_cost -- --ignored --nocapture`.
#[test]
#[ignore = "timing report, not a pass/fail check"]
fn large_board_frame_cost() {
    let started = Instant::now();
    let mut rig = DragRig::new(20, 20);
    println!("rig + first frame: {:?}", started.elapsed());
    for (label, command) in [
        ("whole table", ViewCommand::Fit),
        ("board", ViewCommand::Board),
        ("tray", ViewCommand::LoosePieces),
    ] {
        rig.app.view_command = Some(command);
        rig.frame(Vec::new());
        let frames = 20;
        let mut build = Duration::ZERO;
        let mut tessellate = Duration::ZERO;
        let mut vertices = 0;
        for _ in 0..frames {
            let begun = Instant::now();
            let output = rig.frame(Vec::new());
            build += begun.elapsed();
            let begun = Instant::now();
            let meshes = rig.ctx.tessellate(output.shapes, output.pixels_per_point);
            tessellate += begun.elapsed();
            vertices = meshes
                .iter()
                .map(|clipped| match &clipped.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                    _ => 0,
                })
                .sum::<usize>();
        }
        println!(
            "{label:>11}: build {:>7.2?}/frame  tessellate {:>7.2?}/frame  {vertices} vertices",
            build / frames,
            tessellate / frames
        );
    }
}

#[test]
fn cover_uv_rect_crops_the_long_side_and_is_identity_for_equal_aspect() {
    use crate::board::cover_uv_rect;
    let same = cover_uv_rect(64.0, 48.0, 128.0, 96.0);
    assert!(same.min.x.abs() < 1e-6 && (same.max.x - 1.0).abs() < 1e-6);
    assert!(same.min.y.abs() < 1e-6 && (same.max.y - 1.0).abs() < 1e-6);
    // 16:9 media on a 4:3 board: left and right trimmed equally.
    let wide = cover_uv_rect(64.0, 48.0, 160.0, 90.0);
    assert!(wide.min.y.abs() < 1e-6 && (wide.max.y - 1.0).abs() < 1e-6);
    let width = wide.max.x - wide.min.x;
    assert!((width - (4.0 / 3.0) / (16.0 / 9.0)).abs() < 1e-5);
    assert!((wide.min.x + wide.max.x - 1.0).abs() < 1e-6);
    // Tall media: top and bottom trimmed.
    let tall = cover_uv_rect(64.0, 48.0, 90.0, 160.0);
    assert!(tall.min.x.abs() < 1e-6 && (tall.max.x - 1.0).abs() < 1e-6);
    assert!(tall.min.y > 0.0 && tall.max.y < 1.0);
    // Unknown media size must not produce NaN.
    let unknown = cover_uv_rect(64.0, 48.0, 0.0, 0.0);
    assert_eq!(unknown.min, egui::Pos2::ZERO);
    assert_eq!(unknown.max, egui::Pos2::new(1.0, 1.0));
}

#[test]
fn piece_mesh_samples_the_cover_fitted_uv_rect() {
    use crate::board::{cover_uv_rect, piece_mesh};
    let uv = cover_uv_rect(64.0, 48.0, 160.0, 90.0);
    let piece = pieceoff_core::Piece {
        id: 0,
        group_id: 0,
        row: 0,
        column: 0,
        x: 0.0,
        y: 0.0,
        placed: false,
        held_by: None,
    };
    let mesh = piece_mesh(
        egui::TextureId::default(),
        &piece,
        &[],
        &[[0.0, 0.0], [32.0, 24.0]],
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 48.0)),
        1.0,
        32.0,
        24.0,
        64.0,
        48.0,
        uv,
    );
    assert!((mesh.vertices[0].uv.x - uv.min.x).abs() < 1e-6);
    assert!((mesh.vertices[0].uv.y - uv.min.y).abs() < 1e-6);
    let middle = uv.min.x + 0.5 * (uv.max.x - uv.min.x);
    assert!((mesh.vertices[1].uv.x - middle).abs() < 1e-6);
}

#[test]
fn switching_items_loads_the_cached_file_and_keeps_the_room_title() {
    let directory = std::env::temp_dir().join(format!("pieceoff-switch-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let write = |name: &str, width: u32, height: u32, shade: u8| {
        let path = directory.join(name);
        image::RgbaImage::from_pixel(width, height, image::Rgba([shade, 0, 0, 255]))
            .save(&path)
            .unwrap();
        path
    };
    let first = write("first.png", 16, 12, 10);
    let second = write("second.png", 24, 12, 200);
    let first_meta = inspect_media(&first).unwrap();
    let second_meta = inspect_media(&second).unwrap();

    let mut app = PieceOffApp::default();
    let ctx = egui::Context::default();
    app.load_media(first.clone(), &ctx).unwrap();
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "SWAP01".into(),
        "Room title".into(),
        first_meta.sha256.clone(),
        "png".into(),
        16,
        12,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    snapshot.queue.push(pieceoff_core::MediaItem {
        sha256: second_meta.sha256.clone(),
        extension: "png".into(),
        title: "second".into(),
        width: 24,
        height: 12,
        duration_ms: 0,
        available: true,
        position_ms: 0,
        loop_start_ms: 0,
        loop_end_ms: 0,
    });
    snapshot.current = 1;
    snapshot.sync_current_mirror();
    app.snapshot = Some(snapshot);

    // Not cached yet: stays on the old media and says it is waiting.
    app.switch_to_current_item(&ctx);
    assert_eq!(app.media_hash, first_meta.sha256);
    assert!(!app.media_ready);
    assert!(app.status.contains("download"));

    app.media_files
        .lock()
        .unwrap()
        .insert(second_meta.sha256.clone(), second.clone());
    app.switch_to_current_item(&ctx);
    assert_eq!(app.media_hash, second_meta.sha256);
    assert_eq!(app.media_path.as_ref(), Some(&second));
    assert_eq!(app.media_size, (24, 12));
    assert_eq!(app.title, "Room title");
    assert!(app.media_ready);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn package_carries_every_queued_item_and_single_item_packages_still_read() {
    let directory = std::env::temp_dir().join(format!(
        "pieceoff-queue-package-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let write = |name: &str, width: u32, shade: u8| {
        let path = directory.join(name);
        image::RgbaImage::from_pixel(width, 12, image::Rgba([shade, 1, 2, 255]))
            .save(&path)
            .unwrap();
        path
    };
    let first = write("a.png", 16, 10);
    let second = write("b.png", 24, 200);
    let first_hash = sha256_file(&first).unwrap();
    let second_hash = sha256_file(&second).unwrap();
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "PKGQ01".into(),
        "Queue".into(),
        first_hash.clone(),
        "png".into(),
        16,
        12,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    snapshot.queue.push(pieceoff_core::MediaItem {
        sha256: second_hash.clone(),
        extension: "png".into(),
        title: "b".into(),
        width: 24,
        height: 12,
        duration_ms: 0,
        available: true,
        position_ms: 0,
        loop_start_ms: 0,
        loop_end_ms: 0,
    });
    let package = directory.join("queue.pieceoff");
    write_puzzle_package_with_extras(
        &package,
        &first,
        &snapshot,
        &[(second_hash.clone(), second.clone())],
    )
    .unwrap();
    let (restored, media, extras) = read_puzzle_package_with_extras(&package).unwrap();
    assert_eq!(restored.queue.len(), 2);
    assert_eq!(sha256_file(&media).unwrap(), first_hash);
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].0, second_hash);
    assert_eq!(sha256_file(&extras[0].1).unwrap(), second_hash);
    assert!(restored.queue.iter().all(|item| !item.available));

    // A mislabeled extra is refused at write time.
    let wrong = write_puzzle_package_with_extras(
        &directory.join("wrong.pieceoff"),
        &first,
        &snapshot,
        &[(first_hash.clone(), second.clone())],
    );
    assert!(wrong.is_err());

    // Single-item packages keep the original layout and read with no extras.
    let single_snapshot = RoomSnapshot::create(
        "room".into(),
        "PKGQ02".into(),
        "One".into(),
        first_hash.clone(),
        "png".into(),
        16,
        12,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    let single = directory.join("single.pieceoff");
    write_puzzle_package(&single, &first, &single_snapshot).unwrap();
    let (_, _, none) = read_puzzle_package_with_extras(&single).unwrap();
    assert!(none.is_empty());
    assert!(read_puzzle_package(&single).is_ok());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn item_end_prefers_loop_out_then_duration() {
    use crate::playback::item_end_ms;
    let mut state = pieceoff_core::PlaybackState::default();
    assert_eq!(item_end_ms(&state, 9_000), 9_000);
    state.loop_end_ms = 4_000;
    assert_eq!(item_end_ms(&state, 9_000), 4_000);
}

#[test]
fn unwrapped_position_does_not_apply_the_loop() {
    use crate::playback::unwrapped_position_at;
    let state = pieceoff_core::PlaybackState {
        playing: true,
        position_ms: 3_000,
        rate: 1.0,
        loop_start_ms: 1_000,
        loop_end_ms: 4_000,
        changed_at_ms: 100,
    };
    assert_eq!(unwrapped_position_at(&state, 2_100), 5_000);
    // The wrapped estimate differs, which is why the hold check needs this one.
    assert_ne!(crate::playback::estimated_position_at(&state, 2_100), 5_000);
}

#[test]
fn holding_at_item_end_pauses_the_decoder_once_when_not_repeating_one() {
    let (control_tx, control_rx) = mpsc::channel();
    let playing = Arc::new(AtomicBool::new(true));
    let mut app = PieceOffApp {
        decoder_control: Some(control_tx),
        decoder_playing: Some(playing.clone()),
        media_duration_ms: 1_000,
        ..PieceOffApp::default()
    };
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "HOLD01".into(),
        "Hold".into(),
        "a".repeat(64),
        "mp4".into(),
        32,
        24,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    snapshot.repeat = pieceoff_core::RepeatMode::All;
    snapshot.playback.playing = true;
    snapshot.playback.position_ms = 0;
    // Started long enough ago that the 1s item has finished.
    snapshot.playback.changed_at_ms = 1;
    app.snapshot = Some(snapshot);

    app.hold_at_item_end();
    let command = control_rx.try_recv().expect("hold sends a pause command");
    assert!(!command.playing);
    assert_eq!(command.position_ms, 1_000);
    assert!(!playing.load(Ordering::Relaxed));
    // Only once per hold.
    app.hold_at_item_end();
    assert!(control_rx.try_recv().is_err());

    // Repeat One never holds.
    app.holding_at_end = false;
    app.snapshot.as_mut().unwrap().repeat = pieceoff_core::RepeatMode::One;
    app.hold_at_item_end();
    assert!(control_rx.try_recv().is_err());
}

fn queue_ui_app() -> (
    PieceOffApp,
    tokio_mpsc::UnboundedReceiver<ClientMessage>,
    RoomSnapshot,
) {
    let (_incoming_tx, incoming) = mpsc::channel();
    let (outgoing, outgoing_rx) = tokio_mpsc::unbounded_channel();
    let app = PieceOffApp {
        client_id: Some("host-id".into()),
        network: Some(Network { outgoing, incoming }),
        ..PieceOffApp::default()
    };
    let snapshot = RoomSnapshot::create(
        "room".into(),
        "QUEUE1".into(),
        "Queue".into(),
        "a".repeat(64),
        "png".into(),
        16,
        12,
        2,
        2,
        "host-id".into(),
    )
    .unwrap();
    (app, outgoing_rx, snapshot)
}

#[test]
fn queue_add_from_file_probes_registers_and_sends_queue_add() {
    let directory = std::env::temp_dir().join(format!("pieceoff-qadd-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("extra.png");
    image::RgbaImage::from_pixel(20, 10, image::Rgba([9, 9, 9, 255]))
        .save(&path)
        .unwrap();
    let hash = sha256_file(&path).unwrap();
    let (mut app, mut outgoing_rx, snapshot) = queue_ui_app();
    app.snapshot = Some(snapshot);

    app.send_queue_add_from_file(path.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.media_files.lock().unwrap().get(&hash).is_none() && Instant::now() < deadline {
        app.poll_queue_probe();
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        app.media_files.lock().unwrap().get(&hash),
        Some(&path),
        "the file is remembered so it can be uploaded"
    );
    match outgoing_rx.try_recv().expect("QueueAdd was sent") {
        ClientMessage::QueueAdd {
            sha256,
            extension,
            title,
            width,
            height,
            ..
        } => {
            assert_eq!(sha256, hash);
            assert_eq!(extension, "png");
            assert_eq!(title, "extra");
            assert_eq!((width, height), (20, 10));
        }
        other => panic!("unexpected message {other:?}"),
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn queue_controls_are_for_the_unlocked_host_only() {
    let (mut app, _rx, snapshot) = queue_ui_app();
    assert!(!app.queue_controls_enabled(), "no room yet");
    app.snapshot = Some(snapshot);
    assert!(app.queue_controls_enabled());
    app.snapshot.as_mut().unwrap().video_lock = true;
    assert!(!app.queue_controls_enabled(), "locked");
    app.snapshot.as_mut().unwrap().video_lock = false;
    app.client_id = Some("someone-else".into());
    assert!(!app.queue_controls_enabled(), "guest");
}

#[test]
fn queue_shortcuts_step_the_room_and_cycle_repeat() {
    let (mut app, mut outgoing_rx, mut snapshot) = queue_ui_app();
    snapshot.queue.push(pieceoff_core::MediaItem {
        sha256: "b".repeat(64),
        extension: "png".into(),
        title: "b".into(),
        width: 16,
        height: 12,
        duration_ms: 0,
        available: true,
        position_ms: 0,
        loop_start_ms: 0,
        loop_end_ms: 0,
    });
    app.snapshot = Some(snapshot);
    app.run_queue_action(crate::controls::Action::NextVideo);
    assert!(matches!(
        outgoing_rx.try_recv().unwrap(),
        ClientMessage::NextItem
    ));
    app.run_queue_action(crate::controls::Action::PreviousVideo);
    assert!(matches!(
        outgoing_rx.try_recv().unwrap(),
        ClientMessage::PrevItem
    ));
    // Repeat defaults to One, so cycling goes One → All.
    app.run_queue_action(crate::controls::Action::CycleRepeat);
    assert!(matches!(
        outgoing_rx.try_recv().unwrap(),
        ClientMessage::SetRepeat {
            mode: pieceoff_core::RepeatMode::All
        }
    ));
    // Locked rooms send nothing.
    app.snapshot.as_mut().unwrap().video_lock = true;
    app.run_queue_action(crate::controls::Action::NextVideo);
    assert!(outgoing_rx.try_recv().is_err());
}

#[test]
fn uploads_only_use_files_known_by_their_hash() {
    use crate::net::{pending_uploads, token_upload};
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "UPLD01".into(),
        "Upload".into(),
        "a".repeat(64),
        "png".into(),
        16,
        12,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    snapshot.queue.push(pieceoff_core::MediaItem {
        sha256: "b".repeat(64),
        extension: "png".into(),
        title: "b".into(),
        width: 16,
        height: 12,
        duration_ms: 0,
        available: false,
        position_ms: 0,
        loop_start_ms: 0,
        loop_end_ms: 0,
    });
    let mut files = HashMap::new();
    files.insert("a".repeat(64), PathBuf::from("/media/a.png"));

    // A new host that never had "b" must not upload some other file under
    // b's hash: it would be rejected only after streaming the whole thing.
    let pending = pending_uploads(&snapshot, &files);
    assert_eq!(
        pending,
        vec![("a".repeat(64), PathBuf::from("/media/a.png"))]
    );
    assert_eq!(token_upload(Some(&"b".repeat(64)), None, &files), None);
    assert_eq!(
        token_upload(Some(&"a".repeat(64)), None, &files),
        Some(("a".repeat(64), PathBuf::from("/media/a.png")))
    );
    // Tokens from older servers carry no hash: they mean the room's media.
    assert_eq!(
        token_upload(None, Some(&"a".repeat(64)), &files),
        Some(("a".repeat(64), PathBuf::from("/media/a.png")))
    );
    assert_eq!(token_upload(None, None, &files), None);
}

#[test]
fn queue_add_probe_runs_off_the_ui_thread_and_then_sends_queue_add() {
    let directory = std::env::temp_dir().join(format!("pieceoff-qprobe-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("later.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
        .save(&path)
        .unwrap();
    let (mut app, mut outgoing_rx, snapshot) = queue_ui_app();
    app.snapshot = Some(snapshot);
    app.send_queue_add_from_file(path.clone());
    // Nothing is sent from the calling (UI) thread.
    assert!(outgoing_rx.try_recv().is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sent = None;
    while Instant::now() < deadline {
        app.poll_queue_probe();
        if let Ok(message) = outgoing_rx.try_recv() {
            sent = Some(message);
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(sent, Some(ClientMessage::QueueAdd { .. })));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn switching_items_does_not_rehash_the_cached_file() {
    // A cached file is trusted by its recorded hash: the swap path must not
    // read and hash a multi-GB file on the UI thread again.
    let directory = std::env::temp_dir().join(format!("pieceoff-norehash-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("cached.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([5, 5, 5, 255]))
        .save(&path)
        .unwrap();
    let recorded = "c".repeat(64); // deliberately not the file's real hash
    let mut app = PieceOffApp::default();
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "NOHASH".into(),
        "Room".into(),
        recorded.clone(),
        "png".into(),
        8,
        8,
        2,
        2,
        "host".into(),
    )
    .unwrap();
    snapshot.queue[0].available = true;
    app.snapshot = Some(snapshot);
    app.media_files
        .lock()
        .unwrap()
        .insert(recorded.clone(), path);
    app.switch_to_current_item(&egui::Context::default());
    assert_eq!(app.media_hash, recorded, "no re-hash happened");
    std::fs::remove_dir_all(directory).unwrap();
}

impl DragRig {
    /// Press at `from`, drag to `to` (screen coordinates), release.
    fn marquee(&mut self, from: Pos2, to: Pos2, modifiers: egui::Modifiers) {
        self.modifiers = modifiers;
        self.frame(vec![egui::Event::PointerMoved(from)]);
        self.button(from, true, modifiers);
        for step in 1..=8 {
            let pos = from + (to - from) * (step as f32 / 8.0);
            self.frame(vec![egui::Event::PointerMoved(pos)]);
        }
        self.button(to, false, modifiers);
        self.modifiers = egui::Modifiers::NONE;
        self.frame(Vec::new());
    }

    /// The tray is shuffled, so neighbours are found by position: the two
    /// loose pieces nearest the top-left corner of the tray, and the one
    /// furthest away.
    fn tray_neighbours(&self) -> (u32, u32, u32) {
        let mut loose: Vec<Piece> = self
            .app
            .snapshot
            .as_ref()
            .unwrap()
            .pieces
            .iter()
            .filter(|piece| !piece.placed)
            .cloned()
            .collect();
        loose.sort_by(|a, b| {
            (a.y, a.x)
                .partial_cmp(&(b.y, b.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        (loose[0].id, loose[1].id, loose[loose.len() - 1].id)
    }

    /// A screen rectangle that comfortably covers the cells of `ids`.
    fn around(&self, ids: &[u32]) -> (Pos2, Pos2) {
        let mut min = Pos2::new(f32::MAX, f32::MAX);
        let mut max = Pos2::new(f32::MIN, f32::MIN);
        for id in ids {
            let piece = self.piece(*id);
            let a = self.to_screen(piece.x, piece.y);
            let b = self.to_screen(piece.x + self.cw, piece.y + self.ch);
            min = min.min(a);
            max = max.max(b);
        }
        let pad = Vec2::splat(14.0);
        (min - pad, max + pad)
    }

    fn selected(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.app.selected_pieces.iter().copied().collect();
        ids.sort_unstable();
        ids
    }
}

#[test]
fn ctrl_drag_on_empty_space_selects_the_loose_pieces_it_covers() {
    let mut rig = DragRig::new(3, 4);
    let (a, b, far) = rig.tray_neighbours();
    let (from, to) = rig.around(&[a, b]);
    rig.marquee(from, to, egui::Modifiers::COMMAND);
    let selected = rig.selected();
    assert!(
        selected.contains(&a) && selected.contains(&b),
        "{selected:?}"
    );
    assert!(!selected.contains(&far), "far piece selected: {selected:?}");
    assert_eq!(rig.messages, 0, "selecting must not lease or move anything");
}

#[test]
fn marquee_skips_placed_pieces_and_adds_to_the_existing_selection() {
    let mut rig = DragRig::new(3, 4);
    let (a, b, far) = rig.tray_neighbours();
    // Place a piece that is not one of the box's targets, then box over its
    // home on the board together with the two tray neighbours.
    let placed = (0..12).find(|id| ![a, b, far].contains(id)).unwrap();
    rig.drag_to_target(placed);
    assert!(rig.piece(placed).placed);
    let (a, b, _) = rig.tray_neighbours();
    // Dragging selected the piece that is now placed; start clean.
    rig.app.selected_pieces.clear();
    rig.app.selected_pieces.insert(far);
    let (from, to) = rig.around(&[placed, a, b]);
    rig.marquee(from, to, egui::Modifiers::COMMAND);
    let selected = rig.selected();
    assert!(
        !selected.contains(&placed),
        "placed piece selected: {selected:?}"
    );
    assert!(
        selected.contains(&a) && selected.contains(&b),
        "{selected:?}"
    );
    assert!(
        selected.contains(&far),
        "existing selection kept: {selected:?}"
    );
}

#[test]
fn a_marquee_selection_drags_as_one_group() {
    let mut rig = DragRig::new(3, 4);
    let (a, b, _) = rig.tray_neighbours();
    let before: Vec<(f32, f32)> = [a, b]
        .iter()
        .map(|id| (rig.piece(*id).x, rig.piece(*id).y))
        .collect();
    let (from, to) = rig.around(&[a, b]);
    rig.marquee(from, to, egui::Modifiers::COMMAND);
    assert!(rig.selected().contains(&a) && rig.selected().contains(&b));
    rig.drag(
        a,
        before[0].0 + 40.0,
        before[0].1 + 40.0,
        egui::Modifiers::NONE,
    );
    let moved = |id: u32, origin: (f32, f32)| {
        let piece = rig.piece(id);
        (piece.x - origin.0, piece.y - origin.1)
    };
    let one = moved(a, before[0]);
    let two = moved(b, before[1]);
    assert!(one.0.abs() > 1.0, "the dragged piece moved: {one:?}");
    assert!(
        (one.0 - two.0).abs() < 0.01 && (one.1 - two.1).abs() < 0.01,
        "both moved by the same amount: {one:?} vs {two:?}"
    );
}

#[test]
fn drag_on_empty_space_without_the_modifier_still_pans() {
    let mut rig = DragRig::new(3, 4);
    let (a, b, _) = rig.tray_neighbours();
    let (from, to) = rig.around(&[a, b]);
    let pan_before = rig.app.board_pan;
    rig.marquee(from, to, egui::Modifiers::NONE);
    assert!(rig.selected().is_empty());
    assert_ne!(rig.app.board_pan, pan_before, "the board panned");
}

#[test]
fn byte_counts_read_naturally() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1_500), "1.5 KB");
    assert_eq!(format_bytes(3_400_000), "3.4 MB");
    assert_eq!(format_bytes(7_800_000_000), "7.8 GB");
}

/// Read the request line and headers; returns (request line, headers).
fn read_request_head(reader: &mut BufReader<TcpStream>) -> (String, Vec<String>) {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let request = line.trim().to_owned();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        headers.push(line.trim().to_ascii_lowercase());
    }
    (request, headers)
}

#[test]
fn an_interrupted_download_resumes_with_a_range_request() {
    let _serial = crate::package::transfer_test_lock();
    let content: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
    let hash = format!("{:x}", Sha256::digest(&content));
    let cached = std::env::temp_dir()
        .join("pieceoff-media-cache")
        .join(format!("{hash}.png"));
    let _ = std::fs::remove_file(&cached);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = content.clone();
    let server = thread::spawn(move || {
        // First attempt: promise everything, deliver 60 000 bytes, hang up.
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let (request, headers) = read_request_head(&mut reader);
        assert!(request.starts_with("GET /media/"));
        assert!(!headers.iter().any(|h| h.starts_with("range:")));
        write!(
            reader.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            served.len()
        )
        .unwrap();
        reader.get_mut().write_all(&served[..60_000]).unwrap();
        drop(reader);

        // Second attempt must ask for the rest only.
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let (_, headers) = read_request_head(&mut reader);
        assert!(
            headers.iter().any(|h| h == "range: bytes=60000-"),
            "resume asked for {headers:?}"
        );
        write!(
            reader.get_mut(),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 60000-{}/{}\r\nConnection: close\r\n\r\n",
            served.len() - 60_000,
            served.len() - 1,
            served.len()
        )
        .unwrap();
        reader.get_mut().write_all(&served[60_000..]).unwrap();
    });
    let url = format!("ws://{address}/ws");
    let first = download_room_media(&url, "RESUME1", &hash, "png");
    assert!(first.is_err(), "a cut-off download is an error");
    let second = download_room_media(&url, "RESUME1", &hash, "png").unwrap();
    assert_eq!(std::fs::read(&second).unwrap(), content);
    server.join().unwrap();
    let _ = std::fs::remove_file(cached);
}

#[test]
fn a_slow_but_steady_transfer_is_not_cut_off_by_a_total_timeout() {
    let _serial = crate::package::transfer_test_lock();
    // 1.2 s of trickling bytes with a 0.4 s idle limit: only a limit on the
    // whole request would end this.
    let content = vec![9u8; 8];
    let hash = format!("{:x}", Sha256::digest(&content));
    let cached = std::env::temp_dir()
        .join("pieceoff-media-cache")
        .join(format!("{hash}.png"));
    let _ = std::fs::remove_file(&cached);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = content.clone();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        read_request_head(&mut reader);
        write!(
            reader.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            served.len()
        )
        .unwrap();
        for byte in &served {
            thread::sleep(Duration::from_millis(150));
            reader.get_mut().write_all(&[*byte]).unwrap();
            reader.get_mut().flush().unwrap();
        }
    });
    let url = format!("ws://{address}/ws");
    let timeouts = TransferTimeouts {
        connect: Duration::from_secs(2),
        idle: Duration::from_millis(400),
    };
    let path =
        download_room_media_with(&url, "SLOW001", &hash, "png", timeouts).expect("steady transfer");
    assert_eq!(std::fs::read(&path).unwrap(), content);
    server.join().unwrap();
    let _ = std::fs::remove_file(cached);
}

#[test]
fn a_transfer_that_goes_silent_gives_up_after_the_idle_limit() {
    let _serial = crate::package::transfer_test_lock();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let hash = "d".repeat(64);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        read_request_head(&mut reader);
        write!(
            reader.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        // Nothing more: the client must not wait forever.
        thread::sleep(Duration::from_secs(2));
    });
    let url = format!("ws://{address}/ws");
    let timeouts = TransferTimeouts {
        connect: Duration::from_secs(1),
        idle: Duration::from_millis(300),
    };
    let started = Instant::now();
    let result = download_room_media_with(&url, "IDLE001", &hash, "png", timeouts);
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_millis(1800));
    server.join().unwrap();
    let _ = std::fs::remove_file(
        std::env::temp_dir()
            .join("pieceoff-media-cache")
            .join(format!(".{hash}.part")),
    );
}

#[test]
fn committing_the_server_field_saves_a_normalised_default() {
    let mut app = PieceOffApp {
        server_url: "  192.168.1.5 ".into(),
        ..PieceOffApp::default()
    };
    app.commit_server_url();
    assert_eq!(app.server_url, "ws://192.168.1.5:28878/ws");
    assert_eq!(
        app.settings.server_url.as_deref(),
        Some("ws://192.168.1.5:28878/ws")
    );
    assert!(app.status.is_empty() || !app.status.contains("not a valid"));
}

#[test]
fn an_invalid_server_field_is_reported_and_not_saved() {
    let mut app = PieceOffApp {
        server_url: "ho$t".into(),
        ..PieceOffApp::default()
    };
    app.commit_server_url();
    assert_eq!(app.settings.server_url, None);
    assert!(app.status.contains("not a valid"), "{}", app.status);
    assert_eq!(app.server_url, "ho$t", "the player's text is left to fix");
}

#[test]
fn reset_returns_to_the_local_server_and_forgets_the_saved_one() {
    let mut app = PieceOffApp {
        server_url: "game.example".into(),
        ..PieceOffApp::default()
    };
    app.commit_server_url();
    app.reset_server_url();
    assert_eq!(app.server_url, "ws://127.0.0.1:28878/ws");
    assert_eq!(app.settings.server_url, None);
}

#[test]
fn rejoining_a_recent_room_does_not_replace_the_saved_default() {
    let mut app = PieceOffApp {
        server_url: "home.example".into(),
        recent_rooms: vec![RecentRoom {
            server_url: "ws://friend.example:9000/ws".into(),
            invite_code: "ABCD".into(),
            player_name: "P".into(),
            title: "Room".into(),
            last_joined_ms: 0,
        }],
        ..PieceOffApp::default()
    };
    app.commit_server_url();
    app.select_recent_room(0);
    assert_eq!(app.server_url, "ws://friend.example:9000/ws");
    app.commit_server_url_if_edited();
    assert_eq!(
        app.settings.server_url.as_deref(),
        Some("ws://home.example:28878/ws")
    );
}

#[test]
fn locked_view_ignores_pan_drags_and_view_commands() {
    let mut rig = DragRig::new(3, 4);
    rig.app.settings.lock_view = true;
    let (a, b, _) = rig.tray_neighbours();
    let (from, to) = rig.around(&[a, b]);
    let pan_before = rig.app.board_pan;
    let zoom_before = rig.app.board_zoom;
    rig.marquee(from, to, egui::Modifiers::NONE);
    rig.app.view_command = Some(ViewCommand::ZoomBy(2.0));
    rig.marquee(from, to, egui::Modifiers::NONE);
    assert_eq!(rig.app.board_pan, pan_before);
    assert_eq!(rig.app.board_zoom, zoom_before);
}

#[test]
fn grab_margin_picks_a_piece_just_outside_its_edge() {
    let outline = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    assert_eq!(distance_to_outline(Pos2::new(14.0, 5.0), &outline), 4.0);
    assert_eq!(distance_to_outline(Pos2::new(13.0, 14.0), &outline), 5.0);
}

/// Windows keeps a file locked briefly after the process that read it is
/// killed, so deleting a test's media right after dropping the app can fail.
fn remove_file_when_released(path: &Path) {
    for _ in 0..50 {
        match std::fs::remove_file(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    std::fs::remove_file(path).unwrap();
}

fn ffmpeg_has_encoder(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(name))
        .unwrap_or(false)
}

fn chat_line(id: u64, client_id: &str, name: &str, text: &str) -> pieceoff_core::ChatLine {
    pieceoff_core::ChatLine {
        id,
        client_id: client_id.into(),
        name: name.into(),
        text: text.into(),
        sent_at_ms: 1_000 + id,
    }
}

#[test]
fn chat_counts_unread_lines_from_others_only_while_collapsed() {
    let mut chat = RoomChat::default();
    assert!(
        !chat.open,
        "starts collapsed so it does not cover the board"
    );
    chat.receive(chat_line(1, "ada", "Ada", "hi"), Some("me"));
    chat.receive(chat_line(2, "me", "Me", "hello"), Some("me"));
    chat.note("Bob joined");
    assert_eq!(chat.unread, 1, "our own lines and notes are not news");
    chat.show();
    assert_eq!(chat.unread, 0);
    chat.receive(chat_line(3, "ada", "Ada", "look"), Some("me"));
    assert_eq!(chat.unread, 0, "nothing is unread while it is open");
    chat.hide();
    chat.receive(chat_line(4, "ada", "Ada", "here"), Some("me"));
    assert_eq!(chat.unread, 1);

    // A reconnect's history adds only what we missed, in time order, and
    // only that counts as unread.
    chat.history(
        vec![
            chat_line(1, "ada", "Ada", "hi"),
            chat_line(3, "ada", "Ada", "look"),
            chat_line(5, "bob", "Bob", "missed this"),
            // Ours from before a reconnect, under an older client ID.
            chat_line(6, "old-me", "Me", "back"),
        ],
        "Me",
    );
    assert_eq!(chat.unread, 2);
    let texts: Vec<_> = chat
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ChatEntry::Line(line) => Some(line.text.as_str()),
            ChatEntry::Note { .. } => None,
        })
        .collect();
    assert_eq!(
        texts,
        ["hi", "hello", "look", "here", "missed this", "back"]
    );

    // Leaving forgets the room's chat but keeps the panel where it was.
    chat.show();
    chat.reset();
    assert!(chat.entries.is_empty() && chat.open && chat.unread == 0);
}

#[test]
fn enter_opens_the_chat_types_and_sends_and_keys_typed_there_are_not_shortcuts() {
    let mut rig = DragRig::new(3, 4);
    rig.room_frame(Vec::new());
    assert!(!rig.app.chat.open);
    let none = egui::Modifiers::NONE;
    rig.room_frame(vec![key_event(egui::Key::Enter, none)]);
    assert!(rig.app.chat.open, "Enter opens the chat");
    rig.room_frame(Vec::new());
    assert!(rig.ctx.wants_keyboard_input(), "and puts the cursor in it");

    // While typing, P and G are letters, not the ping and ghost shortcuts.
    let ghost = rig.app.settings.show_ghost;
    rig.room_frame(vec![
        key_event(egui::Key::P, none),
        egui::Event::Text("p".into()),
        key_event(egui::Key::G, none),
        egui::Event::Text("g ok".into()),
    ]);
    assert_eq!(rig.app.chat.draft, "pg ok");
    assert!(!rig.app.chat.ping_requested);
    assert_eq!(rig.app.settings.show_ghost, ghost);

    rig.room_frame(vec![key_event(egui::Key::Enter, none)]);
    rig.relay();
    assert_eq!(rig.chats_sent, vec!["pg ok".to_owned()]);
    assert!(rig.app.chat.draft.is_empty());
    assert!(!rig.ctx.wants_keyboard_input(), "Enter hands the keys back");
    assert!(rig.app.chat.open, "the history stays up");

    // Escape leaves the box with the draft kept, and does not also clear
    // the selection.
    rig.room_frame(vec![key_event(egui::Key::Enter, none)]);
    rig.room_frame(vec![egui::Event::Text("half".into())]);
    rig.app.selected_pieces.insert(2);
    rig.room_frame(vec![key_event(egui::Key::Escape, none)]);
    rig.room_frame(Vec::new());
    assert!(!rig.ctx.wants_keyboard_input());
    assert_eq!(rig.app.chat.draft, "half");
    assert!(rig.app.selected_pieces.contains(&2));
    rig.relay();
    assert_eq!(rig.chats_sent.len(), 1, "nothing more was sent");
}

#[test]
fn chat_from_the_room_shows_with_names_and_an_unread_count_when_collapsed() {
    let mut rig = DragRig::new(3, 4);
    rig.incoming_tx
        .send(ServerMessage::Chat(chat_line(
            1,
            "ada",
            "Ada",
            "edge pieces?",
        )))
        .unwrap();
    rig.incoming_tx
        .send(ServerMessage::PeerJoined {
            name: "Bob".into(),
            spectator: false,
        })
        .unwrap();
    rig.relay();
    // A new egui area is measured, unseen, on its first frame.
    rig.room_frame(Vec::new());
    let texts = painted_texts(&rig.room_frame(Vec::new()));
    assert!(texts.iter().any(|text| text == "Chat · 1 new"), "{texts:?}");
    rig.app.chat.show();
    rig.room_frame(Vec::new());
    let texts = painted_texts(&rig.room_frame(Vec::new())).join("\n");
    assert!(texts.contains("Ada: edge pieces?"), "{texts}");
    assert!(texts.contains("Bob joined"), "{texts}");
}

#[test]
fn the_ping_key_marks_the_spot_under_the_pointer_and_pings_fade_after_three_seconds() {
    let mut rig = DragRig::new(3, 4);
    let none = egui::Modifiers::NONE;
    let spot = rig.to_screen(80.0, 50.0);
    rig.room_frame(vec![egui::Event::PointerMoved(spot)]);
    rig.room_frame(vec![key_event(egui::Key::P, none)]);
    rig.relay();
    let sent = rig.pings_sent.last().copied().expect("a ping");
    assert!(
        (sent.x - 80.0).abs() < 0.5 && (sent.y - 50.0).abs() < 0.5,
        "board coordinates: {sent:?}"
    );

    // Off the table there is nothing to point at.
    rig.room_frame(vec![egui::Event::PointerGone]);
    rig.room_frame(vec![key_event(egui::Key::P, none)]);
    rig.relay();
    assert_eq!(rig.pings_sent.len(), 1);
    assert!(
        rig.app.status.contains("Point at the table"),
        "{}",
        rig.app.status
    );
    assert!(!rig.app.chat.ping_requested, "the request does not linger");

    // Everyone's pings, ours included, come back from the server.
    rig.incoming_tx
        .send(ServerMessage::Ping {
            client_id: "ada".into(),
            name: "Ada".into(),
            x: 80.0,
            y: 50.0,
        })
        .unwrap();
    rig.relay();
    assert_eq!(rig.app.chat.pings[0].color, player_color("Ada"));
    assert!(painted_texts(&rig.room_frame(Vec::new())).contains(&"Ada".to_owned()));
    rig.time += PING_SECONDS;
    assert!(!painted_texts(&rig.room_frame(Vec::new())).contains(&"Ada".to_owned()));
    assert!(rig.app.chat.pings.is_empty());
}

#[test]
fn a_players_colour_follows_their_name_so_it_survives_a_reconnect() {
    // A reconnect gets a new client ID but keeps the name.
    let mut cursors = LiveCursors::default();
    cursors.update("first-connection".into(), "Ada".into(), 1.0, 1.0, 0.0);
    let before = cursors.remote["first-connection"].color;
    cursors.update("second-connection".into(), "Ada".into(), 1.0, 1.0, 1.0);
    assert_eq!(cursors.remote["second-connection"].color, before);
    let mut chat = RoomChat::default();
    chat.add_ping("Ada".into(), 0.0, 0.0, 0.0);
    assert_eq!(chat.pings[0].color, before);
    assert_ne!(player_color("Ada"), player_color("Bob"));
}

#[test]
fn room_events_play_snap_join_finish_chat_and_ping_effects_once() {
    let mut rig = DragRig::new(3, 4);
    let (cw, ch) = (rig.cw, rig.ch);
    // Moving a piece somewhere empty is silent.
    rig.drag(0, 2.0 * cw + 20.0, 1.0 * ch, egui::Modifiers::NONE);
    assert!(
        rig.app.effects.played.is_empty(),
        "{:?}",
        rig.app.effects.played
    );
    // Its neighbour joins it: one join, heard when the server confirms.
    let anchor = rig.piece(0);
    rig.drag(
        1,
        anchor.x + cw + 6.0,
        anchor.y - 5.0,
        egui::Modifiers::NONE,
    );
    assert_eq!(rig.app.effects.played, vec![Effect::Join]);
    rig.app.effects.played.clear();
    // Snapping into place clicks; the last piece chimes instead.
    rig.drag_to_target(2);
    assert_eq!(rig.app.effects.played, vec![Effect::Snap]);
    for id in [0, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
        rig.drag_to_target(id);
    }
    assert!(rig.app.snapshot.as_ref().unwrap().completed());
    assert_eq!(rig.app.effects.played.last(), Some(&Effect::Complete));
    assert_eq!(
        rig.app
            .effects
            .played
            .iter()
            .filter(|effect| **effect == Effect::Complete)
            .count(),
        1
    );
    rig.app.effects.played.clear();

    // A restart scatters everything without a sound.
    rig.app.restart_puzzle();
    rig.relay();
    assert!(!rig.app.snapshot.as_ref().unwrap().completed());
    // A whole-room snapshot (join or lag resync) is silent too, even when
    // it shows pieces placed that were loose here.
    let mut resync = rig.room.snapshot.clone().unwrap();
    resync.pieces[0].placed = true;
    resync.revision += 1;
    rig.incoming_tx
        .send(ServerMessage::Snapshot { snapshot: resync })
        .unwrap();
    rig.relay();
    assert!(
        rig.app.effects.played.is_empty(),
        "{:?}",
        rig.app.effects.played
    );

    // Chat from others blips; our own lines and the history do not.
    rig.frame(Vec::new());
    for packet in [
        ServerMessage::Chat(chat_line(1, "host", "Host", "mine")),
        ServerMessage::ChatHistory {
            lines: vec![chat_line(2, "ada", "Ada", "earlier")],
        },
        ServerMessage::Chat(chat_line(3, "ada", "Ada", "hi")),
        // A burst inside the rate limit plays once.
        ServerMessage::Chat(chat_line(4, "ada", "Ada", "hi again")),
        ServerMessage::Ping {
            client_id: "host".into(),
            name: "Host".into(),
            x: 1.0,
            y: 1.0,
        },
    ] {
        rig.incoming_tx.send(packet).unwrap();
    }
    rig.relay();
    assert_eq!(rig.app.effects.played, vec![Effect::Chat, Effect::Ping]);

    // Muted effects stay quiet.
    rig.app.effects.played.clear();
    rig.app.effects.set_level(50, true);
    rig.frame(Vec::new());
    rig.incoming_tx
        .send(ServerMessage::Chat(chat_line(5, "ada", "Ada", "anyone?")))
        .unwrap();
    rig.relay();
    assert!(rig.app.effects.played.is_empty());
}

#[test]
fn effects_settings_are_clamped_on_load_and_default_on() {
    let settings: Settings =
        serde_json::from_str(r#"{"effects_volume": 255, "effects_muted": false}"#).unwrap();
    let effects = SoundEffects::new(settings.effects_volume, settings.effects_muted);
    assert_eq!(effects.volume, 100);
    let defaults = SoundEffects::new(Settings::default().effects_volume, false);
    assert!(defaults.audible());
    assert_eq!(defaults.volume, DEFAULT_EFFECTS_VOLUME);
}

/// A 4×2-cell holder below the rig's board (cells are 100×100).
fn rig_holder(owner: &str) -> pieceoff_core::PieceHolder {
    pieceoff_core::PieceHolder {
        id: 7,
        owner: owner.into(),
        label: "Sky".into(),
        x: 0.0,
        y: 400.0,
        width: 400.0,
        height: 200.0,
        owner_away_since_ms: None,
    }
}

/// Put `holder` and piece 0 (centred inside it) in both the room and the app.
fn put_piece_in_holder(rig: &mut DragRig, holder: pieceoff_core::PieceHolder) {
    for snapshot in [
        rig.room.snapshot.as_mut().unwrap(),
        rig.app.snapshot.as_mut().unwrap(),
    ] {
        snapshot.holders = vec![holder.clone()];
        let piece = snapshot.piece_mut(0).unwrap();
        piece.x = 100.0;
        piece.y = 450.0;
    }
}

#[test]
fn a_piece_in_another_players_holder_shows_a_lock_and_stays_put() {
    let mut rig = DragRig::new(3, 4);
    rig.app.player_name = "Host".into();
    rig.room.names.insert("ada".into(), "Ada".into());
    put_piece_in_holder(&mut rig, rig_holder("Ada"));
    let before = rig.piece(0);
    let over = rig.to_screen(before.x + rig.cw / 2.0, before.y + rig.ch / 2.0);
    rig.frame(vec![egui::Event::PointerMoved(over)]);
    let output = rig.frame(vec![egui::Event::PointerMoved(over)]);
    let texts = painted_texts(&output);
    assert!(
        texts.iter().any(|text| text == "🔒 In Ada's holder"),
        "{texts:?}"
    );
    assert!(texts.iter().any(|text| text == "🔒 Sky · Ada"), "{texts:?}");
    assert_eq!(
        output.platform_output.cursor_icon,
        egui::CursorIcon::NotAllowed
    );
    // The drag never reaches the room (`relay` would panic on a refusal).
    rig.drag(0, -300.0, -300.0, egui::Modifiers::NONE);
    assert_eq!(rig.piece(0), before);
    assert_eq!(rig.messages, 0);
    assert!(
        rig.app.status.contains("Ada's holder"),
        "{}",
        rig.app.status
    );
}

#[test]
fn our_own_holder_is_made_moved_with_its_pieces_and_kept_by_the_server() {
    let mut rig = DragRig::new(3, 4);
    rig.app.player_name = "Host".into();
    // The New holder key, with the pointer below the board.
    let near = rig.to_screen(200.0, 500.0);
    rig.frame(vec![egui::Event::PointerMoved(near)]);
    rig.app.holders.create_requested = true;
    rig.frame(vec![egui::Event::PointerMoved(near)]);
    rig.relay();
    let holder = rig.app.snapshot.as_ref().unwrap().holders[0].clone();
    assert_eq!(holder.owner, "Host");
    assert_eq!(holder.label, "Holder 1");
    assert_eq!(
        rig.room.snapshot.as_ref().unwrap().holders,
        vec![holder.clone()]
    );
    // Our own piece in it is ours to pick up.
    for snapshot in [
        rig.room.snapshot.as_mut().unwrap(),
        rig.app.snapshot.as_mut().unwrap(),
    ] {
        let piece = snapshot.piece_mut(0).unwrap();
        piece.x = holder.x + 50.0;
        piece.y = holder.y + 50.0;
    }
    let piece_before = rig.piece(0);

    // Drag the tab 300 px left: the holder and the piece inside follow.
    let tab = rig.to_screen(holder.x + 30.0, holder.y) - Vec2::new(0.0, HOLDER_TAB_HEIGHT / 2.0);
    let end = tab - Vec2::new(300.0 * rig.scale, 0.0);
    rig.frame(vec![egui::Event::PointerMoved(tab)]);
    rig.button(tab, true, egui::Modifiers::NONE);
    for step in 1..=10 {
        let pos = tab + (end - tab) * (step as f32 / 10.0);
        rig.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    rig.button(end, false, egui::Modifiers::NONE);
    rig.relay();
    rig.frame(Vec::new());
    let moved = rig.app.snapshot.as_ref().unwrap().holders[0].clone();
    assert!((moved.x - (holder.x - 300.0)).abs() < 1.0, "{moved:?}");
    assert_eq!(moved.y, holder.y);
    assert_eq!(
        rig.room.snapshot.as_ref().unwrap().holders,
        vec![moved.clone()]
    );
    let piece = rig.piece(0);
    assert!((piece.x - (piece_before.x + moved.x - holder.x)).abs() < 0.01);
    assert_eq!(piece.y, piece_before.y);
}

#[test]
fn packages_leave_holders_behind() {
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "INVITE".into(),
        "Pack".into(),
        "a".repeat(64),
        "png".into(),
        400,
        300,
        3,
        4,
        "host".into(),
    )
    .unwrap();
    snapshot.holders = vec![rig_holder("Ada")];
    assert!(portable_snapshot(&snapshot).holders.is_empty());
}

#[test]
fn a_spectator_drags_the_view_not_the_pieces_and_sends_nothing() {
    let mut rig = DragRig::new(3, 4);
    rig.app.spectating = true;
    let before = rig.app.snapshot.clone().unwrap();
    let pan = rig.app.board_pan;
    let piece = rig.piece(0);
    rig.drag(0, piece.x + 200.0, piece.y + 50.0, egui::Modifiers::NONE);
    rig.relay();
    assert_eq!(rig.messages, 0, "no room change was sent");
    assert!(
        rig.cursors_sent.is_empty(),
        "a spectator's pointer stays private"
    );
    assert_eq!(rig.app.snapshot.as_ref().unwrap().pieces, before.pieces);
    assert_ne!(rig.app.board_pan, pan, "dragging pans the view instead");
    assert!(rig.app.status.contains("watching"), "{}", rig.app.status);
    // Shortcuts that would change the room do nothing either.
    rig.app.arrange_tray(TrayArrangement::Shuffle);
    rig.app.chat.ping_requested = true;
    rig.frame(Vec::new());
    rig.relay();
    assert_eq!(rig.messages, 0);
    assert!(rig.pings_sent.is_empty());
}

#[test]
fn welcome_and_peer_messages_track_spectators_apart_from_players() {
    let mut rig = DragRig::new(2, 2);
    let snapshot = rig.app.snapshot.clone().unwrap();
    for message in [
        ServerMessage::Welcome {
            client_id: "me".into(),
            name: "Me".into(),
            peers: vec!["Ana".into()],
            snapshot,
            media_upload_token: None,
            spectator: true,
            spectators: vec!["Bo".into()],
            server_now_ms: 0,
        },
        ServerMessage::PeerJoined {
            name: "Cy".into(),
            spectator: true,
        },
        ServerMessage::PeerLeft {
            name: "Bo".into(),
            spectator: true,
        },
    ] {
        rig.incoming_tx.send(message).unwrap();
    }
    rig.relay();
    assert!(rig.app.spectating);
    assert!(rig.app.watch_only, "a reconnect rejoins as a spectator");
    assert_eq!(rig.app.peers, ["Ana"]);
    assert_eq!(rig.app.spectators, ["Cy"]);
    assert_eq!(
        presence_label(rig.app.peers.len(), rig.app.spectators.len(), true),
        "1 player · 2 watching"
    );
    rig.app.leave_room();
    assert!(!rig.app.spectating && rig.app.spectators.is_empty());
}

#[test]
fn presence_counts_players_and_watchers() {
    assert_eq!(presence_label(0, 0, false), "Just you");
    assert_eq!(presence_label(2, 0, false), "3 players");
    assert_eq!(presence_label(1, 1, false), "2 players · 1 watching");
    assert_eq!(presence_label(0, 0, true), "0 players · 1 watching");
}

#[test]
fn the_timelapse_replays_the_solve_step_by_step_without_touching_the_room() {
    let mut rig = DragRig::new(3, 4);
    let start = rig.app.snapshot.clone().unwrap();
    rig.drag(0, 0.0, 0.0, egui::Modifiers::NONE);
    let (x, y) = (rig.piece(5).x - 60.0, rig.piece(5).y + 10.0);
    rig.drag(5, x, y, egui::Modifiers::NONE);
    let live = rig.app.snapshot.clone().unwrap();
    assert!(live.piece(0).unwrap().placed);

    rig.app.request_timelapse();
    rig.relay();
    let mut replay = rig.app.replay.take().expect("the replay opened");
    assert_eq!(replay.steps(), 2);
    assert!(replay.playing);

    // Step 0 is the scattered start.
    replay.seek(0.0);
    let view = replay.view_of(&live);
    for piece in &start.pieces {
        let shown = view.piece(piece.id).unwrap();
        assert!((shown.x - piece.x).abs() <= 0.5 && (shown.y - piece.y).abs() <= 0.5);
        assert!(!shown.placed);
    }
    // Half a step animates the first drop part-way.
    replay.seek(0.5);
    let view = replay.view_of(&live);
    let from = start.piece(0).unwrap();
    let halfway = view.piece(0).unwrap();
    assert!(
        (halfway.x - from.x.round() / 2.0).abs() <= 1.0,
        "{}",
        halfway.x
    );
    // One step: the first piece is home, and counts as placed.
    replay.seek(1.0);
    let view = replay.view_of(&live);
    assert!(view.piece(0).unwrap().placed);
    assert_eq!(view.piece(5).unwrap().x, start.piece(5).unwrap().x.round());
    // The end matches the room.
    replay.seek(2.0);
    let view = replay.view_of(&live);
    for piece in &live.pieces {
        let shown = view.piece(piece.id).unwrap();
        assert!((shown.x - piece.x).abs() <= 0.5 && (shown.y - piece.y).abs() <= 0.5);
    }
    // And back again: scrubbing backwards rebuilds from the start.
    replay.seek(0.0);
    assert!(!replay.view_of(&live).piece(0).unwrap().placed);

    // Playing covers the whole solve in the chosen time, then stops.
    replay.seconds = 20;
    replay.playing = true;
    replay.advance(100.0);
    replay.advance(110.0);
    assert!((replay.position - 1.0).abs() < 1e-9, "{}", replay.position);
    replay.advance(200.0);
    assert!(replay.finished() && !replay.playing);
    replay.toggle();
    assert!(
        replay.playing && replay.position == 0.0,
        "play again from the start"
    );

    // Drawing the replay changes nothing in the live room or on the wire.
    rig.app.replay = Some(replay);
    rig.frame(Vec::new());
    let piece = rig.piece(3);
    rig.drag(3, piece.x + 100.0, piece.y, egui::Modifiers::NONE);
    assert_eq!(rig.app.snapshot.as_ref().unwrap().pieces, live.pieces);
}

#[test]
fn an_empty_or_foreign_timelapse_does_not_open() {
    let mut rig = DragRig::new(2, 2);
    rig.app.request_timelapse();
    rig.relay();
    assert!(rig.app.replay.is_none());
    assert!(
        rig.app.status.contains("Nothing to replay"),
        "{}",
        rig.app.status
    );
    let pieces = rig.app.snapshot.as_ref().unwrap().pieces.clone();
    let mut foreign = pieceoff_core::Timelapse::default();
    foreign.ensure_started(&pieces[..3]);
    foreign.record(5, &pieces[..1]);
    assert!(Replay::new(foreign, &pieces).is_err());
    // An answer nobody asked for is ignored.
    let mut log = pieceoff_core::Timelapse::default();
    log.ensure_started(&pieces);
    log.record(5, &pieces[..1]);
    rig.incoming_tx
        .send(ServerMessage::Timelapse { timelapse: log })
        .unwrap();
    rig.relay();
    assert!(rig.app.replay.is_none());
}

#[test]
fn replay_views_work_out_placement_and_joins_from_positions() {
    let mut snapshot = RoomSnapshot::create(
        "room".into(),
        "CODE".into(),
        "Joins".into(),
        "a".repeat(64),
        "png".into(),
        300,
        200,
        2,
        3,
        "host".into(),
    )
    .unwrap();
    let (cw, ch) = (snapshot.cell_width(), snapshot.cell_height());
    // Pieces 0 and 1 meet edge to edge off the board (rounded to pixels);
    // piece 5 is home; the rest are scattered far apart.
    for piece in &mut snapshot.pieces {
        let (x, y) = match (piece.row, piece.column) {
            (0, 0) => (1000.0, 1000.0),
            (0, 1) => ((1000.0 + cw).round(), 1000.0),
            (1, 2) => (2.0 * cw, ch),
            (row, column) => (3000.0 + 500.0 * f32::from(column), 500.0 * f32::from(row)),
        };
        piece.x = x;
        piece.y = y;
        piece.placed = false;
        piece.group_id = piece.id;
    }
    timelapse::settle_view(&mut snapshot);
    let at = |row, column| {
        snapshot
            .pieces
            .iter()
            .find(|piece| piece.row == row && piece.column == column)
            .unwrap()
            .clone()
    };
    assert_eq!(at(0, 0).group_id, at(0, 1).group_id);
    assert_ne!(at(0, 0).group_id, at(1, 0).group_id);
    assert!(!at(0, 0).placed);
    assert!(at(1, 2).placed);
    assert_eq!(snapshot.settled_pieces(), 3);
}
