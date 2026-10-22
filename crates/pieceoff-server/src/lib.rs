pub mod config;
pub mod limits;

use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::{
        ConnectInfo, Path as AxumPath, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use config::Config;
use futures_util::StreamExt;
use limits::{RateLimiter, client_address, origin_allowed};
use pieceoff_core::{
    ClientMessage, MediaItem, PROTOCOL_VERSION, RoomRuntime, RoomSnapshot, ServerMessage,
    clean_media_extension, clean_name, clean_title, valid_media_dimensions,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs as tokio_fs,
    io::AsyncWriteExt,
    net::TcpListener,
    sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, broadcast},
};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    rooms: Arc<RwLock<HashMap<String, Arc<RoomRecord>>>>,
    data_dir: Arc<PathBuf>,
    media_quota_bytes: u64,
    max_media_bytes: u64,
    /// Held only while a finished upload is installed and while unreferenced
    /// media is collected, never while bytes are streaming in.
    media_install_lock: Arc<Mutex<()>>,
    /// Bytes promised to uploads that have not finished, so concurrent
    /// uploads cannot together overshoot the quota.
    media_reserved: Arc<std::sync::Mutex<u64>>,
    room_ttl: Duration,
    max_rooms: usize,
    max_message_bytes: usize,
    trust_proxy: bool,
    allowed_origins: Arc<Vec<String>>,
    /// One permit per open socket; a server at its limit refuses more.
    connection_slots: Arc<Semaphore>,
    /// New sockets per address.
    connect_limiter: Arc<RateLimiter>,
    /// Joins with an unknown room code, per address: the guess budget.
    join_failure_limiter: Arc<RateLimiter>,
}

impl AppState {
    #[cfg(test)]
    fn new(data_dir: PathBuf) -> Self {
        Self::from_config(&Config::defaults(data_dir))
    }

    fn from_config(config: &Config) -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            data_dir: Arc::new(config.data_dir.clone()),
            media_quota_bytes: config.media_quota_bytes,
            max_media_bytes: config.max_media_bytes,
            media_install_lock: Arc::new(Mutex::new(())),
            media_reserved: Arc::new(std::sync::Mutex::new(0)),
            room_ttl: Duration::from_secs(config.room_ttl_days.saturating_mul(24 * 60 * 60)),
            max_rooms: config.max_rooms,
            max_message_bytes: config.max_message_bytes,
            trust_proxy: config.trust_proxy,
            allowed_origins: Arc::new(config.allowed_origins.clone()),
            connection_slots: Arc::new(Semaphore::new(config.max_connections)),
            connect_limiter: Arc::new(RateLimiter::per_period(
                config.connections_per_minute,
                Duration::from_secs(60),
            )),
            join_failure_limiter: Arc::new(RateLimiter::per_period(
                config.failed_joins_per_hour,
                Duration::from_secs(60 * 60),
            )),
        }
    }
}

struct RoomRecord {
    runtime: Mutex<RoomRuntime>,
    events: broadcast::Sender<ServerMessage>,
    persist_lock: Mutex<()>,
    media_upload_token: Mutex<Option<String>>,
    last_activity: Arc<Mutex<SystemTime>>,
}

impl RoomRecord {
    fn new(media_upload_token: Option<String>) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            runtime: Mutex::new(RoomRuntime::default()),
            events,
            persist_lock: Mutex::new(()),
            media_upload_token: Mutex::new(media_upload_token),
            last_activity: Arc::new(Mutex::new(SystemTime::now())),
        }
    }

    fn with_runtime(runtime: RoomRuntime, last_activity: SystemTime) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            runtime: Mutex::new(runtime),
            events,
            persist_lock: Mutex::new(()),
            media_upload_token: Mutex::new(None),
            last_activity: Arc::new(Mutex::new(last_activity)),
        }
    }

    async fn touch(&self) {
        *self.last_activity.lock().await = SystemTime::now();
    }
}

/// Run the room server until it is told to stop (Ctrl-C or SIGTERM), or
/// handle `--healthcheck` / `--version` style arguments. Returns the process
/// exit code. This is the whole of the `pieceoff-server` binary.
pub async fn run() -> i32 {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--version") => {
            println!("pieceoff-server {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        Some("--help" | "-h") => {
            println!(
                "pieceoff-server {}\n\nUsage: pieceoff-server [--healthcheck | --version]\n\n\
                 Settings come from PIECEOFF_* environment variables; see docs/DEPLOYMENT.md.\n\
                 --healthcheck  ask a running server (at PIECEOFF_BIND) whether it is up; exits 0 if so",
                env!("CARGO_PKG_VERSION")
            );
            return 0;
        }
        Some("--healthcheck") => {
            let healthy = match Config::from_env() {
                Ok(config) => health_check(config.bind).await,
                Err(error) => {
                    eprintln!("{error}");
                    false
                }
            };
            return if healthy { 0 } else { 1 };
        }
        Some(other) => {
            eprintln!("Unknown option '{other}'. Try --help.");
            return 2;
        }
        None => {}
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("PieceOff room server cannot start: {error}");
            return 2;
        }
    };
    let state = AppState::from_config(&config);
    println!("PieceOff room server {}", env!("CARGO_PKG_VERSION"));
    for line in config.summary() {
        println!("  {line}");
    }
    load_rooms(&state).await;
    cleanup_expired_rooms(&state).await;
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            cleanup_expired_rooms(&cleanup_state).await;
            cleanup_state.connect_limiter.forget_idle();
            cleanup_state.join_failure_limiter.forget_idle();
        }
    });
    let app = application(state.clone());
    let addr = config.bind;
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("PieceOff room server cannot listen on {addr}: {error}");
            if error.kind() == std::io::ErrorKind::AddrInUse {
                eprintln!(
                    "Another program is already using that port. Stop it, or pick a free \
                     address with PIECEOFF_BIND (for example PIECEOFF_BIND=127.0.0.1:28879) \
                     and point the app's Server field at it."
                );
            }
            return 1;
        }
    };
    println!("PieceOff room server listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("serve room server");
    // Moves between drops are not saved as they happen; save every room
    // before exiting so a restart resumes where players left off.
    println!("Shutting down; saving rooms");
    persist_all(&state).await;
    0
}

/// A room server running inside this process on its own runtime: what
/// integration tests (and anything else that wants a server without spawning
/// one) use. Dropping it, or calling [`RunningServer::crash`], stops the
/// runtime, which closes every open connection at once like a killed process.
pub struct RunningServer {
    runtime: Option<tokio::runtime::Runtime>,
    state: AppState,
    address: SocketAddr,
}

impl RunningServer {
    /// Start a server for `config`, restoring any rooms saved in its data
    /// directory. Pass `config.bind` as `127.0.0.1:0` for a free port, or a
    /// fixed port to bring a "restarted" server back at the same address.
    pub fn start(config: &Config) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let state = AppState::from_config(config);
        let (listener, app) = runtime.block_on(async {
            load_rooms(&state).await;
            let listener = TcpListener::bind(config.bind).await?;
            Ok::<_, std::io::Error>((listener, application(state.clone())))
        })?;
        let address = listener.local_addr()?;
        runtime.spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        Ok(Self {
            runtime: Some(runtime),
            state,
            address,
        })
    }

    /// The WebSocket URL players use.
    pub fn url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Save every room, then stop (what SIGTERM does).
    pub fn stop(mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.block_on(persist_all(&self.state));
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }

    /// Stop at once without saving, like a killed process: moves not yet
    /// persisted are lost and every connection drops.
    pub fn crash(mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_millis(100));
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Resolves on Ctrl-C, or SIGTERM (what `docker stop` and systemd send).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

async fn persist_all(state: &AppState) {
    let rooms: Vec<Arc<RoomRecord>> = state.rooms.read().await.values().cloned().collect();
    for room in rooms {
        persist_room(state, &room).await;
    }
}

/// `GET /health` against a server on this machine, with no HTTP client
/// library: this runs in minimal container images with no curl.
async fn health_check(bind: SocketAddr) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let target = if bind.ip().is_unspecified() {
        SocketAddr::new(
            if bind.is_ipv4() {
                IpAddr::from([127, 0, 0, 1])
            } else {
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1])
            },
            bind.port(),
        )
    } else {
        bind
    };
    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect(target).await.ok()?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .ok()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.ok()?;
        Some(response.starts_with(b"HTTP/1.1 200"))
    };
    tokio::time::timeout(Duration::from_secs(3), attempt)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// Advance every room's queue when its current item has played out. The
/// server owns this so all clients change item together.
fn spawn_queue_ticker(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            interval.tick().await;
            let rooms: Vec<Arc<RoomRecord>> = state.rooms.read().await.values().cloned().collect();
            for room in rooms {
                let advanced = {
                    let mut runtime = room.runtime.lock().await;
                    match runtime.tick_advance(now_ms()) {
                        Some(snapshot) => {
                            // Sent under the lock so it stays in revision
                            // order with every other event (see `dispatch`).
                            let _ = room.events.send(ServerMessage::Snapshot { snapshot });
                            true
                        }
                        None => false,
                    }
                };
                if advanced {
                    persist_room(&state, &room).await;
                }
            }
        }
    });
}

fn application(state: AppState) -> Router {
    spawn_queue_ticker(state.clone());
    Router::new()
        .route(
            "/health",
            get(|| async { "PieceOff room server is running" }),
        )
        .route("/ws", get(websocket_handler))
        .route("/media/{sha256}", get(download_media).put(upload_media))
        .with_state(state)
}

#[derive(Deserialize)]
struct SocketQuery {
    /// The protocol version the client speaks. Clients older than this check
    /// send none and are let through as before.
    protocol: Option<u16>,
}

/// Why a client speaking `client` cannot use this server, if it cannot.
fn protocol_mismatch(client: Option<u16>) -> Option<String> {
    let client = client?;
    if client == PROTOCOL_VERSION {
        return None;
    }
    let newer = if client < PROTOCOL_VERSION {
        "server"
    } else {
        "app"
    };
    Some(format!(
        "This PieceOff {newer} is newer (app protocol v{client}, server protocol \
         v{PROTOCOL_VERSION}). Update so the app and the room server match."
    ))
}

#[derive(Deserialize)]
struct MediaQuery {
    code: Option<String>,
    token: Option<String>,
    /// An upload with `probe=1` only asks whether the server already holds
    /// the asset: 201 if so (and the item is marked available), 204 if the
    /// client should send it. Clients probe first so they never stream a
    /// body the server will not read.
    #[serde(default)]
    probe: Option<u8>,
}

const ROOM_NOT_FOUND: &str = "Room code not found";

/// Space promised to one in-flight upload; returned when it ends.
struct MediaReservation {
    reserved: Arc<std::sync::Mutex<u64>>,
    bytes: u64,
}

impl Drop for MediaReservation {
    fn drop(&mut self) {
        if let Ok(mut total) = self.reserved.lock() {
            *total = total.saturating_sub(self.bytes);
        }
    }
}

/// The byte range a `Range` header asks for, if it is a single `bytes=`
/// range we understand. `Ok(None)` means serve everything.
fn requested_range(headers: &axum::http::HeaderMap, length: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(value) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Ok(None);
    };
    let (start, end) = match (first.trim(), last.trim()) {
        ("", suffix) => {
            let Ok(suffix) = suffix.parse::<u64>() else {
                return Ok(None);
            };
            if suffix == 0 {
                return Err(());
            }
            (length.saturating_sub(suffix), length.saturating_sub(1))
        }
        (start, "") => {
            let Ok(start) = start.parse::<u64>() else {
                return Ok(None);
            };
            (start, length.saturating_sub(1))
        }
        (start, end) => {
            let (Ok(start), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) else {
                return Ok(None);
            };
            (start, end.min(length.saturating_sub(1)))
        }
    };
    if length == 0 || start >= length || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

