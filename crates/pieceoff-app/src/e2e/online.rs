//! The desktop app against the real rooms Worker (PieceOff Online), not a
//! stub: `scripts/online-e2e.sh` starts a local `wrangler dev` set up like
//! production (uploads off, Turnstile-gated device links with a loopback
//! stand-in for Turnstile) and runs this ignored test with its address in
//! `PIECEOFF_ONLINE_E2E_URL`.
//!
//! Two app instances run `PieceOffApp::tick` as the window does and use the
//! same entry points as the buttons (create, quick match, join, leave,
//! recent rooms, the listed toggle's message). The only thing done by hand
//! is what a browser does: approving the device link on the site.
//!
//! Each player reaches the Worker through its own cuttable TCP proxy, so a
//! dropped network can be made for one player at a time. Device keys over
//! plaintext are allowed for the host here only (`allow_plaintext_keys`).

use super::*;
use crate::online::{self, LobbyRoom};
use crate::online_ui::LinkStage;

const SLOW_ORBIT: &str = "Slow Orbit";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// The Worker's `host:port` from `ws://host:port/ws`.
fn upstream_address(url: &str) -> std::net::SocketAddr {
    url.trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .expect("PIECEOFF_ONLINE_E2E_URL is ws://<ip>:<port>/ws")
}

/// A TCP relay to the Worker that can drop every connection through it at
/// once (a lost network) and keeps accepting new ones (the network is back).
/// It also keeps what clients sent, to show what was and was not asked.
struct CutProxy {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    streams: Arc<std::sync::Mutex<Vec<std::net::TcpStream>>>,
    sent: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl CutProxy {
    fn start(upstream: std::net::SocketAddr) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let streams: Arc<std::sync::Mutex<Vec<std::net::TcpStream>>> = Arc::default();
        let sent: Arc<std::sync::Mutex<Vec<u8>>> = Arc::default();
        let (stop_flag, kept, log) = (stop.clone(), streams.clone(), sent.clone());
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
                let log = log.clone();
                thread::spawn(move || {
                    let mut buffer = [0u8; 16384];
                    while let Ok(count) = from_client.read(&mut buffer) {
                        if count == 0 || to_server.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                        log.lock().unwrap().extend_from_slice(&buffer[..count]);
                    }
                    let _ = to_server.shutdown(std::net::Shutdown::Both);
                });
                let (mut from_server, mut to_client) = (server, client);
                thread::spawn(move || {
                    let mut buffer = [0u8; 16384];
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
            sent,
        }
    }

    fn url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    /// Drop every open connection, like a lost network; new ones still work.
    fn cut(&self) {
        for stream in self.streams.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Clients sent something containing `needle` (HTTP request lines and
    /// headers; WebSocket frames from clients are masked, so never match).
    fn saw(&self, needle: &str) -> bool {
        let sent = self.sent.lock().unwrap();
        sent.windows(needle.len()).any(|w| w == needle.as_bytes())
    }
}

impl Drop for CutProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cut();
    }
}

/// What the site's /link page does once the player passed Turnstile.
fn approve_link(server_url: &str, code: &str, site_origin: &str, token: &str) {
    let base = online::http_base(server_url).unwrap();
    let response = reqwest::blocking::Client::new()
        .post(format!("{base}/api/links/{code}/approve"))
        .header("origin", site_origin)
        .header("x-turnstile-token", token)
        .send()
        .expect("the approval reaches the Worker");
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    assert_eq!(status, 200, "approval refused: {body}");
}

/// Ask for the lobby through `viewer`'s own lobby code (as the open list
/// does every few seconds) until `wanted` holds, running every player.
fn lobby_until(
    players: &mut [&mut Player],
    viewer: usize,
    timeout: Duration,
    what: &str,
    wanted: impl Fn(&[LobbyRoom]) -> bool,
) -> Vec<LobbyRoom> {
    let started = Instant::now();
    let mut asked: Option<Instant> = None;
    loop {
        let app = &mut players[viewer].app;
        if !app.online.lobby.loading {
            if asked.is_some() {
                if let Some(error) = &app.online.lobby.error {
                    eprintln!("lobby error (retrying): {error}");
                } else if wanted(&app.online.lobby.rooms) {
                    return app.online.lobby.rooms.clone();
                }
            }
            // Under the Worker's 60 lobby reads per minute per address.
            if asked.is_none_or(|at| at.elapsed() > Duration::from_millis(1500)) {
                app.request_lobby(false);
                asked = Some(Instant::now());
            }
        }
        assert!(
            started.elapsed() < timeout,
            "timed out after {timeout:?} waiting for {what}; lobby: {:?}",
            players[viewer].app.online.lobby.rooms
        );
        for player in players.iter_mut() {
            player.frame();
        }
        thread::sleep(STEP);
    }
}

fn entry<'a>(rooms: &'a [LobbyRoom], code: &str) -> Option<&'a LobbyRoom> {
    rooms.iter().find(|room| room.code == code)
}