async fn download_media(
    State(state): State<AppState>,
    AxumPath(sha256): AxumPath<String>,
    Query(query): Query<MediaQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(code) = query.code.map(|code| code.trim().to_ascii_uppercase()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let room = state.rooms.read().await.get(&code).cloned();
    let Some(room) = room else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let snapshot = room.runtime.lock().await.snapshot.clone();
    let Some(item) = snapshot
        .as_ref()
        .and_then(|snapshot| queue_item(snapshot, &sha256))
        .filter(|item| item.available)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = media_path(&state.data_dir, &item.sha256, &item.extension);
    let Ok(mut file) = tokio_fs::File::open(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(metadata) = file.metadata().await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let total = metadata.len();
    let (start, end, partial) = match requested_range(&headers, total) {
        Ok(Some((start, end))) => (start, end, true),
        Ok(None) => (0, total.saturating_sub(1), false),
        Err(()) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    if start > 0 {
        use tokio::io::AsyncSeekExt;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let slice = if total == 0 { 0 } else { end - start + 1 };
    let stream = futures_util::stream::try_unfold((file, slice), |(mut file, left)| async move {
        use tokio::io::AsyncReadExt;
        if left == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        let mut buffer = vec![0u8; (64 * 1024).min(left as usize)];
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            Ok(None)
        } else {
            buffer.truncate(count);
            Ok(Some((Bytes::from(buffer), (file, left - count as u64))))
        }
    });
    let mut response = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, slice);
    if partial {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    } else {
        response = response.status(StatusCode::OK);
    }
    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn upload_media(
    State(state): State<AppState>,
    AxumPath(sha256): AxumPath<String>,
    Query(query): Query<MediaQuery>,
    body: Body,
) -> Response {
    if !valid_sha256(&sha256) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(token) = query.token else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rooms: Vec<Arc<RoomRecord>> = state.rooms.read().await.values().cloned().collect();
    let mut target = None;
    for room in rooms {
        if room.media_upload_token.lock().await.as_deref() != Some(token.as_str()) {
            continue;
        }
        let snapshot = room.runtime.lock().await.snapshot.clone();
        if snapshot
            .as_ref()
            .is_some_and(|snapshot| queue_item(snapshot, &sha256).is_some())
        {
            target = Some((room, snapshot.unwrap()));
            break;
        }
    }
    let Some((room, snapshot)) = target else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let declared = body.size_hint().exact();
    if declared.is_some_and(|length| length > state.max_media_bytes) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let directory = state.data_dir.join("media");
    if let Err(error) = tokio_fs::create_dir_all(&directory).await {
        eprintln!("Could not create media directory: {error}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let extension = queue_item(&snapshot, &sha256)
        .map(|item| item.extension.clone())
        .unwrap_or_default();
    let path = media_path(&state.data_dir, &sha256, &extension);
    // Content-addressed assets can be reused by another room. This also
    // repairs a room interrupted after the asset was installed but before its
    // availability flag was persisted.
    if tokio_fs::metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        mark_item_available(&state, &room, &sha256).await;
        return StatusCode::CREATED.into_response();
    }
    if query.probe.is_some() {
        return StatusCode::NO_CONTENT.into_response();
    }
    // Promise this upload its space up front, so that several large uploads
    // at once cannot together pass the quota, and none of them has to wait
    // for the others to finish.
    let used = match media_storage_used_bytes(&directory).await {
        Ok(used) => used,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let reservation = {
        let Ok(mut reserved) = state.media_reserved.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let free = state
            .media_quota_bytes
            .saturating_sub(used)
            .saturating_sub(*reserved);
        let wanted = declared.unwrap_or_else(|| state.max_media_bytes.min(free));
        if wanted == 0 || wanted > free {
            return StatusCode::INSUFFICIENT_STORAGE.into_response();
        }
        *reserved += wanted;
        MediaReservation {
            reserved: state.media_reserved.clone(),
            bytes: wanted,
        }
    };
    let upload_limit = reservation.bytes;
    let temporary = directory.join(format!(".{sha256}.{}.part", Uuid::new_v4()));
    let mut file = match tokio_fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
    {
        Ok(file) => file,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut stream = body.into_data_stream();
    let mut hasher = Sha256::new();
    let mut length = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                drop(file);
                let _ = tokio_fs::remove_file(&temporary).await;
                return StatusCode::BAD_REQUEST.into_response();
            }
        };
        length = length.saturating_add(chunk.len() as u64);
        if length > upload_limit {
            drop(file);
            let _ = tokio_fs::remove_file(&temporary).await;
            return if length > state.max_media_bytes {
                StatusCode::PAYLOAD_TOO_LARGE.into_response()
            } else {
                StatusCode::INSUFFICIENT_STORAGE.into_response()
            };
        }
        hasher.update(&chunk);
        if file.write_all(&chunk).await.is_err() {
            drop(file);
            let _ = tokio_fs::remove_file(&temporary).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if file.sync_all().await.is_err() {
        drop(file);
        let _ = tokio_fs::remove_file(&temporary).await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    drop(file);
    let received_hash = format!("{:x}", hasher.finalize());
    if received_hash != sha256 {
        let _ = tokio_fs::remove_file(&temporary).await;
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    {
        // Installing is the only step that must not race with collecting
        // unreferenced media, and it is quick: the bytes are already on disk.
        let _install = state.media_install_lock.lock().await;
        let still_hosted = state
            .rooms
            .read()
            .await
            .values()
            .any(|hosted| Arc::ptr_eq(hosted, &room));
        if !still_hosted {
            let _ = tokio_fs::remove_file(&temporary).await;
            return StatusCode::GONE.into_response();
        }
        if let Err(error) = tokio_fs::hard_link(&temporary, &path).await {
            let _ = tokio_fs::remove_file(&temporary).await;
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        } else {
            let _ = tokio_fs::remove_file(&temporary).await;
        }
    }
    drop(reservation);
    mark_item_available(&state, &room, &sha256).await;
    StatusCode::CREATED.into_response()
}

async fn media_storage_used_bytes(directory: &Path) -> std::io::Result<u64> {
    let mut entries = tokio_fs::read_dir(directory).await?;
    let mut total = 0u64;
    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        // Partial uploads are accounted for by their reservations.
        let in_progress = entry.file_name().to_string_lossy().starts_with('.');
        if metadata.is_file() && !in_progress {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

async fn mark_item_available(state: &AppState, room: &Arc<RoomRecord>, sha256: &str) {
    let changed = {
        let mut runtime = room.runtime.lock().await;
        if let Some(snapshot) = runtime.snapshot.as_mut() {
            for item in &mut snapshot.queue {
                if item.sha256 == sha256 {
                    item.available = true;
                }
            }
            snapshot.sync_current_mirror();
            snapshot.revision = snapshot.revision.saturating_add(1);
            // Sent under the lock, in revision order with every other event
            // (see `dispatch`).
            let _ = room.events.send(ServerMessage::Snapshot {
                snapshot: snapshot.clone(),
            });
            true
        } else {
            false
        }
    };
    if changed {
        persist_room(state, room).await;
    }
}

fn media_path(data_dir: &Path, sha256: &str, extension: &str) -> PathBuf {
    let safe_sha = if valid_sha256(sha256) {
        sha256
    } else {
        "invalid"
    };
    let safe_extension = clean_media_extension(extension).unwrap_or_else(|_| "bin".into());
    data_dir
        .join("media")
        .join(format!("{safe_sha}.{safe_extension}"))
}

/// Characters for invite codes: no 0/O or 1/I, so a code read aloud or off a
/// screen cannot be mistaken. Exactly 32 of them, so a random byte maps onto
/// the alphabet without bias.
const INVITE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const INVITE_CODE_LENGTH: usize = 8;

/// A random code of 8 characters from a 32-character alphabet (40 bits): far
/// too many to guess within the join-failure budget.
fn new_invite_code() -> String {
    Uuid::new_v4()
        .as_bytes()
        .iter()
        .take(INVITE_CODE_LENGTH)
        .map(|byte| INVITE_ALPHABET[(byte % 32) as usize] as char)
        .collect()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn has_unavailable_item(snapshot: &RoomSnapshot) -> bool {
    snapshot.queue.iter().any(|item| !item.available)
}

fn queue_item<'a>(snapshot: &'a RoomSnapshot, sha256: &str) -> Option<&'a MediaItem> {
    snapshot.queue.iter().find(|item| item.sha256 == sha256)
}

async fn websocket_handler(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
    Query(query): Query<SocketQuery>,
    request: axum::extract::Request,
) -> Response {
    // Present when served with connect info (the real server); absent in
    // tests that serve the router directly.
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let headers = request.headers();
    let header_text = |name| headers.get(name).and_then(|value| value.to_str().ok());
    if !origin_allowed(
        header_text(header::ORIGIN),
        header_text(header::HOST),
        &state.allowed_origins,
    ) {
        return (StatusCode::FORBIDDEN, "Browser origin not allowed").into_response();
    }
    let address = client_address(peer, headers, state.trust_proxy);
    if let Some(address) = address {
        if !state.connect_limiter.try_take(address, 1.0) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "Too many connections; slow down",
            )
                .into_response();
        }
    }
    let Ok(slot) = state.connection_slots.clone().try_acquire_owned() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "The server is full").into_response();
    };
    let limit = state.max_message_bytes;
    upgrade
        .max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |mut socket| async move {
            // Refuse a mismatched app up front, with a message every version
            // can show, instead of messages it cannot parse.
            if let Some(message) = protocol_mismatch(query.protocol) {
                let _ = send(&mut socket, &ServerMessage::Error { message }).await;
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            handle_socket(socket, state, address, slot).await
        })
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    address: Option<IpAddr>,
    _slot: OwnedSemaphorePermit,
) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else {
        return;
    };
    let Ok(first) = serde_json::from_str::<ClientMessage>(&first) else {
        let _ = send(
            &mut socket,
            &ServerMessage::Error {
                message: "Invalid first message".into(),
            },
        )
        .await;
        return;
    };

    let client_id = Uuid::new_v4().to_string();
    let is_join = matches!(first, ClientMessage::JoinRoom { .. });
    if is_join
        && address.is_some_and(|address| !state.join_failure_limiter.would_allow(address, 1.0))
    {
        let _ = send(
            &mut socket,
            &ServerMessage::Error {
                message: "Too many wrong room codes from this address. Try again later.".into(),
            },
        )
        .await;
        return;
    }
    let (room, room_code, name, created) = match connect_room(&state, &client_id, first).await {
        Ok(value) => value,
        Err(message) => {
            // Every unknown code is a guess; spend from this address's budget.
            if is_join && message == ROOM_NOT_FOUND {
                if let Some(address) = address {
                    state.join_failure_limiter.try_take(address, 1.0);
                }
            }
            let _ = send(&mut socket, &ServerMessage::Error { message }).await;
            return;
        }
    };
    persist_room(&state, &room).await;
    let mut events = room.events.subscribe();
    let peers = {
        let runtime = room.runtime.lock().await;
        runtime
            .names
            .iter()
            .filter(|(id, _)| *id != &client_id)
            .map(|(_, name)| name.clone())
            .collect()
    };
    let snapshot = room
        .runtime
        .lock()
        .await
        .snapshot
        .clone()
        .expect("room snapshot exists");
    let media_upload_token = if snapshot.host_id == client_id && has_unavailable_item(&snapshot) {
        room.media_upload_token.lock().await.clone()
    } else {
        None
    };
    if send(
        &mut socket,
        &ServerMessage::Welcome {
            client_id: client_id.clone(),
            name: name.clone(),
            peers,
            snapshot,
            media_upload_token,
        },
    )
    .await
    .is_err()
    {
        remove_peer(&room, &client_id, &name).await;
        persist_room(&state, &room).await;
        return;
    }
    if !created {
        let _ = room
            .events
            .send(ServerMessage::PeerJoined { name: name.clone() });
    }
    println!("{} joined room {} as {}", name, room_code, client_id);

    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.tick().await;
    let mut last_pong = Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= Duration::from_secs(60)
                    || socket.send(Message::Ping(Bytes::new())).await.is_err()
                {
                    break;
                }
            }
            event = events.recv() => match event {
                Ok(packet) => {
                    if packet.visible_to(&client_id)
                        && send(&mut socket, &packet).await.is_err()
                    {
                        break
                    }
                },
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if let Some(snapshot) = room.runtime.lock().await.snapshot.clone() {
                        if send(&mut socket, &ServerMessage::Snapshot { snapshot }).await.is_err() { break }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else {
                        let _ = send(&mut socket, &ServerMessage::Error { message: "Invalid message".into() }).await;
                        continue;
                    };
                    let queued_sha = match &message {
                        ClientMessage::QueueAdd { sha256, .. } => Some(sha256.to_ascii_lowercase()),
                        _ => None,
                    };
                    match dispatch(&room, &client_id, message).await {
                        Ok(true) => {
                            persist_room(&state, &room).await;
                            if let Some(sha256) = queued_sha {
                                after_queue_add(&state, &room, &client_id, &sha256).await;
                            }
                        }
                        Ok(false) => {},
                        Err(err) => {
                            let _ = send(&mut socket, &ServerMessage::Error { message: err }).await;
                            if let Some(snapshot) = room.runtime.lock().await.snapshot.clone() {
                                let _ = send(&mut socket, &ServerMessage::Snapshot { snapshot }).await;
                            }
                        }
                    }
                }
                Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
    remove_peer(&room, &client_id, &name).await;
    persist_room(&state, &room).await;
}

async fn connect_room(
    state: &AppState,
    client_id: &str,
    first: ClientMessage,
) -> Result<(Arc<RoomRecord>, String, String, bool), String> {
    match first {
        ClientMessage::CreateRoom {
            name,
            title,
            media_sha256,
            media_extension,
            width,
            height,
            rows,
            columns,
            initial_snapshot,
            duration_ms,
        } => {
            if !valid_sha256(&media_sha256) {
                return Err("Invalid media fingerprint".into());
            }
            let name = clean_name(&name);
            let room_id = Uuid::new_v4().to_string();
            let invite_code = new_invite_code();
            let clean_title = clean_title(&title);
            let clean_extension = clean_media_extension(&media_extension)?;
            let snapshot = if let Some(initial) = initial_snapshot {
                let initial = *initial;
                if initial.title != clean_title
                    || initial.media_sha256 != media_sha256
                    || initial.media_extension != clean_extension
                    || initial.width != width
                    || initial.height != height
                    || initial.rows != rows
                    || initial.columns != columns
                {
                    return Err("Imported puzzle snapshot does not match its media or grid".into());
                }
                initial.rehost(room_id, invite_code.clone(), client_id.to_owned())?
            } else {
                let mut snapshot = RoomSnapshot::create(
                    room_id,
                    invite_code.clone(),
                    clean_title,
                    media_sha256,
                    clean_extension,
                    width,
                    height,
                    rows,
                    columns,
                    client_id.to_owned(),
                )?;
                snapshot.revision = 1;
                if let Some(first) = snapshot.queue.first_mut() {
                    first.duration_ms = duration_ms;
                }
                snapshot
            };
            let media_upload_token = Uuid::new_v4().to_string();
            let room = Arc::new(RoomRecord::new(Some(media_upload_token)));
            {
                let mut runtime = room.runtime.lock().await;
                runtime.host_id = Some(client_id.to_owned());
                runtime.names.insert(client_id.to_owned(), name.clone());
                runtime.snapshot = Some(snapshot);
            }
            {
                let mut rooms = state.rooms.write().await;
                if rooms.len() >= state.max_rooms {
                    return Err(
                        "This server is full. Try again later, or use another server.".into(),
                    );
                }
                // A code is only a handful of random characters, so two rooms
                // can collide; never replace a room that already exists.
                if rooms.contains_key(&invite_code) {
                    return Err("Could not pick a free room code. Please try again.".into());
                }
                rooms.insert(invite_code.clone(), room.clone());
            }
            Ok((room, invite_code, name, true))
        }
        ClientMessage::JoinRoom {
            invite_code,
            name,
            media_sha256,
            media_extension: _,
        } => {
            let invite_code = invite_code.trim().to_ascii_uppercase();
            let room = state
                .rooms
                .read()
                .await
                .get(&invite_code)
                .cloned()
                .ok_or(ROOM_NOT_FOUND.to_string())?;
            {
                let mut runtime = room.runtime.lock().await;
                let snapshot = runtime
                    .snapshot
                    .as_ref()
                    .ok_or("Room is not ready".to_string())?;
                if media_sha256 != snapshot.media_sha256 && !snapshot.media_available {
                    return Err("The room media is still uploading. Wait for the host and try joining again.".into());
                }
                if runtime.names.len() >= 8 {
                    return Err("This room already has 8 players".into());
                }
                runtime
                    .names
                    .insert(client_id.to_owned(), clean_name(&name));
                let host_present = runtime
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| runtime.names.contains_key(&snapshot.host_id));
                if !host_present {
                    if let Some(snapshot) = runtime.snapshot.as_mut() {
                        snapshot.host_id = client_id.to_owned();
                        snapshot.revision += 1;
                        if has_unavailable_item(snapshot) {
                            *room.media_upload_token.lock().await =
                                Some(Uuid::new_v4().to_string());
                        }
                    }
                }
            }
            Ok((room, invite_code, clean_name(&name), false))
        }
        _ => Err("First message must create or join a room".into()),
    }
}

/// A freshly queued item is available at once when the server already holds
/// those bytes; otherwise the host is asked to upload it.
async fn after_queue_add(state: &AppState, room: &Arc<RoomRecord>, client_id: &str, sha256: &str) {
    let extension = {
        let runtime = room.runtime.lock().await;
        let Some(item) = runtime
            .snapshot
            .as_ref()
            .and_then(|snapshot| queue_item(snapshot, sha256))
        else {
            return;
        };
        item.extension.clone()
    };
    if media_path(&state.data_dir, sha256, &extension).is_file() {
        mark_item_available(state, room, sha256).await;
        return;
    }
    let token = {
        let mut slot = room.media_upload_token.lock().await;
        slot.get_or_insert_with(|| Uuid::new_v4().to_string())
            .clone()
    };
    let _ = room.events.send(ServerMessage::MediaUploadToken {
        client_id: client_id.to_owned(),
        token,
        sha256: Some(sha256.to_owned()),
    });
}

async fn dispatch(
    room: &Arc<RoomRecord>,
    client_id: &str,
    message: ClientMessage,
) -> Result<bool, String> {
    // The room lock is held from the change through the broadcast. Each
    // change takes the next revision, and clients ignore anything older than
    // what they have, so events must leave in revision order: sending after
    // the lock was released let two players' moves overtake each other, and a
    // client then dropped the older one.
    let mut runtime = room.runtime.lock().await;
    let outcome = runtime.apply(client_id, message, now_ms())?;
    let _ = room.events.send(outcome.event);
    Ok(outcome.persist)
}

async fn persist_room(state: &AppState, room: &Arc<RoomRecord>) {
    let _guard = room.persist_lock.lock().await;
    room.touch().await;
    let Some(snapshot) = room.runtime.lock().await.snapshot.clone() else {
        return;
    };
    let dir = (*state.data_dir).clone();
    let _ = tokio::task::spawn_blocking(move || persist_snapshot(&dir, &snapshot)).await;
}

fn persist_snapshot(data_dir: &Path, snapshot: &RoomSnapshot) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let basename = format!(
        "{}-{:020}-{}",
        snapshot.invite_code, snapshot.revision, stamp
    );
    let destination = data_dir.join(format!("{basename}.json"));
    let temporary = data_dir.join(format!("{basename}.tmp"));
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(std::io::Error::other)?;
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &destination)?;
    // Commit the uniquely named new snapshot first. This keeps replacement
    // atomic on Windows, where rename cannot replace an existing destination.
    let prefix = format!("{}-", snapshot.invite_code);
    for entry in fs::read_dir(data_dir)?.filter_map(Result::ok) {
        let path = entry.path();
        if path != destination
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
        {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

async fn load_rooms(state: &AppState) {
    let directory = (*state.data_dir).clone();
    let snapshots = tokio::task::spawn_blocking(move || {
        let Ok(entries) = fs::read_dir(&directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or_else(|_| SystemTime::now());
                fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<RoomSnapshot>(&bytes).ok())
                    .map(|snapshot| (snapshot, modified))
            })
            .filter(|(snapshot, _)| {
                matches!(
                    snapshot.protocol_version,
                    2..=pieceoff_core::PROTOCOL_VERSION
                ) && valid_media_dimensions(snapshot.width, snapshot.height)
            })
            .map(|(mut snapshot, modified)| {
                snapshot.migrate();
                // The media file and room snapshot are committed separately.
                // If the process stops after the content-addressed file is
                // installed but before this flag is persisted, recover from
                // the file itself instead of preserving a stale `false`.
                for item in &mut snapshot.queue {
                    item.available =
                        media_path(&directory, &item.sha256, &item.extension).is_file();
                }
                snapshot.sync_current_mirror();
                (snapshot, modified)
            })
            .fold(
                HashMap::<String, (RoomSnapshot, SystemTime)>::new(),
                |mut latest, (snapshot, modified)| {
                    let entry = latest
                        .entry(snapshot.invite_code.clone())
                        .or_insert_with(|| (snapshot.clone(), modified));
                    if snapshot.revision > entry.0.revision
                        || (snapshot.revision == entry.0.revision && modified > entry.1)
                    {
                        *entry = (snapshot, modified);
                    }
                    latest
                },
            )
            .into_values()
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut rooms = state.rooms.write().await;
    for (mut snapshot, last_activity) in snapshots {
        for piece in &mut snapshot.pieces {
            piece.held_by = None;
        }
        let code = snapshot.invite_code.clone();
        let mut runtime = RoomRuntime::default();
        runtime.host_id = Some(snapshot.host_id.clone());
        runtime.snapshot = Some(snapshot);
        rooms.insert(
            code,
            Arc::new(RoomRecord::with_runtime(runtime, last_activity)),
        );
    }
    if !rooms.is_empty() {
        println!("Restored {} saved rooms", rooms.len());
    }
}

async fn cleanup_expired_rooms(state: &AppState) -> usize {
    if state.room_ttl.is_zero() {
        return 0;
    }
    let mut rooms = state.rooms.write().await;
    let mut expired = Vec::new();
    for (code, room) in rooms.iter() {
        if !room.runtime.lock().await.names.is_empty() {
            continue;
        }
        let last_activity = *room.last_activity.lock().await;
        if last_activity.elapsed().unwrap_or_default() >= state.room_ttl {
            expired.push(code.clone());
        }
    }
    if expired.is_empty() {
        return 0;
    }

    let expired_set: HashSet<_> = expired.iter().cloned().collect();
    let mut referenced_media = HashSet::new();
    for (code, room) in rooms.iter() {
        if expired_set.contains(code) {
            continue;
        }
        if let Some(snapshot) = &room.runtime.lock().await.snapshot {
            referenced_media.insert(format!(
                "{}.{}",
                snapshot.media_sha256, snapshot.media_extension
            ));
            for item in &snapshot.queue {
                referenced_media.insert(format!("{}.{}", item.sha256, item.extension));
            }
        }
    }

    // Do not let uploads create an unreferenced asset while the media tree is
    // being collected. Keep the room map locked too, so a join cannot race
    // with expiration after the activity check.
    let _upload_guard = state.media_install_lock.lock().await;
    let directory = (*state.data_dir).clone();
    let expired_codes = expired.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        let mut removed_assets = 0usize;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if expired_codes
                .iter()
                .any(|code| name.starts_with(&format!("{code}-")) || name == format!("{code}.json"))
            {
                fs::remove_file(entry.path())?;
            }
        }
        let media_directory = directory.join("media");
        if media_directory.is_dir() {
            for entry in fs::read_dir(media_directory)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    && !referenced_media.contains(&entry.file_name().to_string_lossy().to_string())
                {
                    fs::remove_file(entry.path())?;
                    removed_assets += 1;
                }
            }
        }
        Ok::<_, std::io::Error>(removed_assets)
    })
    .await;

    match deleted {
        Ok(Ok(removed_assets)) => {
            for code in &expired {
                rooms.remove(code);
            }
            println!(
                "Expired {} inactive rooms and removed {removed_assets} unreferenced media files",
                expired.len()
            );
            expired.len()
        }
        Ok(Err(error)) => {
            eprintln!("Could not finish expired room cleanup: {error}");
            0
        }
        Err(error) => {
            eprintln!("Room cleanup task failed: {error}");
            0
        }
    }
}

async fn remove_peer(room: &Arc<RoomRecord>, client_id: &str, fallback_name: &str) {
    // Events go out under the room lock so they leave in revision order (see
    // `dispatch`).
    let mut runtime = room.runtime.lock().await;
    runtime.forget_drag(client_id);
    let name = runtime
        .names
        .remove(client_id)
        .unwrap_or_else(|| fallback_name.to_owned());
    let next_host = runtime.names.keys().next().cloned();
    let mut changed_snapshot = None;
    let mut upload_assignment = None;
    if let Some(snapshot) = runtime.snapshot.as_mut() {
        let mut changed = false;
        for piece in &mut snapshot.pieces {
            if piece.held_by.as_deref() == Some(client_id) {
                piece.held_by = None;
                changed = true;
            }
        }
        if snapshot.host_id == client_id {
            if let Some(next) = next_host {
                snapshot.host_id = next.clone();
                changed = true;
                if let Some(pending) = snapshot.queue.iter().find(|item| !item.available) {
                    let token = Uuid::new_v4().to_string();
                    *room.media_upload_token.lock().await = Some(token.clone());
                    upload_assignment = Some((next, token, pending.sha256.clone()));
                }
            }
        }
        if changed {
            snapshot.revision += 1;
            changed_snapshot = Some(snapshot.clone());
        }
    }
    let _ = room.events.send(ServerMessage::PeerLeft { name });
    if let Some(snapshot) = changed_snapshot {
        let _ = room.events.send(ServerMessage::Snapshot { snapshot });
    }
    if let Some((client_id, token, sha256)) = upload_assignment {
        let _ = room.events.send(ServerMessage::MediaUploadToken {
            client_id,
            token,
            sha256: Some(sha256),
        });
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn send(socket: &mut WebSocket, packet: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(packet).unwrap_or_else(|_| "{}".into());
    socket.send(Message::Text(text.into())).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn mismatched_app_protocols_are_refused_with_a_reason() {
        assert_eq!(protocol_mismatch(None), None);
        assert_eq!(protocol_mismatch(Some(PROTOCOL_VERSION)), None);
        let older_app = protocol_mismatch(Some(PROTOCOL_VERSION - 1)).unwrap();
        assert!(older_app.contains("server is newer"), "{older_app}");
        let newer_app = protocol_mismatch(Some(PROTOCOL_VERSION + 1)).unwrap();
        assert!(newer_app.contains("app is newer"), "{newer_app}");
    }

    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio_tungstenite::{WebSocketStream, connect_async, tungstenite::Message};

    #[tokio::test]
    async fn create_room_rehosts_imported_puzzle_progress() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-rehost-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let mut imported = RoomSnapshot::create(
            "old-room".into(),
            "OLD123".into(),
            "Imported puzzle".into(),
            "c".repeat(64),
            "mp4".into(),
            640,
            360,
            2,
            2,
            "old-host".into(),
        )
        .unwrap();
        imported.pieces[0].placed = true;
        imported.pieces[0].held_by = Some("old-player".into());
        imported.playback.position_ms = 750;
        imported.playback.playing = true;

        let request = ClientMessage::CreateRoom {
            name: "Host".into(),
            title: "Imported puzzle".into(),
            media_sha256: "c".repeat(64),
            media_extension: "mp4".into(),
            width: 640,
            height: 360,
            rows: 2,
            columns: 2,
            initial_snapshot: Some(Box::new(imported)),
            duration_ms: 0,
        };
        let wire_message = serde_json::to_string(&request).unwrap();
        let decoded_message: ClientMessage = serde_json::from_str(&wire_message).unwrap();
        let (_, invite_code, _, is_host) = connect_room(&state, "new-host", decoded_message)
            .await
            .unwrap();
        let room = state.rooms.read().await.get(&invite_code).cloned().unwrap();
        let snapshot = room.runtime.lock().await.snapshot.clone().unwrap();
        assert!(is_host);
        assert_ne!(snapshot.id, "old-room");
        assert_eq!(snapshot.host_id, "new-host");
        assert!(snapshot.pieces[0].placed);
        assert_eq!(snapshot.pieces[0].held_by, None);
        assert_eq!(snapshot.playback.position_ms, 750);
        assert!(!snapshot.playback.playing);
        assert!(!snapshot.media_available);
        fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn room_snapshot_survives_atomic_persistence_roundtrip() {
        let path = std::env::temp_dir().join(format!("pieceoff-test-{}", Uuid::new_v4()));
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "ABC123".into(),
            "Puzzle".into(),
            "hash".into(),
            "png".into(),
            800,
            600,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        persist_snapshot(&path, &snapshot).unwrap();
        let file = fs::read_dir(&path).unwrap().next().unwrap().unwrap().path();
        let restored: RoomSnapshot = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(snapshot, restored);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn oversized_persisted_room_is_not_loaded() {
        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-oversized-room-{}", Uuid::new_v4()));
        let mut snapshot = RoomSnapshot::create(
            "room-id".into(),
            "LARGE1".into(),
            "Oversized room".into(),
            format!("{:x}", Sha256::digest(b"large media")),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        snapshot.width = 8192;
        snapshot.height = 8192;
        persist_snapshot(&data_dir, &snapshot).unwrap();

        let state = AppState::new(data_dir.clone());
        load_rooms(&state).await;
        assert!(!state.rooms.read().await.contains_key("LARGE1"));
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn saved_v2_rooms_are_migrated_without_claiming_server_media() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-migration-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "OLD123".into(),
            "Older room".into(),
            "old-hash".into(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let mut value = serde_json::to_value(snapshot).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.insert("protocol_version".into(), serde_json::json!(2));
        fields.remove("media_extension");
        fields.remove("media_available");
        // A v2 file predates the queue model entirely.
        for field in ["queue", "current", "repeat", "video_lock", "shape_seed"] {
            fields.remove(field);
        }
        fs::write(
            data_dir.join("OLD123.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        let state = AppState::new(data_dir.clone());
        load_rooms(&state).await;
        let rooms = state.rooms.read().await;
        let restored = rooms.get("OLD123").unwrap().runtime.lock().await;
        let snapshot = restored.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.protocol_version, pieceoff_core::PROTOCOL_VERSION);
        assert_eq!(snapshot.media_extension, "bin");
        assert!(!snapshot.media_available);
        drop(restored);
        drop(rooms);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn room_recovery_detects_media_installed_before_snapshot_update() {
        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-media-recovery-{}", Uuid::new_v4()));
        let media = b"media persisted before the room-ready flag";
        let hash = format!("{:x}", Sha256::digest(media));
        let mut snapshot = RoomSnapshot::create(
            "room-id".into(),
            "REC123".into(),
            "Interrupted upload".into(),
            hash.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        // Model a crash after the file install but before the JSON snapshot
        // update that sets media_available.
        snapshot.media_available = false;
        persist_snapshot(&data_dir, &snapshot).unwrap();
        let path = media_path(&data_dir, &hash, "png");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, media).unwrap();

        let state = AppState::new(data_dir.clone());
        load_rooms(&state).await;
        let rooms = state.rooms.read().await;
        let recovered = rooms.get("REC123").unwrap().runtime.lock().await;
        assert!(recovered.snapshot.as_ref().unwrap().media_available);
        drop(recovered);
        drop(rooms);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn rejoining_host_gets_a_new_upload_token_after_restart() {
        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-incomplete-upload-{}", Uuid::new_v4()));
        let hash = format!("{:x}", Sha256::digest(b"upload interrupted before restart"));
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "REJOIN1".into(),
            "Interrupted upload".into(),
            hash.clone(),
            "mp4".into(),
            32,
            24,
            2,
            2,
            "departed-host".into(),
        )
        .unwrap();
        persist_snapshot(&data_dir, &snapshot).unwrap();

        // Loading a room after restart restores its snapshot but intentionally
        // cannot restore the previous process's in-memory upload secret.
        let state = AppState::new(data_dir.clone());
        load_rooms(&state).await;
        let client_id = "replacement-host";
        let (room, _, _, created) = connect_room(
            &state,
            client_id,
            ClientMessage::JoinRoom {
                invite_code: "REJOIN1".into(),
                name: "Replacement host".into(),
                media_sha256: hash,
                media_extension: "mp4".into(),
            },
        )
        .await
        .unwrap();

        assert!(!created);
        let snapshot = room.runtime.lock().await.snapshot.clone().unwrap();
        assert_eq!(snapshot.host_id, client_id);
        assert!(!snapshot.media_available);
        assert!(room.media_upload_token.lock().await.is_some());

        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn pending_media_upload_moves_to_the_new_room_host() {
        let mut snapshot = RoomSnapshot::create(
            "room-id".into(),
            "HANDOFF".into(),
            "Pending upload".into(),
            format!("{:x}", Sha256::digest(b"pending upload media")),
            "mp4".into(),
            32,
            24,
            2,
            2,
            "departing-host".into(),
        )
        .unwrap();
        snapshot.revision = 1;
        let room = Arc::new(RoomRecord::new(Some("departing-host-token".into())));
        {
            let mut runtime = room.runtime.lock().await;
            runtime.host_id = Some("departing-host".into());
            runtime.names.insert("departing-host".into(), "Host".into());
            runtime.names.insert("next-host".into(), "Guest".into());
            runtime.snapshot = Some(snapshot);
        }
        let mut events = room.events.subscribe();

        remove_peer(&room, "departing-host", "Host").await;

        let snapshot = room.runtime.lock().await.snapshot.clone().unwrap();
        assert_eq!(snapshot.host_id, "next-host");
        assert!(!snapshot.media_available);
        let (recipient, token) = loop {
            if let ServerMessage::MediaUploadToken {
                client_id, token, ..
            } = events.recv().await.unwrap()
            {
                break (client_id, token);
            }
        };
        assert_eq!(recipient, "next-host");
        assert_ne!(token, "departing-host-token");
        assert_eq!(
            room.media_upload_token.lock().await.as_deref(),
            Some(token.as_str())
        );
        let message = ServerMessage::MediaUploadToken {
            client_id: recipient,
            token,
            sha256: None,
        };
        assert!(message.visible_to("next-host"));
        assert!(!message.visible_to("another-player"));
    }

    #[tokio::test]
    async fn retention_expires_only_inactive_rooms_and_prunes_unreferenced_media() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-retention-{}", Uuid::new_v4()));
        let mut state = AppState::new(data_dir.clone());
        state.room_ttl = Duration::from_secs(60);
        for (code, label) in [
            ("OLD123", "expired clip"),
            ("LIVE12", "active clip"),
            ("NEW123", "recent clip"),
        ] {
            let media = label.as_bytes();
            let hash = format!("{:x}", Sha256::digest(media));
            let mut snapshot = RoomSnapshot::create(
                Uuid::new_v4().to_string(),
                code.into(),
                label.into(),
                hash.clone(),
                "png".into(),
                32,
                24,
                2,
                2,
                "host".into(),
            )
            .unwrap();
            snapshot.media_available = true;
            persist_snapshot(&data_dir, &snapshot).unwrap();
            let media_file = media_path(&data_dir, &hash, "png");
            fs::create_dir_all(media_file.parent().unwrap()).unwrap();
            fs::write(media_file, media).unwrap();
        }

        load_rooms(&state).await;
        let old = state.rooms.read().await.get("OLD123").unwrap().clone();
        let live = state.rooms.read().await.get("LIVE12").unwrap().clone();
        let stale = SystemTime::now() - Duration::from_secs(120);
        *old.last_activity.lock().await = stale;
        *live.last_activity.lock().await = stale;
        live.runtime
            .lock()
            .await
            .names
            .insert("connected-player".into(), "Player".into());

        assert_eq!(cleanup_expired_rooms(&state).await, 1);
        let rooms = state.rooms.read().await;
        assert!(!rooms.contains_key("OLD123"));
        assert!(rooms.contains_key("LIVE12"));
        assert!(rooms.contains_key("NEW123"));
        drop(rooms);

        let media_dir = data_dir.join("media");
        assert_eq!(fs::read_dir(&media_dir).unwrap().count(), 2);
        assert!(
            fs::read_dir(&data_dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| { !entry.file_name().to_string_lossy().starts_with("OLD123-") })
        );
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn any_player_can_rearrange_the_tray_and_everyone_is_told() {
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "ARR123".into(),
            "Arrange".into(),
            "a".repeat(64),
            "png".into(),
            64,
            48,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        let room = Arc::new(RoomRecord::new(None));
        room.runtime.lock().await.snapshot = Some(snapshot);
        let mut events = room.events.subscribe();

        let persist = dispatch(
            &room,
            "guest",
            ClientMessage::RearrangeTray {
                arrangement: pieceoff_core::TrayArrangement::EdgesFirst,
            },
        )
        .await
        .unwrap();
        assert!(persist, "the new arrangement must be saved");
        match events.recv().await.unwrap() {
            ServerMessage::PiecesChanged { pieces, revision } => {
                assert_eq!(pieces.len(), 12);
                assert!(revision > 0);
            }
            other => panic!("unexpected event {other:?}"),
        }

        // With every piece placed there is nothing to arrange; no event is sent.
        for piece in &mut room.runtime.lock().await.snapshot.as_mut().unwrap().pieces {
            piece.placed = true;
        }
        let refused = dispatch(
            &room,
            "guest",
            ClientMessage::RearrangeTray {
                arrangement: pieceoff_core::TrayArrangement::Shuffle,
            },
        )
        .await;
        assert!(refused.is_err());
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn saved_v3_through_v5_rooms_migrate_to_current_protocol() {
        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-v3-migration-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        for (code, version) in [("OLD123", 3), ("OLD124", 4), ("OLD125", 5)] {
            let mut snapshot = RoomSnapshot::create(
                format!("room-{version}"),
                code.into(),
                "Older room".into(),
                "old-hash".into(),
                "png".into(),
                32,
                24,
                2,
                2,
                "host".into(),
            )
            .unwrap();
            snapshot.protocol_version = version;
            fs::write(
                data_dir.join(format!("{code}.json")),
                serde_json::to_vec(&snapshot).unwrap(),
            )
            .unwrap();
        }

        let state = AppState::new(data_dir.clone());
        load_rooms(&state).await;
        let rooms = state.rooms.read().await;
        for code in ["OLD123", "OLD124", "OLD125"] {
            let restored = rooms.get(code).unwrap().runtime.lock().await;
            assert_eq!(
                restored.snapshot.as_ref().unwrap().protocol_version,
                pieceoff_core::PROTOCOL_VERSION
            );
        }
        drop(rooms);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn uploaded_room_media_is_hash_checked_stored_and_invite_gated() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-media-test-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let content = b"portable room media bytes";
        let hash = format!("{:x}", Sha256::digest(content));
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "ABC123".into(),
            "Portable test".into(),
            hash.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let room = Arc::new(RoomRecord::new(Some("upload-secret".into())));
        {
            let mut runtime = room.runtime.lock().await;
            runtime.host_id = Some("host".into());
            runtime.names.insert("host".into(), "Host".into());
            runtime.snapshot = Some(snapshot);
        }
        state.rooms.write().await.insert("ABC123".into(), room);

        let guest = ClientMessage::JoinRoom {
            invite_code: "ABC123".into(),
            name: "Guest".into(),
            media_sha256: String::new(),
            media_extension: String::new(),
        };
        assert!(
            connect_room(&state, "early-guest", guest.clone())
                .await
                .is_err()
        );

        let response = upload_media(
            State(state.clone()),
            AxumPath(hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("upload-secret".into()),
                probe: Some(1),
            }),
            Body::empty(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "a probe for missing media asks for the upload"
        );
        assert!(!media_path(&data_dir, &hash, "png").exists());

        let response = upload_media(
            State(state.clone()),
            AxumPath(hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("upload-secret".into()),
                probe: None,
            }),
            Body::from(content.to_vec()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            fs::read(media_path(&data_dir, &hash, "png")).unwrap(),
            content
        );
        let probe = upload_media(
            State(state.clone()),
            AxumPath(hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("upload-secret".into()),
                probe: Some(1),
            }),
            Body::empty(),
        )
        .await;
        assert_eq!(
            probe.status(),
            StatusCode::CREATED,
            "held media needs no upload"
        );
        assert!(connect_room(&state, "guest", guest).await.is_ok());

        let response = download_media(
            State(state.clone()),
            AxumPath(hash.clone()),
            Query(MediaQuery {
                code: Some("ABC123".into()),
                token: None,
                probe: None,
            }),
            axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let downloaded = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(downloaded.as_ref(), content);

        let denied = download_media(
            State(state),
            AxumPath(hash),
            Query(MediaQuery {
                code: Some("WRONG1".into()),
                token: None,
                probe: None,
            }),
            axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn media_upload_rejects_wrong_digest_and_bad_upload_tokens() {
        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-media-reject-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let expected = format!("{:x}", Sha256::digest(b"expected"));
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "ABC123".into(),
            "Portable test".into(),
            expected.clone(),
            "mp4".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let room = Arc::new(RoomRecord::new(Some("upload-secret".into())));
        room.runtime.lock().await.snapshot = Some(snapshot);
        state.rooms.write().await.insert("ABC123".into(), room);

        let rejected = upload_media(
            State(state.clone()),
            AxumPath(expected.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("wrong-token".into()),
                probe: None,
            }),
            Body::from(b"expected".to_vec()),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

        let rejected = upload_media(
            State(state.clone()),
            AxumPath(expected.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("upload-secret".into()),
                probe: None,
            }),
            Body::from(b"different content".to_vec()),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!media_path(&data_dir, &expected, "mp4").exists());
        fs::remove_dir_all(data_dir).unwrap_or_default();
    }

    #[tokio::test]
    async fn concurrent_room_uploads_share_the_configured_media_quota() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-quota-{}", Uuid::new_v4()));
        let mut state = AppState::new(data_dir.clone());
        state.media_quota_bytes = 10;
        let first_content = b"12345678";
        let second_content = b"abcd";
        let first_hash = format!("{:x}", Sha256::digest(first_content));
        let second_hash = format!("{:x}", Sha256::digest(second_content));
        for (code, token, hash) in [
            ("FIRST1", "first-token", first_hash.clone()),
            ("SECOND", "second-token", second_hash.clone()),
        ] {
            let snapshot = RoomSnapshot::create(
                Uuid::new_v4().to_string(),
                code.into(),
                "Quota test".into(),
                hash,
                "png".into(),
                32,
                24,
                2,
                2,
                "host".into(),
            )
            .unwrap();
            let room = Arc::new(RoomRecord::new(Some(token.into())));
            room.runtime.lock().await.snapshot = Some(snapshot);
            state.rooms.write().await.insert(code.into(), room);
        }

        let first = upload_media(
            State(state.clone()),
            AxumPath(first_hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("first-token".into()),
                probe: None,
            }),
            Body::from(first_content.to_vec()),
        );
        let second = upload_media(
            State(state.clone()),
            AxumPath(second_hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("second-token".into()),
                probe: None,
            }),
            Body::from(second_content.to_vec()),
        );
        let (first, second) = tokio::join!(first, second);
        let statuses = [first.status(), second.status()];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::INSUFFICIENT_STORAGE)
                .count(),
            1
        );
        let used = media_storage_used_bytes(&data_dir.join("media"))
            .await
            .unwrap();
        assert!(
            [4, 8].contains(&used),
            "stored media must remain within the quota"
        );
        let entries = fs::read_dir(data_dir.join("media")).unwrap().count();
        assert_eq!(
            entries, 1,
            "quota rejection should delete its partial upload"
        );
        fs::remove_dir_all(data_dir).unwrap();
    }

    async fn read_message<S>(socket: &mut WebSocketStream<S>, wanted: &str) -> serde_json::Value
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let message = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
                .await
                .unwrap()
                .expect("socket remains open")
                .expect("websocket read succeeds");
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["type"] == wanted {
                    return value;
                }
            }
        }
    }

    async fn put_media(
        address: std::net::SocketAddr,
        hash: &str,
        token: &str,
        bytes: &[u8],
    ) -> String {
        let mut upload = tokio::net::TcpStream::connect(address).await.unwrap();
        upload
            .write_all(
                format!(
                    "PUT /media/{hash}?token={token} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        upload.write_all(bytes).await.unwrap();
        let mut response = Vec::new();
        upload.read_to_end(&mut response).await.unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    type TestSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    /// Start a server on a free port; returns its address and state.
    #[tokio::test]
    async fn an_app_on_another_protocol_is_told_to_update() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-proto-{}", Uuid::new_v4()));
        let (address, _state) = queue_test_server(&data_dir).await;
        let (mut old_app, _) = connect_async(format!(
            "ws://{address}/ws?protocol={}",
            PROTOCOL_VERSION - 1
        ))
        .await
        .unwrap();
        let error = read_message(&mut old_app, "error").await;
        assert!(
            error["message"].as_str().unwrap().contains("Update"),
            "{error}"
        );
        // The matching version gets through to the room handshake.
        let (mut app, _) = connect_async(format!("ws://{address}/ws?protocol={PROTOCOL_VERSION}"))
            .await
            .unwrap();
        app.send(Message::Text(
            serde_json::json!({
                "type":"join_room", "invite_code":"NOPE00", "name":"A",
                "media_sha256":"", "media_extension":""
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let reply = read_message(&mut app, "error").await;
        assert_eq!(reply["message"], ROOM_NOT_FOUND);
        let _ = fs::remove_dir_all(data_dir);
    }

    async fn queue_test_server(data_dir: &std::path::Path) -> (std::net::SocketAddr, AppState) {
        let state = AppState::new(data_dir.to_path_buf());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        tokio::spawn(async move {
            axum::serve(listener, application(server_state))
                .await
                .unwrap();
        });
        (address, state)
    }

    /// Create a room whose first media is `media`, upload it, and return the
    /// host socket, the invite code, and the media hash.
    async fn queue_test_room(
        address: std::net::SocketAddr,
        media: &[u8],
    ) -> (TestSocket, String, String) {
        let hash = format!("{:x}", Sha256::digest(media));
        let (mut host, _) = connect_async(format!("ws://{address}/ws")).await.unwrap();
        host.send(Message::Text(
            serde_json::json!({
                "type":"create_room", "name":"Host", "title":"Show",
                "media_sha256":hash, "media_extension":"png",
                "width":32, "height":24, "rows":2, "columns":2, "duration_ms": 9000
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let welcome = read_message(&mut host, "welcome").await;
        let code = welcome["snapshot"]["invite_code"]
            .as_str()
            .unwrap()
            .to_owned();
        let token = welcome["media_upload_token"].as_str().unwrap().to_owned();
        let response = put_media(address, &hash, &token, media).await;
        assert!(response.starts_with("HTTP/1.1 201"), "{response}");
        read_message(&mut host, "snapshot").await;
        (host, code, hash)
    }

    fn queue_add_message(hash: &str, title: &str) -> Message {
        Message::Text(
            serde_json::json!({
                "type":"queue_add", "sha256":hash, "extension":"png", "title":title,
                "width":32, "height":24, "duration_ms": 5000
            })
            .to_string()
            .into(),
        )
    }

    #[tokio::test]
    async fn queued_item_uploads_downloads_and_survives_restart() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-queue-up-{}", Uuid::new_v4()));
        let (address, _state) = queue_test_server(&data_dir).await;
        let (mut host, code, hash_a) = queue_test_room(address, b"clip A").await;
        let media_b = b"clip B bytes";
        let hash_b = format!("{:x}", Sha256::digest(media_b));
        host.send(queue_add_message(&hash_b, "B")).await.unwrap();
        let token = read_message(&mut host, "media_upload_token").await;
        assert_eq!(token["sha256"], hash_b);
        let response = put_media(address, &hash_b, token["token"].as_str().unwrap(), media_b).await;
        assert!(response.starts_with("HTTP/1.1 201"), "{response}");
        let ready = loop {
            let snapshot = read_message(&mut host, "snapshot").await;
            if snapshot["snapshot"]["queue"][1]["available"] == true {
                break snapshot;
            }
        };
        assert_eq!(ready["snapshot"]["queue"][0]["available"], true);

        let mut download = tokio::net::TcpStream::connect(address).await.unwrap();
        download
            .write_all(
                format!(
                    "GET /media/{hash_b}?code={code} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut body = Vec::new();
        download.read_to_end(&mut body).await.unwrap();
        assert!(String::from_utf8_lossy(&body).starts_with("HTTP/1.1 200"));
        assert!(body.ends_with(media_b));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let restored_state = AppState::new(data_dir.clone());
        load_rooms(&restored_state).await;
        let rooms = restored_state.rooms.read().await;
        let runtime = rooms.get(&code).unwrap().runtime.lock().await;
        let snapshot = runtime.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.queue.len(), 2);
        assert!(snapshot.queue.iter().all(|item| item.available));
        assert_eq!(snapshot.queue[0].sha256, hash_a);
        drop(runtime);
        drop(rooms);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn adding_media_already_in_the_store_is_available_immediately() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-queue-dup-{}", Uuid::new_v4()));
        let (address, _state) = queue_test_server(&data_dir).await;
        let (_first_host, _code, shared_hash) = queue_test_room(address, b"shared clip").await;
        let (mut host, _code, _hash) = queue_test_room(address, b"other clip").await;
        host.send(queue_add_message(&shared_hash, "Shared"))
            .await
            .unwrap();
        loop {
            let snapshot = read_message(&mut host, "snapshot").await;
            if snapshot["snapshot"]["queue"].as_array().unwrap().len() == 2
                && snapshot["snapshot"]["queue"][1]["available"] == true
            {
                break;
            }
        }
        // No upload is requested for media the server already holds.
        let extra = tokio::time::timeout(std::time::Duration::from_millis(400), async {
            read_message(&mut host, "media_upload_token").await
        })
        .await;
        assert!(extra.is_err(), "unexpected upload token: {extra:?}");
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn host_handoff_assigns_upload_token_while_an_item_is_unavailable() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-queue-hand-{}", Uuid::new_v4()));
        let (address, _state) = queue_test_server(&data_dir).await;
        let (mut host, code, hash_a) = queue_test_room(address, b"handoff clip").await;
        let (mut guest, _) = connect_async(format!("ws://{address}/ws")).await.unwrap();
        guest
            .send(Message::Text(
                serde_json::json!({
                    "type":"join_room", "invite_code":code, "name":"Guest",
                    "media_sha256":hash_a, "media_extension":""
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        read_message(&mut guest, "welcome").await;
        let hash_b = format!("{:x}", Sha256::digest(b"never uploaded"));
        host.send(queue_add_message(&hash_b, "B")).await.unwrap();
        read_message(&mut host, "media_upload_token").await;
        host.close(None).await.unwrap();
        drop(host);
        let token = read_message(&mut guest, "media_upload_token").await;
        assert_eq!(token["sha256"], hash_b);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn queue_add_rejects_path_like_fingerprints_and_bad_dimensions() {
        let mut room = RoomRuntime::default();
        room.snapshot = Some(
            RoomSnapshot::create(
                "r".into(),
                "ABCD".into(),
                "A".into(),
                "a1".repeat(32),
                "png".into(),
                32,
                24,
                2,
                2,
                "host".into(),
            )
            .unwrap(),
        );
        let item = |sha: &str, width: u32| pieceoff_core::MediaItem {
            sha256: sha.into(),
            extension: "png".into(),
            title: "x".into(),
            width,
            height: 24,
            duration_ms: 0,
            available: false,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        };
        let traversal = format!("../{}", "b".repeat(61));
        assert!(room.queue_add("host", item(&traversal, 32)).is_err());
        assert!(room.queue_add("host", item(&"b2".repeat(32), 0)).is_err());
        assert!(room.queue_add("host", item(&"b2".repeat(32), 32)).is_ok());
    }

    #[tokio::test]
    async fn retention_keeps_media_of_queued_items_that_are_not_current() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-keepq-{}", Uuid::new_v4()));
        let mut state = AppState::new(data_dir.clone());
        state.room_ttl = Duration::from_secs(60);
        let hash_a = format!("{:x}", Sha256::digest(b"keep A"));
        let hash_b = format!("{:x}", Sha256::digest(b"keep B"));
        let mut snapshot = RoomSnapshot::create(
            Uuid::new_v4().to_string(),
            "KEEPQ1".into(),
            "Show".into(),
            hash_a.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        snapshot.queue.push(pieceoff_core::MediaItem {
            sha256: hash_b.clone(),
            extension: "png".into(),
            title: "B".into(),
            width: 32,
            height: 24,
            duration_ms: 0,
            available: true,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        });
        persist_snapshot(&data_dir, &snapshot).unwrap();
        let media_dir = data_dir.join("media");
        fs::create_dir_all(&media_dir).unwrap();
        fs::write(media_path(&data_dir, &hash_a, "png"), b"keep A").unwrap();
        fs::write(media_path(&data_dir, &hash_b, "png"), b"keep B").unwrap();
        // An expired room makes the cleanup pass collect unreferenced files.
        let gone_hash = format!("{:x}", Sha256::digest(b"gone"));
        let gone = RoomSnapshot::create(
            Uuid::new_v4().to_string(),
            "GONEQ1".into(),
            "Gone".into(),
            gone_hash.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        persist_snapshot(&data_dir, &gone).unwrap();
        fs::write(media_path(&data_dir, &gone_hash, "png"), b"gone").unwrap();
        load_rooms(&state).await;
        let stale = SystemTime::now() - Duration::from_secs(120);
        *state
            .rooms
            .read()
            .await
            .get("GONEQ1")
            .unwrap()
            .last_activity
            .lock()
            .await = stale;
        assert_eq!(cleanup_expired_rooms(&state).await, 1);
        assert!(media_path(&data_dir, &hash_b, "png").is_file());
        assert!(media_path(&data_dir, &hash_a, "png").is_file());
        assert!(!media_path(&data_dir, &gone_hash, "png").is_file());
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn a_stalled_upload_does_not_block_other_rooms() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-stall-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let slow_content = b"a slow, large upload";
        let quick_content = b"a quick one";
        let slow_hash = format!("{:x}", Sha256::digest(slow_content));
        let quick_hash = format!("{:x}", Sha256::digest(quick_content));
        for (code, token, hash) in [
            ("SLOW01", "slow-token", slow_hash.clone()),
            ("QUICK1", "quick-token", quick_hash.clone()),
        ] {
            let snapshot = RoomSnapshot::create(
                Uuid::new_v4().to_string(),
                code.into(),
                "Stall".into(),
                hash,
                "png".into(),
                32,
                24,
                2,
                2,
                "host".into(),
            )
            .unwrap();
            let room = Arc::new(RoomRecord::new(Some(token.into())));
            room.runtime.lock().await.snapshot = Some(snapshot);
            state.rooms.write().await.insert(code.into(), room);
        }
        // The slow upload sends its first bytes and then waits for more.
        let (feed, receiver) = tokio::sync::mpsc::channel::<Bytes>(4);
        feed.send(Bytes::from_static(&slow_content[..4]))
            .await
            .unwrap();
        let slow_body = Body::from_stream(futures_util::stream::unfold(
            receiver,
            |mut receiver| async move {
                receiver
                    .recv()
                    .await
                    .map(|chunk| (Ok::<_, std::io::Error>(chunk), receiver))
            },
        ));
        let slow = tokio::spawn(upload_media(
            State(state.clone()),
            AxumPath(slow_hash.clone()),
            Query(MediaQuery {
                code: None,
                token: Some("slow-token".into()),
                probe: None,
            }),
            slow_body,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let quick = tokio::time::timeout(
            Duration::from_secs(3),
            upload_media(
                State(state.clone()),
                AxumPath(quick_hash.clone()),
                Query(MediaQuery {
                    code: None,
                    token: Some("quick-token".into()),
                    probe: None,
                }),
                Body::from(quick_content.to_vec()),
            ),
        )
        .await
        .expect("a quick upload must not wait behind a stalled one");
        assert_eq!(quick.status(), StatusCode::CREATED);

        feed.send(Bytes::from_static(&slow_content[4..]))
            .await
            .unwrap();
        drop(feed);
        let slow = slow.await.unwrap();
        assert_eq!(slow.status(), StatusCode::CREATED);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn an_upload_declared_larger_than_the_media_cap_is_refused_up_front() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-cap-{}", Uuid::new_v4()));
        let mut state = AppState::new(data_dir.clone());
        state.max_media_bytes = 8;
        let content = vec![7u8; 100];
        let hash = format!("{:x}", Sha256::digest(&content));
        let snapshot = RoomSnapshot::create(
            Uuid::new_v4().to_string(),
            "CAP001".into(),
            "Cap".into(),
            hash.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let room = Arc::new(RoomRecord::new(Some("cap-token".into())));
        room.runtime.lock().await.snapshot = Some(snapshot);
        state.rooms.write().await.insert("CAP001".into(), room);
        let response = upload_media(
            State(state.clone()),
            AxumPath(hash),
            Query(MediaQuery {
                code: None,
                token: Some("cap-token".into()),
                probe: None,
            }),
            Body::from(content),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let leftovers = fs::read_dir(data_dir.join("media"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "nothing was written");
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn downloads_honour_byte_ranges_so_large_files_can_resume() {
        use axum::http::{HeaderMap, HeaderValue};
        let data_dir = std::env::temp_dir().join(format!("pieceoff-range-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let content = b"0123456789abcdefghij";
        let hash = format!("{:x}", Sha256::digest(content));
        let mut snapshot = RoomSnapshot::create(
            Uuid::new_v4().to_string(),
            "RANGE1".into(),
            "Range".into(),
            hash.clone(),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        snapshot.queue[0].available = true;
        snapshot.sync_current_mirror();
        let room = Arc::new(RoomRecord::new(None));
        room.runtime.lock().await.snapshot = Some(snapshot);
        state.rooms.write().await.insert("RANGE1".into(), room);
        let file = media_path(&data_dir, &hash, "png");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
        let get = |range: Option<&'static str>| {
            let state = state.clone();
            let hash = hash.clone();
            async move {
                let mut headers = HeaderMap::new();
                if let Some(range) = range {
                    headers.insert(header::RANGE, HeaderValue::from_static(range));
                }
                download_media(
                    State(state),
                    AxumPath(hash),
                    Query(MediaQuery {
                        code: Some("RANGE1".into()),
                        token: None,
                        probe: None,
                    }),
                    headers,
                )
                .await
            }
        };
        let whole = get(None).await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(whole.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");

        let tail = get(Some("bytes=10-")).await;
        assert_eq!(tail.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            tail.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 10-19/20"
        );
        let bytes = axum::body::to_bytes(tail.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), &content[10..]);

        let middle = get(Some("bytes=2-4")).await;
        assert_eq!(middle.status(), StatusCode::PARTIAL_CONTENT);
        let bytes = axum::body::to_bytes(middle.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), &content[2..5]);

        let beyond = get(Some("bytes=99-")).await;
        assert_eq!(beyond.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        // Anything we do not understand falls back to the whole file.
        let odd = get(Some("items=1-2")).await;
        assert_eq!(odd.status(), StatusCode::OK);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn the_server_advances_a_room_when_its_video_ends() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-tick-test-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let mut snapshot = RoomSnapshot::create(
            "room-id".into(),
            "TICK01".into(),
            "Show".into(),
            "a1".repeat(32),
            "png".into(),
            32,
            24,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        snapshot.queue[0].available = true;
        snapshot.queue[0].duration_ms = 100;
        snapshot.queue.push(pieceoff_core::MediaItem {
            sha256: "b2".repeat(32),
            extension: "png".into(),
            title: "B".into(),
            width: 32,
            height: 24,
            duration_ms: 60_000,
            available: true,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        });
        snapshot.repeat = pieceoff_core::RepeatMode::All;
        snapshot.playback.playing = true;
        snapshot.playback.changed_at_ms = now_ms().saturating_sub(1_000);
        snapshot.sync_current_mirror();
        let room = Arc::new(RoomRecord::new(None));
        room.runtime.lock().await.snapshot = Some(snapshot);
        state
            .rooms
            .write()
            .await
            .insert("TICK01".into(), room.clone());
        let _router = application(state.clone());
        let mut current = 0;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            current = room.runtime.lock().await.snapshot.as_ref().unwrap().current;
            if current == 1 {
                break;
            }
        }
        assert_eq!(current, 1);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn host_queues_swaps_and_locks_over_the_wire() {
        let media_a = b"queue test clip A";
        let hash_a = format!("{:x}", Sha256::digest(media_a));
        let hash_b = format!("{:x}", Sha256::digest(b"queue test clip B"));
        let data_dir = std::env::temp_dir().join(format!("pieceoff-queue-test-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        tokio::spawn(async move {
            axum::serve(listener, application(server_state))
                .await
                .unwrap();
        });
        let url = format!("ws://{address}/ws");
        let (mut host, _) = connect_async(&url).await.unwrap();
        host.send(Message::Text(
            serde_json::json!({
                "type":"create_room", "name":"Host", "title":"Show",
                "media_sha256":hash_a, "media_extension":"png",
                "width":32, "height":24, "rows":2, "columns":2, "duration_ms": 9000
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let welcome = read_message(&mut host, "welcome").await;
        assert_eq!(welcome["snapshot"]["queue"].as_array().unwrap().len(), 1);
        assert_eq!(welcome["snapshot"]["queue"][0]["duration_ms"], 9000);
        let code = welcome["snapshot"]["invite_code"]
            .as_str()
            .unwrap()
            .to_owned();
        let token = welcome["media_upload_token"].as_str().unwrap().to_owned();
        let response = put_media(address, &hash_a, &token, media_a).await;
        assert!(response.starts_with("HTTP/1.1 201"), "{response}");
        read_message(&mut host, "snapshot").await;

        let (mut guest, _) = connect_async(&url).await.unwrap();
        guest
            .send(Message::Text(
                serde_json::json!({
                    "type":"join_room", "invite_code":code, "name":"Guest",
                    "media_sha256":hash_a, "media_extension":""
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        read_message(&mut guest, "welcome").await;
        read_message(&mut host, "peer_joined").await;

        let send = |value: serde_json::Value| Message::Text(value.to_string().into());
        host.send(send(serde_json::json!({
            "type":"queue_add", "sha256":hash_b, "extension":"png", "title":"B",
            "width":32, "height":24, "duration_ms": 5000
        })))
        .await
        .unwrap();
        let added = read_message(&mut host, "snapshot").await;
        assert_eq!(added["snapshot"]["queue"].as_array().unwrap().len(), 2);
        assert_eq!(added["snapshot"]["current"], 0);
        let pieces = added["snapshot"]["pieces"].clone();

        guest
            .send(send(serde_json::json!({"type":"select_item", "index":1})))
            .await
            .unwrap();
        let denied = read_message(&mut guest, "error").await;
        assert!(denied["message"].as_str().unwrap().contains("host"));

        host.send(send(serde_json::json!({"type":"select_item", "index":1})))
            .await
            .unwrap();
        let swapped = read_message(&mut host, "snapshot").await;
        assert_eq!(swapped["snapshot"]["current"], 1);
        assert_eq!(swapped["snapshot"]["media_sha256"], hash_b);
        assert_eq!(swapped["snapshot"]["pieces"], pieces);

        host.send(send(
            serde_json::json!({"type":"set_video_lock", "locked":true}),
        ))
        .await
        .unwrap();
        let locked = read_message(&mut host, "snapshot").await;
        assert_eq!(locked["snapshot"]["video_lock"], true);
        host.send(send(serde_json::json!({"type":"select_item", "index":0})))
            .await
            .unwrap();
        let refused = read_message(&mut host, "error").await;
        assert!(refused["message"].as_str().unwrap().contains("locked"));
        let _ = fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn two_clients_share_video_playback_and_saved_puzzle_progress() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("Skipping video room integration test: ffmpeg is not installed.");
            return;
        }
        let video_path =
            std::env::temp_dir().join(format!("pieceoff-room-video-{}.mp4", Uuid::new_v4()));
        let generated = std::process::Command::new("ffmpeg")
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
            .arg(&video_path)
            .status()
            .expect("ffmpeg should generate an integration-test video");
        assert!(
            generated.success(),
            "ffmpeg failed to generate an integration-test video"
        );
        let media = fs::read(&video_path).unwrap();
        fs::remove_file(video_path).unwrap();
        let media_hash = format!("{:x}", Sha256::digest(&media));

        let data_dir =
            std::env::temp_dir().join(format!("pieceoff-playback-test-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, application(server_state))
                .await
                .unwrap();
        });
        let url = format!("ws://{address}/ws");
        let (mut host, _) = connect_async(&url).await.unwrap();
        host.send(Message::Text(
            serde_json::json!({
                "type":"create_room", "name":"Host", "title":"Looping clip",
                "media_sha256":media_hash,
                "media_extension":"mp4", "width":32, "height":24,
                "rows":3, "columns":4
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let welcome = read_message(&mut host, "welcome").await;
        let code = welcome["snapshot"]["invite_code"].as_str().unwrap();
        let media_hash = welcome["snapshot"]["media_sha256"].as_str().unwrap();
        let upload_token = welcome["media_upload_token"].as_str().unwrap();

        let mut upload = tokio::net::TcpStream::connect(address).await.unwrap();
        upload
            .write_all(
                format!(
                    "PUT /media/{media_hash}?token={upload_token} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    media.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        upload.write_all(&media).await.unwrap();
        let mut upload_response = Vec::new();
        upload.read_to_end(&mut upload_response).await.unwrap();
        assert!(
            String::from_utf8_lossy(&upload_response).starts_with("HTTP/1.1 201"),
            "unexpected video upload response: {}",
            String::from_utf8_lossy(&upload_response)
        );
        let ready = read_message(&mut host, "snapshot").await;
        assert_eq!(ready["snapshot"]["media_available"], true);

        let (mut guest, _) = connect_async(&url).await.unwrap();
        guest
            .send(Message::Text(
                serde_json::json!({
                    "type":"join_room", "invite_code":code, "name":"Guest",
                    "media_sha256":media_hash, "media_extension":""
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        read_message(&mut guest, "welcome").await;
        read_message(&mut host, "peer_joined").await;

        let mut download = tokio::net::TcpStream::connect(address).await.unwrap();
        download
            .write_all(
                format!(
                    "GET /media/{media_hash}?code={code} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut download_response = Vec::new();
        download.read_to_end(&mut download_response).await.unwrap();
        let body_start = download_response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert!(
            String::from_utf8_lossy(&download_response[..body_start]).starts_with("HTTP/1.1 200")
        );
        assert_eq!(&download_response[body_start..], media);

        let selected = welcome["snapshot"]["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|piece| piece["id"] == 2)
            .unwrap();
        let selected_x = selected["x"].as_f64().unwrap() as f32;
        let selected_y = selected["y"].as_f64().unwrap() as f32;
        let snapshot = &welcome["snapshot"];
        let width = snapshot["width"].as_u64().unwrap() as u32;
        let height = snapshot["height"].as_u64().unwrap() as u32;
        let columns = snapshot["columns"].as_u64().unwrap() as u16;
        let rows = snapshot["rows"].as_u64().unwrap() as u16;
        let tray = pieceoff_core::piece_tray_layout(width, height, rows, columns);
        let cell_w = width as f32 / columns as f32;
        let cell_h = height as f32 / rows as f32;
        let final_x = if selected_x + cell_w + 5.0 <= tray.content_width {
            selected_x + 5.0
        } else {
            selected_x - 5.0
        };
        let final_y = if selected_y + cell_h + 7.0 <= tray.content_height - tray.board_y {
            selected_y + 7.0
        } else {
            selected_y - 7.0
        };
        host.send(Message::Text(
            r#"{"type":"drag_start","piece_id":2}"#.into(),
        ))
        .await
        .unwrap();
        read_message(&mut host, "pieces_changed").await;
        read_message(&mut guest, "pieces_changed").await;
        host.send(Message::Text(
            serde_json::json!({
                "type":"move_piece", "piece_id":2, "x":final_x, "y":final_y
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let host_move = read_message(&mut host, "pieces_changed").await;
        let guest_move = read_message(&mut guest, "pieces_changed").await;
        for update in [host_move, guest_move] {
            let piece = update["pieces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|piece| piece["id"] == 2)
                .unwrap();
            assert!((piece["x"].as_f64().unwrap() as f32 - final_x).abs() < 0.01);
            assert!((piece["y"].as_f64().unwrap() as f32 - final_y).abs() < 0.01);
        }
        host.send(Message::Text(
            serde_json::json!({
                "type":"drag_end", "piece_id":2, "x":final_x, "y":final_y
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        read_message(&mut host, "pieces_changed").await;
        read_message(&mut guest, "pieces_changed").await;

        guest
            .send(Message::Text(
                serde_json::json!({
                    "type":"set_playback", "playing":true, "position_ms":1500,
                    "rate":1.5, "loop_start_ms":1000, "loop_end_ms":5000
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        assert!(
            read_message(&mut guest, "error").await["message"]
                .as_str()
                .unwrap()
                .contains("Only the room host")
        );

        for (playing, position_ms, rate, loop_start_ms, loop_end_ms) in
            [(true, 1500, 1.5, 1000, 5000), (false, 2875, 1.0, 900, 4200)]
        {
            host.send(Message::Text(
                serde_json::json!({
                    "type":"set_playback", "playing":playing, "position_ms":position_ms,
                    "rate":rate, "loop_start_ms":loop_start_ms, "loop_end_ms":loop_end_ms
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let host_update = read_message(&mut host, "playback_changed").await;
            let guest_update = read_message(&mut guest, "playback_changed").await;
            for update in [host_update, guest_update] {
                assert_eq!(update["playback"]["playing"], playing);
                assert_eq!(update["playback"]["position_ms"], position_ms);
                assert_eq!(update["playback"]["rate"], rate);
                assert_eq!(update["playback"]["loop_start_ms"], loop_start_ms);
                assert_eq!(update["playback"]["loop_end_ms"], loop_end_ms);
            }
        }

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let saved = fs::read_dir(&data_dir)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .and_then(|path| fs::read(path).ok())
                    .and_then(|bytes| serde_json::from_slice::<RoomSnapshot>(&bytes).ok());
                if saved.is_some_and(|snapshot| {
                    !snapshot.playback.playing
                        && snapshot.playback.position_ms == 2875
                        && snapshot
                            .piece(2)
                            .is_some_and(|piece| (piece.x - final_x).abs() < 0.01)
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("paused playback state should be durably saved");

        let restored_state = AppState::new(data_dir.clone());
        load_rooms(&restored_state).await;
        let restored_rooms = restored_state.rooms.read().await;
        let playback = restored_rooms
            .get(code)
            .unwrap()
            .runtime
            .lock()
            .await
            .snapshot
            .as_ref()
            .unwrap()
            .playback
            .clone();
        assert!(!playback.playing);
        assert_eq!(playback.position_ms, 2875);
        assert_eq!(playback.loop_start_ms, 900);
        assert_eq!(playback.loop_end_ms, 4200);
        assert_eq!(playback.rate, 1.0);
        let piece = restored_rooms
            .get(code)
            .unwrap()
            .runtime
            .lock()
            .await
            .snapshot
            .as_ref()
            .unwrap()
            .piece(2)
            .unwrap()
            .clone();
        assert!((piece.x - final_x).abs() < 0.01);
        assert!((piece.y - final_y).abs() < 0.01);
        drop(restored_rooms);
        guest.close(None).await.unwrap();
        host.close(None).await.unwrap();
        server.abort();
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn two_clients_share_piece_moves_with_exclusive_drag_and_saved_progress() {
        let data_dir = std::env::temp_dir().join(format!("pieceoff-room-test-{}", Uuid::new_v4()));
        let state = AppState::new(data_dir.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, application(server_state))
                .await
                .unwrap();
        });
        let url = format!("ws://{address}/ws");
        let (mut host, _) = connect_async(&url).await.unwrap();
        host.send(Message::Text(
            serde_json::json!({
                "type":"create_room", "name":"Host", "title":"Room test",
                "media_sha256":format!("{:x}", Sha256::digest(b"looping test clip")),
                "media_extension":"png",
                "width":800, "height":600, "rows":3, "columns":4
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let welcome = read_message(&mut host, "welcome").await;
        let host_pieces = welcome["snapshot"]["pieces"].as_array().unwrap().clone();
        assert!(host_pieces.iter().all(|piece| {
            !piece["placed"].as_bool().unwrap() && piece["x"].as_f64().unwrap() >= 800.0
        }));
        let code = welcome["snapshot"]["invite_code"]
            .as_str()
            .unwrap()
            .to_owned();
        let media_hash = welcome["snapshot"]["media_sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let upload_token = welcome["media_upload_token"].as_str().unwrap();
        let media = b"looping test clip";

        let mut upload = tokio::net::TcpStream::connect(address).await.unwrap();
        upload
            .write_all(
                format!(
                    "PUT /media/{media_hash}?token={upload_token} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    media.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        upload.write_all(media).await.unwrap();
        let mut upload_response = Vec::new();
        upload.read_to_end(&mut upload_response).await.unwrap();
        assert!(
            String::from_utf8_lossy(&upload_response).starts_with("HTTP/1.1 201"),
            "unexpected upload response: {}",
            String::from_utf8_lossy(&upload_response)
        );
        let stored = read_message(&mut host, "snapshot").await;
        assert_eq!(stored["snapshot"]["media_available"], true);

        let (mut guest, _) = connect_async(&url).await.unwrap();
        guest
            .send(Message::Text(
                serde_json::json!({
                    "type":"join_room", "invite_code":code, "name":"Guest",
                    "media_sha256":"", "media_extension":""
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let guest_welcome = read_message(&mut guest, "welcome").await;
        assert_eq!(
            guest_welcome["snapshot"]["pieces"].as_array().unwrap(),
            &host_pieces
        );
        read_message(&mut host, "peer_joined").await;

        let mut download = tokio::net::TcpStream::connect(address).await.unwrap();
        download
            .write_all(
                format!(
                    "GET /media/{media_hash}?code={code} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut download_response = Vec::new();
        download.read_to_end(&mut download_response).await.unwrap();
        let body_start = download_response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert!(
            String::from_utf8_lossy(&download_response[..body_start]).starts_with("HTTP/1.1 200")
        );
        assert_eq!(&download_response[body_start..], media);

        let selected_anchor = welcome["snapshot"]["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|piece| piece["id"] == 2)
            .unwrap();
        let selected_x = selected_anchor["x"].as_f64().unwrap() as f32;
        let selected_y = selected_anchor["y"].as_f64().unwrap() as f32;
        host.send(Message::Text(
            r#"{"type":"drag_start_selection","piece_ids":[2,10]}"#.into(),
        ))
        .await
        .unwrap();
        let leased = read_message(&mut host, "pieces_changed").await;
        assert_eq!(leased["pieces"].as_array().unwrap().len(), 2);
        // Drain the lease broadcast first: the denial below is sent directly
        // and can overtake it, leaving a stale update to be read as the move.
        let guest_leased = read_message(&mut guest, "pieces_changed").await;
        assert_eq!(guest_leased["revision"], leased["revision"]);
        guest
            .send(Message::Text(
                r#"{"type":"drag_start","piece_id":2}"#.into(),
            ))
            .await
            .unwrap();
        let denied = read_message(&mut guest, "error").await;
        assert!(
            denied["message"]
                .as_str()
                .unwrap()
                .contains("Another player")
        );
        host.send(Message::Text(
            serde_json::json!({
                "type":"move_selection", "anchor_piece_id":2,
                "x":selected_x + 5.0, "y":selected_y + 7.0
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let moved = read_message(&mut host, "pieces_changed").await;
        assert_eq!(moved["pieces"].as_array().unwrap().len(), 2);
        let guest_moved = read_message(&mut guest, "pieces_changed").await;
        assert_eq!(guest_moved["pieces"].as_array().unwrap().len(), 2);
        let host_piece_10 = moved["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|piece| piece["id"] == 10)
            .unwrap();
        let guest_piece_10 = guest_moved["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|piece| piece["id"] == 10)
            .unwrap();
        assert!(
            (host_piece_10["x"].as_f64().unwrap() - guest_piece_10["x"].as_f64().unwrap()).abs()
                < 0.01
        );
        assert!(
            (host_piece_10["y"].as_f64().unwrap() - guest_piece_10["y"].as_f64().unwrap()).abs()
                < 0.01
        );
        host.send(Message::Text(
            serde_json::json!({
                "type":"drag_end_selection", "anchor_piece_id":2,
                "x":selected_x + 5.0, "y":selected_y + 7.0
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        read_message(&mut host, "pieces_changed").await;

        host.send(Message::Text(
            r#"{"type":"drag_start","piece_id":1}"#.into(),
        ))
        .await
        .unwrap();
        read_message(&mut host, "pieces_changed").await;
        guest
            .send(Message::Text(
                r#"{"type":"drag_start","piece_id":1}"#.into(),
            ))
            .await
            .unwrap();
        let error = read_message(&mut guest, "error").await;
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("Another player")
        );

        host.send(Message::Text(
            r#"{"type":"move_piece","piece_id":1,"x":200,"y":0}"#.into(),
        ))
        .await
        .unwrap();
        read_message(&mut host, "pieces_changed").await;
        host.send(Message::Text(
            r#"{"type":"drag_end","piece_id":1,"x":200,"y":0}"#.into(),
        ))
        .await
        .unwrap();
        let placed = read_message(&mut host, "pieces_changed").await;
        assert!(
            placed["pieces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|piece| piece["id"] == 1 && piece["placed"] == true)
        );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let snapshot_file = fs::read_dir(&data_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap();
        let saved: RoomSnapshot =
            serde_json::from_slice(&fs::read(snapshot_file).unwrap()).unwrap();
        assert!(saved.piece(1).unwrap().placed);
        let restored_state = AppState::new(data_dir.clone());
        load_rooms(&restored_state).await;
        let restored_rooms = restored_state.rooms.read().await;
        let restored = restored_rooms.get(&code).unwrap().runtime.lock().await;
        assert!(restored.snapshot.as_ref().unwrap().piece(1).unwrap().placed);
        assert!(restored.snapshot.as_ref().unwrap().media_available);
        assert!(
            restored
                .snapshot
                .as_ref()
                .unwrap()
                .pieces
                .iter()
                .all(|piece| piece.held_by.is_none())
        );
        drop(restored);
        drop(restored_rooms);

        // Exercise a real disconnect/rejoin through the public WebSocket path.
        // Once both clients have left, the original host can reclaim host
        // control, and both saved piece placement and server media stay intact.
        guest.close(None).await.unwrap();
        read_message(&mut host, "peer_left").await;
        host.close(None).await.unwrap();
        let room = state.rooms.read().await.get(&code).unwrap().clone();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if room.runtime.lock().await.names.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("room peers should be removed after disconnect");

        let (mut rejoined_host, _) = connect_async(&url).await.unwrap();
        rejoined_host
            .send(Message::Text(
                serde_json::json!({
                    "type":"join_room", "invite_code":code, "name":"Host",
                    "media_sha256":"", "media_extension":""
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let rejoined = read_message(&mut rejoined_host, "welcome").await;
        assert_eq!(
            rejoined["snapshot"]["pieces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|piece| piece["id"] == 1)
                .unwrap()["placed"],
            true
        );
        assert_eq!(rejoined["snapshot"]["media_available"], true);
        assert_eq!(rejoined["snapshot"]["host_id"], rejoined["client_id"]);
        rejoined_host.close(None).await.unwrap();
        server.abort();
        let _ = fs::remove_dir_all(data_dir);
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    use futures_util::SinkExt;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{self, client::IntoClientRequest},
    };

    fn test_config() -> Config {
        Config::defaults(
            std::env::temp_dir().join(format!("pieceoff-hardening-{}", Uuid::new_v4())),
        )
    }

    /// Serve the real router, with connect info, on a free local port.
    async fn serve(config: &Config) -> (AppState, String) {
        let state = AppState::from_config(config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = application(state.clone());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (state, format!("ws://{address}/ws"))
    }

    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn open(
        url: &str,
        headers: &[(&'static str, &str)],
    ) -> Result<Socket, tungstenite::Error> {
        let mut request = url.into_client_request().unwrap();
        for (name, value) in headers {
            request.headers_mut().insert(*name, value.parse().unwrap());
        }
        connect_async(request).await.map(|(socket, _)| socket)
    }

    fn refused_with(result: Result<Socket, tungstenite::Error>) -> u16 {
        match result {
            Err(tungstenite::Error::Http(response)) => response.status().as_u16(),
            Ok(_) => panic!("the handshake should have been refused"),
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }

    fn join(code: &str) -> Message {
        Message::Text(
            serde_json::json!({
                "type": "join_room",
                "invite_code": code,
                "name": "Guesser",
                "media_sha256": "",
                "media_extension": "png",
            })
            .to_string()
            .into(),
        )
    }

    /// The text of the server's first reply.
    async fn reply(socket: &mut Socket) -> String {
        use futures_util::StreamExt;
        match socket.next().await {
            Some(Ok(tungstenite::Message::Text(text))) => text.to_string(),
            other => panic!("expected a text reply, got {other:?}"),
        }
    }

    async fn guess(url: &str, headers: &[(&'static str, &str)], code: &str) -> String {
        let mut socket = open(url, headers).await.unwrap();
        let message = match join(code) {
            Message::Text(text) => tungstenite::Message::Text(text.as_str().into()),
            _ => unreachable!(),
        };
        socket.send(message).await.unwrap();
        reply(&mut socket).await
    }

    #[test]
    fn invite_codes_are_eight_unambiguous_characters_and_do_not_repeat() {
        let mut seen = HashSet::new();
        for _ in 0..2000 {
            let code = new_invite_code();
            assert_eq!(code.len(), 8, "{code}");
            assert!(
                code.bytes().all(|byte| INVITE_ALPHABET.contains(&byte)),
                "{code}"
            );
            assert!(
                !code.contains(['0', 'O', '1', 'I']),
                "{code} has a look-alike character"
            );
            assert!(seen.insert(code), "a code repeated");
        }
    }

    #[tokio::test]
    async fn a_full_server_refuses_new_rooms_and_keeps_the_ones_it_has() {
        let mut config = test_config();
        config.max_rooms = 1;
        let state = AppState::from_config(&config);
        let create = |title: &str| ClientMessage::CreateRoom {
            name: "Host".into(),
            title: title.into(),
            media_sha256: "a".repeat(64),
            media_extension: "png".into(),
            width: 64,
            height: 48,
            rows: 2,
            columns: 2,
            initial_snapshot: None,
            duration_ms: 0,
        };
        let (_, first_code, _, _) = connect_room(&state, "host-1", create("First"))
            .await
            .unwrap();
        let error = connect_room(&state, "host-2", create("Second"))
            .await
            .err()
            .expect("the second room is refused");
        assert!(error.contains("full"), "{error}");
        let rooms = state.rooms.read().await;
        assert_eq!(rooms.len(), 1);
        assert!(rooms.contains_key(&first_code), "the first room survives");
    }

    #[tokio::test]
    async fn browser_origins_are_refused_unless_listed() {
        let mut config = test_config();
        config.allowed_origins = vec!["https://play.example.com".into()];
        let (_, url) = serve(&config).await;
        assert_eq!(
            refused_with(open(&url, &[("origin", "https://evil.example.org")]).await),
            403
        );
        // A listed origin and a native client (no origin) both get through.
        assert!(
            open(&url, &[("origin", "https://play.example.com")])
                .await
                .is_ok()
        );
        assert!(open(&url, &[]).await.is_ok());
    }

    #[tokio::test]
    async fn guessing_room_codes_runs_out_of_budget_per_address() {
        let mut config = test_config();
        config.failed_joins_per_hour = 3;
        config.trust_proxy = true;
        let (_, url) = serve(&config).await;
        let one = [("x-forwarded-for", "198.51.100.1")];
        for _ in 0..3 {
            let answer = guess(&url, &one, "ZZZZZZZZ").await;
            assert!(answer.contains("Room code not found"), "{answer}");
        }
        let blocked = guess(&url, &one, "ZZZZZZZZ").await;
        assert!(blocked.contains("Too many wrong room codes"), "{blocked}");
        // Another address behind the same proxy has its own budget.
        let two = [("x-forwarded-for", "198.51.100.2")];
        let other = guess(&url, &two, "ZZZZZZZZ").await;
        assert!(other.contains("Room code not found"), "{other}");
    }

    #[tokio::test]
    async fn too_many_new_connections_from_one_address_get_429() {
        let mut config = test_config();
        config.connections_per_minute = 2;
        let (_, url) = serve(&config).await;
        let _a = open(&url, &[]).await.unwrap();
        let _b = open(&url, &[]).await.unwrap();
        assert_eq!(refused_with(open(&url, &[]).await), 429);
    }

    #[tokio::test]
    async fn a_server_at_its_connection_limit_says_so_and_frees_a_slot_on_disconnect() {
        let mut config = test_config();
        config.max_connections = 1;
        let (state, url) = serve(&config).await;
        let held = open(&url, &[]).await.unwrap();
        assert_eq!(refused_with(open(&url, &[]).await), 503);
        drop(held);
        // The slot comes back once the server notices the socket closed.
        let mut reopened = false;
        for _ in 0..50 {
            if state.connection_slots.available_permits() == 1 {
                reopened = open(&url, &[]).await.is_ok();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(reopened, "the slot was never released");
    }

    #[tokio::test]
    async fn a_message_over_the_cap_ends_the_connection_without_a_reply() {
        use futures_util::StreamExt;
        let config = test_config();
        let (_, url) = serve(&config).await;
        let mut socket = open(&url, &[]).await.unwrap();
        let huge = "x".repeat(config.max_message_bytes + 1024);
        // The server may close before the whole message is written.
        let _ = socket.send(tungstenite::Message::Text(huge.into())).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("the server should close the connection")
            {
                Some(Ok(tungstenite::Message::Text(text))) => {
                    panic!("an oversized message got a reply: {text}")
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => break,
            }
        }
    }

    #[tokio::test]
    async fn the_health_check_sees_a_running_server_and_a_stopped_one() {
        let config = test_config();
        let state = AppState::from_config(&config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = application(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        assert!(health_check(address).await);
        server.abort();
        let _ = server.await;
        assert!(!health_check(address).await, "nothing is listening now");
    }

    #[tokio::test]
    async fn shutting_down_saves_every_room_for_the_next_start() {
        let config = test_config();
        let state = AppState::from_config(&config);
        let (room, code, _, _) = connect_room(
            &state,
            "host",
            ClientMessage::CreateRoom {
                name: "Host".into(),
                title: "Saved at exit".into(),
                media_sha256: "b".repeat(64),
                media_extension: "png".into(),
                width: 64,
                height: 48,
                rows: 2,
                columns: 2,
                initial_snapshot: None,
                duration_ms: 0,
            },
        )
        .await
        .unwrap();
        // A drag move that never reached a drop: not saved as it happens.
        {
            let mut runtime = room.runtime.lock().await;
            let snapshot = runtime.snapshot.as_mut().unwrap();
            snapshot.pieces[0].x += 5.0;
            snapshot.revision += 1;
        }
        let moved_x = room.runtime.lock().await.snapshot.as_ref().unwrap().pieces[0].x;
        persist_all(&state).await;

        let restored = AppState::from_config(&config);
        load_rooms(&restored).await;
        let rooms = restored.rooms.read().await;
        let snapshot = rooms
            .get(&code)
            .unwrap()
            .runtime
            .lock()
            .await
            .snapshot
            .clone()
            .unwrap();
        assert_eq!(snapshot.title, "Saved at exit");
        assert_eq!(snapshot.pieces[0].x, moved_x);
        drop(rooms);
        let _ = fs::remove_dir_all(&config.data_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn events_leave_the_server_in_revision_order_under_concurrent_changes() {
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "ORDER123".into(),
            "Ordering".into(),
            "a".repeat(64),
            "mp4".into(),
            64,
            48,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let room = Arc::new(RoomRecord::new(None));
        {
            let mut runtime = room.runtime.lock().await;
            runtime.host_id = Some("host".into());
            runtime.snapshot = Some(snapshot);
        }
        let mut events = room.events.subscribe();
        // The channel only holds 128, so a slow runner can lag this receiver.
        // A lag drops events but never reorders them, so skip it and check
        // order plus the final revision rather than an exact count.
        let collector = tokio::spawn(async move {
            let mut revisions = Vec::new();
            loop {
                match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
                    Ok(Ok(ServerMessage::PlaybackChanged { revision, .. })) => {
                        revisions.push(revision)
                    }
                    Ok(Ok(_)) => panic!("unexpected event"),
                    Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                    Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
                }
            }
            revisions
        });
        let tasks: Vec<_> = (0..8)
            .map(|task| {
                let room = room.clone();
                tokio::spawn(async move {
                    for step in 0..60u64 {
                        dispatch(
                            &room,
                            "host",
                            ClientMessage::SetPlayback {
                                playing: true,
                                position_ms: task * 1000 + step,
                                rate: 1.0,
                                loop_start_ms: 0,
                                loop_end_ms: 0,
                            },
                        )
                        .await
                        .unwrap();
                        tokio::task::yield_now().await;
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let revisions = collector.await.unwrap();
        let mut last = 0;
        for revision in &revisions {
            assert!(*revision > last, "revision {revision} arrived after {last}");
            last = *revision;
        }
        let final_revision = room
            .runtime
            .lock()
            .await
            .snapshot
            .as_ref()
            .unwrap()
            .revision;
        assert_eq!(revisions.last(), Some(&final_revision));
    }
}