fn placed(player: &Player) -> usize {
    player
        .snapshot()
        .pieces
        .iter()
        .filter(|piece| piece.placed)
        .count()
}

#[test]
#[ignore = "needs a local rooms Worker: run scripts/online-e2e.sh"]
fn two_desktop_apps_play_together_on_the_rooms_worker() {
    let _serial = serial();
    let server = std::env::var("PIECEOFF_ONLINE_E2E_URL")
        .expect("PIECEOFF_ONLINE_E2E_URL (run scripts/online-e2e.sh)");
    let site_origin = env_or("PIECEOFF_ONLINE_E2E_SITE_ORIGIN", "https://pieceoff.app");
    let turnstile = env_or("PIECEOFF_ONLINE_E2E_TURNSTILE_TOKEN", "good-link-device");
    let root = scratch("online");
    let upstream = upstream_address(&server);
    let host_net = CutProxy::start(upstream);
    let guest_net = CutProxy::start(upstream);
    let mut host = Player::new("Host", &host_net.url(), &root);
    host.app.online.allow_plaintext_keys = true;
    let mut guest = Player::new("Guest", &guest_net.url(), &root);

    // 1. /api/info tells the app it is PieceOff Online.
    host.app.ensure_service_check();
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the server check",
        |p| p[0].app.current_service().is_some(),
    );
    let info = host
        .app
        .current_hosted()
        .cloned()
        .unwrap_or_else(|| panic!("not detected as hosted: {}", host.app.status));
    assert_eq!(info.service, "pieceoff-rooms");
    assert_eq!(info.protocol, u32::from(pieceoff_core::PROTOCOL_VERSION));
    assert_eq!(info.protocol, 15);
    assert_eq!(info.version_problem(), None);
    assert!(!info.uploads, "uploads are off at launch");
    assert!(info.has_feature("lobby") && info.has_feature("device_keys"));
    assert_eq!(info.share_url(), "https://pieceoff.app/join");
    assert!(!host.app.current_device_linked());

    // 3 (start). The host picks a built-in loop and a public room.
    let (title, _, filename, bytes) = crate::library_ui::BUILT_INS
        .into_iter()
        .find(|built_in| built_in.0 == SLOW_ORBIT)
        .unwrap();
    let ctx = host.ctx.clone();
    host.app.open_builtin(title, filename, bytes, &ctx);
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the built-in loop to load",
        |p| p[0].app.media_path.is_some() && p[0].app.media_size.0 > 0,
    );
    assert_eq!(
        pieceoff_core::builtin_media_title(&host.app.media_hash),
        Some(SLOW_ORBIT)
    );
    (host.app.rows, host.app.columns) = (3, 4);
    host.app.online.list_publicly = true;
    assert_eq!(host.app.listing_block_reason(), None);
    assert!(host.app.wants_listing());
    host.app.create_room();

    // 2. No key yet: the app opens a device link; the "browser" approves it.
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "a link code",
        |p| {
            p[0].app
                .online
                .link
                .as_ref()
                .is_some_and(|link| matches!(link.stage, LinkStage::Waiting { .. }))
        },
    );
    let (link_code, link_page) = match &host.app.online.link.as_ref().unwrap().stage {
        LinkStage::Waiting { code, url, .. } => (code.clone(), url.clone()),
        _ => unreachable!(),
    };
    assert!(online::valid_room_code(&link_code), "{link_code}");
    assert_eq!(
        link_page,
        format!("https://pieceoff.app/link?c={link_code}")
    );
    approve_link(&server, &link_code, &site_origin, &turnstile);

    // 3. The key is stored under this origin, the room is created over HTTP
    // and claimed on the socket with the creator's resume token.
    pump(
        &mut [&mut host],
        Duration::from_secs(40),
        "the hosted room",
        |p| p[0].in_room() && p[0].app.media_ready,
    );
    assert!(host.app.online.link.is_none());
    let origin = host.app.device_key_origin(&host_net.url()).unwrap();
    assert_eq!(origin, format!("http://{}", host_net.address));
    assert!(host.app.settings.device_keys.contains_key(&origin));
    assert_eq!(host.app.settings.device_keys.len(), 1);
    assert!(host.app.current_device_linked());
    assert!(!host.app.status.contains("pok_"), "{}", host.app.status);
    assert!(host.app.is_host(), "the creator's token claims the host");
    let host_id = host.app.client_id.clone().unwrap();
    let code = host.snapshot().invite_code.clone();
    assert!(online::valid_room_code(&code), "{code}");
    let session = host.app.online.session.clone().expect("a hosted session");
    assert_eq!(session.code, code);
    assert!(session.token().is_some());
    {
        let snapshot = host.snapshot();
        assert!(snapshot.listed, "created listed");
        assert!(snapshot.only_builtin_media());
        assert!(snapshot.media_available);
        assert!(snapshot.queue.iter().all(|item| item.available));
        assert_eq!(snapshot.pieces.len(), 12);
    }
    assert!(!host.app.media_uploading);
    run_for(&mut [&mut host], Duration::from_millis(500));
    assert!(
        !host_net.saw("/media"),
        "the host must not upload a built-in loop"
    );
    assert!(host_net.saw("POST /api/rooms "), "created over HTTP");
    // The invite link the host shares names this room.
    let invite = online::invite_link(info.share_url(), &code);
    assert_eq!(invite, format!("https://pieceoff.app/join?c={code}"));
    assert_eq!(
        online::parse_invite(&invite),
        Some(online::Invite {
            code: code.clone(),
            online: true
        })
    );

    // 4. The lobby, read through the guest's lobby code, lists it under
    // the bundled title; quick match picks it.
    let rooms = lobby_until(
        &mut [&mut host, &mut guest],
        1,
        Duration::from_secs(20),
        "the room in the lobby",
        |rooms| entry(rooms, &code).is_some(),
    );
    let listed = entry(&rooms, &code).unwrap();
    assert_eq!(listed.title, SLOW_ORBIT);
    assert_eq!(listed.host_name, "Host");
    assert_eq!((listed.players, listed.spectators), (1, 0));
    assert!(listed.max_players > 1 && listed.has_free_slot());
    assert_eq!((listed.pieces, listed.placed), (12, 0));
    assert_eq!(listed.cut_style, "jigsaw");
    assert_eq!(online::quick_match_pick(&rooms).unwrap().code, code);

    // 5. Quick match joins it (the join path an invite code takes).
    guest.app.quick_match();
    pump(
        &mut [&mut guest, &mut host],
        Duration::from_secs(40),
        "the guest to join and show the loop",
        |p| p[0].in_room() && p[0].app.media_ready && p[0].app.texture.is_some(),
    );
    assert_eq!(guest.snapshot().invite_code, code);
    assert!(!guest.app.is_host());
    assert_eq!(guest.app.media_hash, host.app.media_hash);
    assert!(
        !guest_net.saw("/media"),
        "a built-in loop is never downloaded"
    );
    let guest_id = guest.app.client_id.clone().unwrap();
    assert_ne!(guest_id, host_id);
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "both to see the same room",
        |p| {
            let (a, b) = (p[0].snapshot(), p[1].snapshot());
            a.revision == b.revision
                && a.id == b.id
                && a.host_id == b.host_id
                && a.pieces.len() == b.pieces.len()
                && a.pieces
                    .iter()
                    .zip(&b.pieces)
                    .all(|(x, y)| x.id == y.id && x.x == y.x && x.y == y.y && x.placed == y.placed)
                && p[0].app.peers.iter().any(|peer| peer == "Guest")
        },
    );

    // The guest drops a piece on its place; it snaps for both.
    let (cw, ch) = (
        guest.snapshot().cell_width(),
        guest.snapshot().cell_height(),
    );
    let piece = guest.snapshot().pieces[5].clone();
    let (x, y) = (f32::from(piece.column) * cw, f32::from(piece.row) * ch);
    guest.app.send(ClientMessage::DragStartSelection {
        piece_ids: vec![piece.id],
    });
    guest.app.send(ClientMessage::MoveSelection {
        anchor_piece_id: piece.id,
        x: x + 40.0,
        y: y + 40.0,
    });
    guest.app.send(ClientMessage::DragEndSelection {
        anchor_piece_id: piece.id,
        x,
        y,
    });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the dropped piece to snap for both",
        |p| {
            p.iter().all(|player| {
                player
                    .snapshot()
                    .pieces
                    .iter()
                    .any(|p| p.id == piece.id && p.placed && p.held_by.is_none())
            })
        },
    );
    assert_eq!(placed(&host), 1);

    // Chat goes round.
    guest.app.chat.draft = "hello from the guest".into();
    guest.app.send_chat();
    let said = |player: &Player| {
        player.app.chat.entries.iter().any(|entry| {
            matches!(entry, ChatEntry::Line(line)
                if line.text == "hello from the guest" && line.name == "Guest" && line.client_id == guest_id)
        })
    };
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the chat line on both",
        |p| said(&*p[0]) && said(&*p[1]),
    );

    // The lobby shows the progress within its throttle window.
    lobby_until(
        &mut [&mut host, &mut guest],
        1,
        Duration::from_secs(40),
        "the lobby to show two players and a placed piece",
        |rooms| entry(rooms, &code).is_some_and(|room| room.players == 2 && room.placed == 1),
    );

    // 6. The guest's network drops; it comes back as the same player.
    guest_net.cut();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the guest to notice the drop",
        |p| !p[1].app.connected,
    );
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(60),
        "the guest to reconnect",
        |p| p[1].in_room(),
    );
    assert_eq!(guest.app.client_id.as_deref(), Some(guest_id.as_str()));
    assert_eq!(placed(&guest), 1);

    // Leaving and rejoining from recent rooms keeps the identity too.
    guest.app.leave_room();
    guest.frame();
    assert!(guest.app.snapshot.is_none());
    guest.app.select_recent_room(0);
    assert_eq!(guest.app.invite_code, code);
    guest.app.join_room();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(40),
        "the guest to rejoin from recent rooms",
        |p| p[1].in_room() && p[1].app.media_ready,
    );
    assert_eq!(guest.app.client_id.as_deref(), Some(guest_id.as_str()));
    assert!(!guest.app.is_host());

    // The host's network drops while the guest plays on. As on a LAN
    // server, the host role passes to the player still there; the host
    // comes back as the same player.
    host_net.cut();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the host to notice the drop",
        |p| !p[0].app.connected,
    );
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(60),
        "the host to reconnect",
        |p| p[0].in_room() && p[0].app.peers.iter().any(|peer| peer == "Guest"),
    );
    assert_eq!(host.app.client_id.as_deref(), Some(host_id.as_str()));
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "both to agree on the host",
        |p| p[0].snapshot().host_id == p[1].snapshot().host_id,
    );
    assert!(
        guest.app.is_host(),
        "the role went to the player who stayed"
    );
    // Once the guest leaves, it is back with the creator.
    guest.app.leave_room();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the host role to come back",
        |p| p[0].app.is_host() && p[0].app.peers.is_empty(),
    );

    // Alone in the room, the host's network drops; the resume token brings
    // it back as the host, and the room is listed again.
    host_net.cut();
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(20),
        "the host to notice the drop",
        |p| !p[0].app.connected,
    );
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(60),
        "the host to reconnect",
        |p| p[0].in_room(),
    );
    assert_eq!(host.app.client_id.as_deref(), Some(host_id.as_str()));
    assert!(host.app.is_host(), "the host keeps the host role");
    assert!(host.snapshot().listed);
    assert_eq!(placed(&host), 1);
    lobby_until(
        &mut [&mut host, &mut guest],
        1,
        // A relisting waits out the 10 s throttle, then the next alarm.
        Duration::from_secs(40),
        "the room back in the lobby",
        |rooms| entry(rooms, &code).is_some_and(|room| room.players == 1),
    );

    // 8. Delisting (the host's "Listed publicly" toggle) leaves the lobby.
    host.app.send(ClientMessage::SetListed { listed: false });
    pump(
        &mut [&mut host, &mut guest],
        Duration::from_secs(10),
        "the host to see the room unlisted",
        |p| !p[0].snapshot().listed,
    );
    lobby_until(
        &mut [&mut host, &mut guest],
        1,
        Duration::from_secs(20),
        "the room to leave the lobby",
        |rooms| entry(rooms, &code).is_none(),
    );

    // 7. Uploads are off: the host's own picture is refused with the
    // service's words, by the app and (when the app does not know) by the
    // Worker.
    host.app.leave_room();
    host.frame();
    let own = root.join("my picture.png");
    image::RgbImage::from_fn(96, 64, |x, y| image::Rgb([x as u8, y as u8, 128]))
        .save(&own)
        .unwrap();
    host.open_video(&own);
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the picture to load",
        |p| p[0].app.media_path.as_deref() == Some(own.as_path()),
    );
    assert!(!pieceoff_core::is_builtin_media(&host.app.media_hash));
    assert!(host.app.custom_media_blocked());
    host.app.create_room();
    assert_eq!(host.app.status, online::BUILTIN_ONLY_MESSAGE);
    assert!(host.app.network.is_none() && !host.app.online_busy());
    // Pretend the service had not said uploads are off: the Worker refuses.
    let mut told_open = info.clone();
    told_open.uploads = true;
    host.app
        .online
        .remember_service(&host_net.url(), online::ServiceKind::Hosted(told_open));
    host.app.create_room();
    pump(
        &mut [&mut host],
        Duration::from_secs(20),
        "the Worker's refusal",
        |p| !p[0].app.online_busy(),
    );
    assert_eq!(
        host.app.status,
        format!(
            "Could not create the room: {}",
            online::BUILTIN_ONLY_MESSAGE
        )
    );
    assert!(host.app.network.is_none() && host.app.snapshot.is_none());
    assert!(
        host.app.current_device_linked(),
        "a refusal is not a relink"
    );
    let _ = std::fs::remove_dir_all(&root);
}
