//! PieceOff Online: the hosted rooms service at `rooms.pieceoff.app`.
//!
//! A hosted server (the Cloudflare rooms Worker) differs from the axum room
//! server in how rooms are created and media moves: rooms are created over
//! HTTP with a per-device key, media goes through resumable multipart
//! uploads, and the public lobby lists open rooms. This module holds the
//! HTTP client for those flows and the pure rules behind them (link and code
//! parsing, service detection, lobby order, quick match). Every request here
//! blocks, so callers run it on a worker thread, never the UI thread.

use super::*;

/// The hosted room server the app uses unless the player picks another.
pub(crate) const ONLINE_SERVER_URL: &str = "wss://rooms.pieceoff.app/ws";
/// Where invite links point when the service does not say.
pub(crate) const DEFAULT_SHARE_URL: &str = "https://pieceoff.app/join";
/// The site that serves invite pages; links to it select PieceOff Online.
const SITE_HOSTS: [&str; 2] = ["pieceoff.app", "www.pieceoff.app"];
/// Hosted room codes: 8 characters from an alphabet without I, O, 0 and 1.
const ROOM_CODE_ALPHABET: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub(crate) const UNAVAILABLE_MESSAGE: &str =
    "PieceOff Online isn't available right now. Try again later, or use Local or LAN.";
pub(crate) const OFFLINE_MESSAGE: &str =
    "Can't reach PieceOff Online. Check your internet connection and try again.";
pub(crate) const RATE_LIMITED_MESSAGE: &str =
    "PieceOff Online is getting too many requests from your network. Wait a minute and try again.";
pub(crate) const STORAGE_FULL_MESSAGE: &str =
    "PieceOff Online is out of media space right now. Try a smaller file or try later.";

/// Whether `code` looks like a hosted room (or link) code.
pub(crate) fn valid_room_code(code: &str) -> bool {
    code.len() == 8 && code.chars().all(|c| ROOM_CODE_ALPHABET.contains(c))
}

// ---- Invite links ---------------------------------------------------------

/// What the join box was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Invite {
    /// The room code, upper-cased.
    pub(crate) code: String,
    /// It came as an invite link, so it is a PieceOff Online room.
    pub(crate) online: bool,
}

/// Read a bare room code, `https://pieceoff.app/join?c=CODE`, or the same
/// without the scheme. `None` when it is neither.
pub(crate) fn parse_invite(input: &str) -> Option<Invite> {
    let input = input.trim();
    if input.is_empty() || input.len() > 2048 {
        return None;
    }
    let looks_like_link = input.contains('/') || input.contains('?') || input.contains('.');
    if !looks_like_link {
        let code: String = input
            .chars()
            .filter(|c| !matches!(c, ' ' | '-'))
            .collect::<String>()
            .to_ascii_uppercase();
        let valid =
            !code.is_empty() && code.len() <= 32 && code.bytes().all(|b| b.is_ascii_alphanumeric());
        return valid.then_some(Invite {
            code,
            online: false,
        });
    }
    let rest = match input.split_once("://") {
        Some((scheme, rest))
            if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") =>
        {
            rest
        }
        Some(_) => return None,
        None => input,
    };
    let (authority, path_query) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
    if !SITE_HOSTS
        .iter()
        .any(|host| host.eq_ignore_ascii_case(authority))
    {
        return None;
    }
    let (path, query) = path_query.split_once('?').unwrap_or((path_query, ""));
    let query = query.split('#').next().unwrap_or_default();
    if path.trim_end_matches('/') != "/join" {
        return None;
    }
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("c="))?
        .trim()
        .to_ascii_uppercase();
    valid_room_code(&code).then_some(Invite { code, online: true })
}

/// The link a player shares to invite someone to a hosted room.
pub(crate) fn invite_link(share_url: &str, code: &str) -> String {
    let base = if share_url.starts_with("https://") || share_url.starts_with("http://") {
        share_url.trim_end_matches('/')
    } else {
        DEFAULT_SHARE_URL
    };
    format!("{base}?c={code}")
}

// ---- Service detection ----------------------------------------------------

/// `GET /api/info` from a hosted server.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct ServiceInfo {
    #[serde(default)]
    pub(crate) service: String,
    #[serde(default)]
    pub(crate) protocol: u32,
    #[serde(default)]
    pub(crate) hosted: bool,
    #[serde(default)]
    pub(crate) link_url: String,
    #[serde(default)]
    pub(crate) share_url: String,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    /// Whether players may upload their own media (owner decision
    /// 2026-11-10: off at launch). Missing means off.
    #[serde(default)]
    pub(crate) uploads: bool,
}

/// Shown wherever a hosted server without uploads would need a custom file.
pub(crate) const BUILTIN_ONLY_MESSAGE: &str =
    "PieceOff Online uses built-in puzzles for now. Use Local or LAN to play your own files.";

impl ServiceInfo {
    pub(crate) fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// Why this app cannot use the service, if the protocols differ.
    pub(crate) fn version_problem(&self) -> Option<String> {
        let ours = u32::from(pieceoff_core::PROTOCOL_VERSION);
        match self.protocol.cmp(&ours) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(format!(
                "PieceOff Online runs a newer version (protocol {}, this app speaks {ours}). Update PieceOff to play online.",
                self.protocol
            )),
            std::cmp::Ordering::Less => Some(format!(
                "PieceOff Online is on an older version (protocol {}, this app speaks {ours}). Try again later.",
                self.protocol
            )),
        }
    }

    pub(crate) fn share_url(&self) -> &str {
        if self.share_url.is_empty() {
            DEFAULT_SHARE_URL
        } else {
            &self.share_url
        }
    }
}

/// What kind of room server an address is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServiceKind {
    /// The hosted rooms service: HTTP create, multipart media, lobby.
    Hosted(ServiceInfo),
    /// An axum room server (it has no `/api/info`).
    SelfHosted,
}

impl ServiceKind {
    pub(crate) fn hosted(&self) -> Option<&ServiceInfo> {
        match self {
            Self::Hosted(info) => Some(info),
            Self::SelfHosted => None,
        }
    }
}

/// Decide from an `/api/info` answer. Anything but a well-formed hosted
/// answer (the axum server's 404 included) means the classic server.
pub(crate) fn classify_service(status: u16, body: &str) -> ServiceKind {
    if status != 200 {
        return ServiceKind::SelfHosted;
    }
    match serde_json::from_str::<ServiceInfo>(body) {
        Ok(info) if info.hosted && info.service == "pieceoff-rooms" => ServiceKind::Hosted(info),
        _ => ServiceKind::SelfHosted,
    }
}

/// The `http(s)://host` a room URL's HTTP endpoints live on.
pub(crate) fn http_base(ws_url: &str) -> Result<String, String> {
    http_media_url(ws_url, "")
}

fn api_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5).min(timeout))
        .timeout(timeout)
        // Secrets (device keys, poll secrets) must never follow a redirect
        // to another origin.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

/// Ask a room server what it is. `Err` means it could not be reached.
pub(crate) fn detect_service(ws_url: &str) -> Result<ServiceKind, String> {
    let url = http_media_url(ws_url, "/api/info")?;
    let response = api_client(Duration::from_secs(6))?
        .get(url)
        .send()
        .map_err(|error| describe_request_error(&error))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    Ok(classify_service(status, &body))
}

// ---- Errors ---------------------------------------------------------------

fn json_error(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = value.get("error")?.as_str()?;
    let text: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
    (!text.trim().is_empty()).then(|| text.trim().to_owned())
}

/// A failed answer from the hosted service, in words a player can act on.
pub(crate) fn describe_http_failure(status: u16, body: &str) -> String {
    match status {
        429 => RATE_LIMITED_MESSAGE.into(),
        507 => json_error(body).unwrap_or_else(|| STORAGE_FULL_MESSAGE.into()),
        413 => "That file is too large for PieceOff Online.".into(),
        426 => "PieceOff Online needs a newer app. Update PieceOff to play online.".into(),
        500..=599 => {
            format!("PieceOff Online had a problem (HTTP {status}). Try again in a moment.")
        }
        _ => json_error(body).unwrap_or_else(|| format!("PieceOff Online answered HTTP {status}")),
    }
}

/// A request that never got an answer.
pub(crate) fn describe_request_error(error: &reqwest::Error) -> String {
    if error.is_connect() || error.is_timeout() || error.is_request() {
        OFFLINE_MESSAGE.into()
    } else {
        format!("Could not talk to PieceOff Online: {error}")
    }
}

// ---- Device keys ----------------------------------------------------------

/// This computer's private key for creating rooms on PieceOff Online. It is
/// kept in the settings file and never shown, logged or put in a message.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DeviceKey(String);

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "DeviceKey(none)"
        } else {
            "DeviceKey(<redacted>)"
        })
    }
}

impl DeviceKey {
    /// A key as the service hands it out: `pok_` and base64url characters.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let body = text.strip_prefix("pok_")?;
        let valid = (16..=128).contains(&body.len())
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        valid.then(|| Self(text.to_owned()))
    }

    pub(crate) fn is_set(&self) -> bool {
        !self.0.is_empty()
    }

    /// Drop anything that is not a well-formed key (a hand-edited file).
    pub(crate) fn sanitized(self) -> Self {
        Self::parse(&self.0).unwrap_or_default()
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

/// The origin that issues PieceOff Online device keys today.
pub(crate) const OFFICIAL_ORIGIN: &str = "https://rooms.pieceoff.app";

pub(crate) const INSECURE_SERVER_MESSAGE: &str =
    "Creating rooms on this server needs a secure (wss://) address. Joining works without it.";

/// The origin (`https://host[:port]`, lower-cased, default port dropped) a
/// device key for this room server belongs to. Keys are issued and used per
/// origin and only over TLS: a plaintext server never gets or receives one,
/// unless `allow_plaintext` (tests only).
pub(crate) fn key_origin(ws_url: &str, allow_plaintext: bool) -> Result<String, String> {
    let base = http_base(ws_url)?;
    let (scheme, authority) = base
        .split_once("://")
        .ok_or("Invalid room server address")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && !(allow_plaintext && scheme == "http") {
        return Err(INSECURE_SERVER_MESSAGE.into());
    }
    let mut authority = authority.to_ascii_lowercase();
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    if let Some(stripped) = authority.strip_suffix(default_port) {
        authority = stripped.to_owned();
    }
    if authority.is_empty() {
        return Err("Invalid room server address".into());
    }
    Ok(format!("{scheme}://{authority}"))
}

/// `POST /api/links`: a code to approve in the browser. The poll secret is
/// only ever sent back to the service.
#[derive(Clone, Deserialize)]
pub(crate) struct LinkStart {
    pub(crate) link_code: String,
    poll_secret: String,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) expires_in: u64,
}

impl std::fmt::Debug for LinkStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkStart")
            .field("link_code", &self.link_code)
            .field("url", &self.url)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Check a link answer: the code must be a room-style code and the page an
/// http(s) address that carries it.
pub(crate) fn parse_link_start(body: &str) -> Result<LinkStart, String> {
    let mut start: LinkStart = serde_json::from_str(body)
        .map_err(|_| "PieceOff Online sent an unreadable link".to_owned())?;
    start.link_code = start.link_code.trim().to_ascii_uppercase();
    let url_ok = (start.url.starts_with("https://") || start.url.starts_with("http://"))
        && start.url.len() <= 512
        && !start
            .url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control());
    if !valid_room_code(&start.link_code) || start.poll_secret.is_empty() || !url_ok {
        return Err("PieceOff Online sent an unreadable link".into());
    }
    if start.expires_in == 0 {
        start.expires_in = 600;
    }
    start.expires_in = start.expires_in.min(3600);
    Ok(start)
}

/// A name for this device on the link page; deliberately not the host name.
pub(crate) fn device_name() -> String {
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    format!("PieceOff on {os}")
}

pub(crate) fn start_link(base: &str) -> Result<LinkStart, String> {
    let response = api_client(Duration::from_secs(10))?
        .post(format!("{base}/api/links"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "device_name": device_name() }).to_string())
        .send()
        .map_err(|error| describe_request_error(&error))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    if status != 200 && status != 201 {
        return Err(describe_http_failure(status, &body));
    }
    parse_link_start(&body)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkPoll {
    Pending,
    Approved(DeviceKey),
    /// The code expired, was used, or never existed.
    Expired,
}

pub(crate) fn classify_link_poll(status: u16, body: &str) -> Result<LinkPoll, String> {
    match status {
        202 => Ok(LinkPoll::Pending),
        200 => {
            let value: serde_json::Value = serde_json::from_str(body)
                .map_err(|_| "PieceOff Online sent an unreadable answer".to_owned())?;
            match value.get("status").and_then(|s| s.as_str()) {
                Some("pending") => Ok(LinkPoll::Pending),
                Some("approved") => value
                    .get("device_key")
                    .and_then(|key| key.as_str())
                    .and_then(DeviceKey::parse)
                    .map(LinkPoll::Approved)
                    .ok_or_else(|| "PieceOff Online sent an unusable device key".to_owned()),
                _ => Err("PieceOff Online sent an unreadable answer".into()),
            }
        }
        404 | 410 => Ok(LinkPoll::Expired),
        status => Err(describe_http_failure(status, body)),
    }
}

pub(crate) fn poll_link(base: &str, start: &LinkStart) -> Result<LinkPoll, String> {
    let response = api_client(Duration::from_secs(10))?
        .get(format!("{base}/api/links/{}", start.link_code))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", start.poll_secret),
        )
        .send()
        .map_err(|error| describe_request_error(&error))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    classify_link_poll(status, &body)
}

// ---- Room creation --------------------------------------------------------

/// `POST /api/rooms` answered: the room and the creator's secret.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CreatedRoom {
    pub(crate) code: String,
    pub(crate) client_id: String,
    pub(crate) resume_token: String,
}

impl std::fmt::Debug for CreatedRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedRoom")
            .field("code", &self.code)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CreateOutcome {
    Created(CreatedRoom),
    /// The device key is unknown or revoked: link this computer again.
    Relink,
}

pub(crate) fn classify_create(status: u16, body: &str) -> Result<CreateOutcome, String> {
    match status {
        200 | 201 => {
            let created: CreatedRoom = serde_json::from_str(body)
                .map_err(|_| "PieceOff Online sent an unreadable room".to_owned())?;
            let code = created.code.trim().to_ascii_uppercase();
            if !valid_room_code(&code) || created.resume_token.is_empty() {
                return Err("PieceOff Online sent an unreadable room".into());
            }
            Ok(CreateOutcome::Created(CreatedRoom { code, ..created }))
        }
        401 => Ok(CreateOutcome::Relink),
        403 => Err(match json_error(body) {
            // Already a sentence about the service (uploads off, for one).
            Some(error) if error.starts_with("PieceOff Online") => error,
            Some(error) => format!("PieceOff Online refused to create the room: {error}"),
            None => "PieceOff Online refused to create the room.".into(),
        }),
        status => Err(describe_http_failure(status, body)),
    }
}

/// Create a room at `origin` with the key that origin issued. The request
/// goes to exactly `origin`, which callers take from `key_origin`.
pub(crate) fn create_room(
    origin: &str,
    key: &DeviceKey,
    message: &ClientMessage,
) -> Result<CreateOutcome, String> {
    let body = serde_json::to_string(message).map_err(|error| error.to_string())?;
    let response = api_client(Duration::from_secs(20))?
        .post(format!("{origin}/api/rooms"))
        .header(reqwest::header::AUTHORIZATION, key.bearer())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .map_err(|error| describe_request_error(&error))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    classify_create(status, &body)
}

// ---- Lobby ----------------------------------------------------------------

/// One public room as `GET /api/lobby` lists it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct LobbyRoom {
    pub(crate) code: String,
    pub(crate) title: String,
    pub(crate) host_name: String,
    pub(crate) players: u32,
    pub(crate) max_players: u32,
    pub(crate) spectators: u32,
    pub(crate) pieces: u32,
    pub(crate) placed: u32,
    pub(crate) cut_style: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

impl LobbyRoom {
    pub(crate) fn has_free_slot(&self) -> bool {
        self.players < self.max_players
    }

    pub(crate) fn progress_percent(&self) -> u32 {
        if self.pieces == 0 {
            0
        } else {
            (u64::from(self.placed.min(self.pieces)) * 100 / u64::from(self.pieces)) as u32
        }
    }

    pub(crate) fn cut_label(&self) -> &'static str {
        serde_json::from_value::<CutStyle>(serde_json::Value::String(self.cut_style.clone()))
            .unwrap_or_default()
            .label()
    }
}

fn clean_text(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Read the lobby, dropping malformed entries, tidying text, and putting
/// it in lobby order.
pub(crate) fn parse_lobby(body: &str) -> Result<Vec<LobbyRoom>, String> {
    #[derive(Deserialize)]
    struct Lobby {
        #[serde(default)]
        rooms: Vec<serde_json::Value>,
    }
    let lobby: Lobby = serde_json::from_str(body)
        .map_err(|_| "PieceOff Online sent an unreadable room list".to_owned())?;
    let mut rooms: Vec<LobbyRoom> = lobby
        .rooms
        .into_iter()
        .filter_map(|value| serde_json::from_value::<LobbyRoom>(value).ok())
        .filter_map(|mut room| {
            room.code = room.code.trim().to_ascii_uppercase();
            if !valid_room_code(&room.code) || room.max_players == 0 {
                return None;
            }
            room.title = clean_text(&room.title, 80);
            if room.title.is_empty() {
                room.title = "Puzzle".into();
            }
            room.host_name = clean_text(&room.host_name, 24);
            Some(room)
        })
        .take(100)
        .collect();
    sort_lobby(&mut rooms);
    Ok(rooms)
}

/// Lobby order: rooms with a free player slot first, then the busiest, then
/// the newest.
pub(crate) fn sort_lobby(rooms: &mut [LobbyRoom]) {
    rooms.sort_by(lobby_order);
}

fn lobby_order(a: &LobbyRoom, b: &LobbyRoom) -> std::cmp::Ordering {
    b.has_free_slot()
        .cmp(&a.has_free_slot())
        .then(b.players.cmp(&a.players))
        .then(b.created_at.cmp(&a.created_at))
}

/// Quick match joins the first room (in lobby order) with room to play;
/// `None` means start a new public room instead.
pub(crate) fn quick_match_pick(rooms: &[LobbyRoom]) -> Option<&LobbyRoom> {
    rooms
        .iter()
        .filter(|room| room.has_free_slot())
        .min_by(|a, b| lobby_order(a, b))
}

pub(crate) fn fetch_lobby(base: &str) -> Result<Vec<LobbyRoom>, String> {
    let response = api_client(Duration::from_secs(8))?
        .get(format!("{base}/api/lobby"))
        .send()
        .map_err(|error| describe_request_error(&error))?;
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    match status {
        200 => parse_lobby(&body),
        404 => Err("PieceOff Online has no public rooms yet.".into()),
        status => Err(describe_http_failure(status, &body)),
    }
}

// ---- Hosted media ---------------------------------------------------------

/// Part size for a multipart upload of `size` bytes: one part when it fits,
/// otherwise 8 MiB parts, grown (to the service's 64 MiB cap) so a file
/// needs at most 10 000. Deterministic, so a restarted upload resumes.
pub(crate) fn plan_part_size(size: u64) -> Result<u64, String> {
    const MIB: u64 = 1024 * 1024;
    const PART: u64 = 8 * MIB;
    const MAX_PART: u64 = 64 * MIB;
    const MAX_PARTS: u64 = 10_000;
    if size == 0 {
        return Err("Room media must not be empty".into());
    }
    if size <= PART {
        return Ok(size);
    }
    let needed = size.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB;
    let part = needed.max(PART);
    if part > MAX_PART {
        return Err("That file is too large for PieceOff Online.".into());
    }
    Ok(part)
}

/// The byte range of part `n` (1-based).
pub(crate) fn part_range(size: u64, part_size: u64, n: u64) -> Option<(u64, u64)> {
    let count = size.div_ceil(part_size);
    if n == 0 || n > count {
        return None;
    }
    let start = (n - 1) * part_size;
    Some((start, (start + part_size).min(size)))
}

#[derive(Debug, Deserialize)]
struct UploadStart {
    #[serde(default)]
    complete: bool,
    #[serde(default)]
    upload_id: String,
    #[serde(default)]
    part_size: u64,
    #[serde(default)]
    missing: Vec<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct MissingParts {
    #[serde(default)]
    missing: Vec<u64>,
}

/// Media for a room on the hosted service, below `/api/rooms/<CODE>/media`.
pub(crate) struct HostedMedia {
    /// `https://host`.
    pub(crate) base: String,
    pub(crate) code: String,
    /// This player's resume token, which authorises downloads.
    pub(crate) resume_token: Arc<Mutex<Option<String>>>,
}

impl HostedMedia {
    fn media_url(&self, rest: &str) -> String {
        format!("{}/api/rooms/{}/media{rest}", self.base, self.code)
    }

    fn upload_client() -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .connect_timeout(TransferTimeouts::DEFAULT.connect)
            .tcp_keepalive(Duration::from_secs(30))
            .timeout(None)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())
    }

    /// Start (or resume) the upload and send every missing part, then
    /// finish it. A dropped part is sent again a few times before giving up.
    pub(crate) fn upload_file(&self, sha256: &str, token: &str, path: &Path) -> Result<(), String> {
        let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        if size == 0 || size > MAX_ROOM_MEDIA_BYTES {
            return Err(format!(
                "Room media must be between 1 byte and {}",
                format_bytes(MAX_ROOM_MEDIA_BYTES)
            ));
        }
        let part_size = plan_part_size(size)?;
        let client = Self::upload_client()?;
        let bearer = format!("Bearer {token}");
        let response = client
            .post(self.media_url(""))
            .header(reqwest::header::AUTHORIZATION, &bearer)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({ "sha256": sha256, "size": size, "part_size": part_size })
                    .to_string(),
            )
            .send()
            .map_err(|error| describe_request_error(&error))?;
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        if status != 200 {
            return Err(describe_http_failure(status, &body));
        }
        let start: UploadStart = serde_json::from_str(&body)
            .map_err(|_| "PieceOff Online sent an unreadable upload".to_owned())?;
        if start.complete {
            return Ok(());
        }
        if start.upload_id.is_empty() || start.part_size != part_size {
            return Err("PieceOff Online sent an unreadable upload".into());
        }
        let count = size.div_ceil(part_size);
        let mut missing = start.missing;
        missing.retain(|n| (1..=count).contains(n));
        let pending: u64 = missing
            .iter()
            .filter_map(|n| part_range(size, part_size, *n))
            .map(|(a, b)| b - a)
            .sum();
        UPLOAD_PROGRESS.begin(size - pending, size);
        let result = (|| {
            for attempt in 0..2 {
                for n in &missing {
                    self.send_part(
                        &client,
                        &bearer,
                        &mut file,
                        size,
                        part_size,
                        &start.upload_id,
                        *n,
                    )?;
                }
                let response = client
                    .post(self.media_url(&format!("/{}/complete", start.upload_id)))
                    .header(reqwest::header::AUTHORIZATION, &bearer)
                    .send()
                    .map_err(|error| describe_request_error(&error))?;
                let status = response.status().as_u16();
                let body = response.text().unwrap_or_default();
                match status {
                    200 => return Ok(()),
                    // Some part did not stick; send those again once.
                    409 if attempt == 0 => {
                        let parts: MissingParts = serde_json::from_str(&body).unwrap_or_default();
                        missing = parts.missing;
                        missing.retain(|n| (1..=count).contains(n));
                        if missing.is_empty() {
                            return Err(describe_http_failure(status, &body));
                        }
                    }
                    status => return Err(describe_http_failure(status, &body)),
                }
            }
            Err("PieceOff Online did not accept every part of the upload".into())
        })();
        UPLOAD_PROGRESS.finish();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn send_part(
        &self,
        client: &reqwest::blocking::Client,
        bearer: &str,
        file: &mut std::fs::File,
        size: u64,
        part_size: u64,
        upload_id: &str,
        n: u64,
    ) -> Result<(), String> {
        let (start, end) = part_range(size, part_size, n)
            .ok_or("PieceOff Online asked for a part that does not exist")?;
        let mut bytes = vec![0u8; (end - start) as usize];
        std::io::Seek::seek(file, std::io::SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        file.read_exact(&mut bytes)
            .map_err(|_| "The media file changed while uploading".to_owned())?;
        let part_hash = hex(Sha256::digest(&bytes));
        let url = self.media_url(&format!("/{upload_id}/parts/{n}"));
        let mut delay = Duration::from_secs(1);
        let mut last_error = String::new();
        for attempt in 0..4 {
            let sent = client
                .put(&url)
                .header(reqwest::header::AUTHORIZATION, bearer)
                .header("x-part-sha256", &part_hash)
                .header(reqwest::header::CONTENT_LENGTH, bytes.len() as u64)
                .body(bytes.clone())
                .send();
            match sent {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response.text().unwrap_or_default();
                    match status {
                        200 => {
                            UPLOAD_PROGRESS.advance(bytes.len() as u64);
                            return Ok(());
                        }
                        // Busy, a part already in flight, or a storage hiccup.
                        409 | 429 | 500..=599 => last_error = describe_http_failure(status, &body),
                        status => return Err(describe_http_failure(status, &body)),
                    }
                }
                Err(error) => last_error = describe_request_error(&error),
            }
            if attempt < 3 {
                thread::sleep(delay);
                delay *= 2;
            }
        }
        Err(last_error)
    }

    /// Download into the media cache, resuming a partial file with `Range`.
    pub(crate) fn download_file(&self, sha256: &str, extension: &str) -> Result<PathBuf, String> {
        let token = self
            .resume_token
            .lock()
            .ok()
            .and_then(|token| token.clone())
            .unwrap_or_default();
        let url = self.media_url(&format!("/{sha256}"));
        download_into_cache(
            sha256,
            extension,
            TransferTimeouts::DEFAULT,
            move |client| {
                if token.is_empty() {
                    return Err("Not allowed to download this room's media yet".into());
                }
                Ok(client
                    .get(url.clone())
                    .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}")))
            },
        )
    }
}

impl MediaTransport for HostedMedia {
    fn upload(&self, sha256: &str, token: &str, path: &Path) -> Result<(), String> {
        self.upload_file(sha256, token, path)
    }

    fn download(&self, sha256: &str, extension: &str) -> Result<PathBuf, String> {
        self.download_file(sha256, extension)
    }
}

/// What the network thread needs to know about a hosted room.
#[derive(Clone)]
pub(crate) struct HostedSession {
    pub(crate) code: String,
    /// The player's resume token: sent when joining (the creator's claims
    /// the host role), updated from each `welcome`, and used for downloads.
    pub(crate) resume_token: Arc<Mutex<Option<String>>>,
    pub(crate) info: ServiceInfo,
}

impl std::fmt::Debug for HostedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedSession")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

impl HostedSession {
    pub(crate) fn new(code: String, resume_token: Option<String>, info: ServiceInfo) -> Self {
        Self {
            code,
            resume_token: Arc::new(Mutex::new(resume_token.filter(|token| !token.is_empty()))),
            info,
        }
    }

    pub(crate) fn token(&self) -> Option<String> {
        self.resume_token
            .lock()
            .ok()
            .and_then(|token| token.clone())
    }

    /// The socket URL for this room: the Worker routes by `code`.
    pub(crate) fn socket_url(&self, versioned: &str) -> String {
        let separator = if versioned.contains('?') { '&' } else { '?' };
        format!("{versioned}{separator}code={}", self.code)
    }

    /// The first message as JSON, carrying the resume token when we hold one.
    pub(crate) fn first_message_json(&self, message: &ClientMessage) -> Option<String> {
        let mut value = serde_json::to_value(message).ok()?;
        if let (Some(token), Some(object)) = (self.token(), value.as_object_mut()) {
            if object.get("type").and_then(|t| t.as_str()) == Some("join_room") {
                object.insert("resume_token".into(), serde_json::Value::String(token));
            }
        }
        serde_json::to_string(&value).ok()
    }

    /// Keep the resume token a hosted `welcome` carries.
    pub(crate) fn absorb_welcome(&self, raw: &str) {
        let token = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|value| value.get("resume_token")?.as_str().map(str::to_owned))
            .filter(|token| !token.is_empty() && token.len() <= 256);
        if let (Some(token), Ok(mut slot)) = (token, self.resume_token.lock()) {
            *slot = Some(token);
        }
    }

    pub(crate) fn media(&self, ws_url: &str) -> Result<HostedMedia, String> {
        Ok(HostedMedia {
            base: http_base(ws_url)?,
            code: self.code.clone(),
            resume_token: self.resume_token.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_links_and_bare_codes_are_read() {
        let online = |code: &str| {
            Some(Invite {
                code: code.into(),
                online: true,
            })
        };
        assert_eq!(
            parse_invite("https://pieceoff.app/join?c=ABCD2345"),
            online("ABCD2345")
        );
        assert_eq!(
            parse_invite("  pieceoff.app/join?c=abcd2345 "),
            online("ABCD2345")
        );
        assert_eq!(
            parse_invite("http://www.pieceoff.app/join/?c=ABCD2345&x=1#top"),
            online("ABCD2345")
        );
        assert_eq!(
            parse_invite("https://PieceOff.app/join?utm=a&c=WXYZ6789"),
            online("WXYZ6789")
        );
        assert_eq!(
            parse_invite("abcd-2345"),
            Some(Invite {
                code: "ABCD2345".into(),
                online: false
            })
        );
        assert_eq!(
            parse_invite("JOIN42"),
            Some(Invite {
                code: "JOIN42".into(),
                online: false
            })
        );
        for bad in [
            "",
            "https://evil.example/join?c=ABCD2345",
            "https://pieceoff.app.evil.example/join?c=ABCD2345",
            "https://pieceoff.app/rooms?c=ABCD2345",
            "https://pieceoff.app/join?c=ABCD0123",
            "https://pieceoff.app/join",
            "ftp://pieceoff.app/join?c=ABCD2345",
            "ab$cd",
        ] {
            assert_eq!(parse_invite(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn invite_links_use_the_services_share_page() {
        assert_eq!(
            invite_link("https://pieceoff.app/join", "ABCD2345"),
            "https://pieceoff.app/join?c=ABCD2345"
        );
        assert_eq!(
            invite_link("https://pieceoff.app/join/", "ABCD2345"),
            "https://pieceoff.app/join?c=ABCD2345"
        );
        assert_eq!(
            invite_link("javascript:x", "ABCD2345"),
            "https://pieceoff.app/join?c=ABCD2345"
        );
        let link = invite_link(DEFAULT_SHARE_URL, "WXYZ6789");
        assert_eq!(parse_invite(&link).unwrap().code, "WXYZ6789");
    }

    #[test]
    fn hosted_servers_are_told_apart_from_axum() {
        let body = r#"{"service":"pieceoff-rooms","protocol":15,"hosted":true,
            "link_url":"https://pieceoff.app/link","share_url":"https://pieceoff.app/join",
            "features":["device_keys","lobby"]}"#;
        let ServiceKind::Hosted(info) = classify_service(200, body) else {
            panic!("hosted");
        };
        assert!(info.has_feature("lobby"));
        assert_eq!(info.version_problem(), None);
        assert!(!info.uploads, "uploads are off unless the service says so");
        let ServiceKind::Hosted(open) = classify_service(
            200,
            r#"{"service":"pieceoff-rooms","protocol":15,"hosted":true,"uploads":true}"#,
        ) else {
            panic!("hosted");
        };
        assert!(open.uploads);
        assert_eq!(classify_service(404, "Not Found"), ServiceKind::SelfHosted);
        assert_eq!(classify_service(200, "<html>"), ServiceKind::SelfHosted);
        assert_eq!(
            classify_service(200, r#"{"service":"other","hosted":true}"#),
            ServiceKind::SelfHosted
        );
        assert_eq!(classify_service(500, body), ServiceKind::SelfHosted);
        let newer = ServiceInfo {
            protocol: u32::from(pieceoff_core::PROTOCOL_VERSION) + 1,
            ..info.clone()
        };
        assert!(newer.version_problem().unwrap().contains("Update PieceOff"));
        let older = ServiceInfo {
            protocol: 1,
            ..info
        };
        assert!(older.version_problem().unwrap().contains("older version"));
    }

    #[test]
    fn errors_name_the_problem() {
        assert_eq!(describe_http_failure(429, ""), RATE_LIMITED_MESSAGE);
        assert_eq!(describe_http_failure(507, ""), STORAGE_FULL_MESSAGE);
        assert_eq!(
            describe_http_failure(507, r#"{"error":"Out of space"}"#),
            "Out of space"
        );
        assert!(describe_http_failure(503, "").contains("Try again"));
        assert_eq!(
            describe_http_failure(400, r#"{"error":"Bad\u0007 thing"}"#),
            "Bad thing"
        );
    }

    #[test]
    fn device_keys_are_checked_and_never_printed() {
        let key = DeviceKey::parse("pok_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG").unwrap();
        assert!(key.is_set());
        assert_eq!(format!("{key:?}"), "DeviceKey(<redacted>)");
        assert!(DeviceKey::parse("abcdefghijklmnopqrstuvwxyz").is_none());
        assert!(DeviceKey::parse("pok_short").is_none());
        assert!(DeviceKey::parse("pok_abcdefghijklmnop qrstuvwxyz").is_none());
        let stored: DeviceKey =
            serde_json::from_str("\"pok_<script>alert(1)</script>xxxx\"").unwrap();
        assert!(!stored.sanitized().is_set());
    }

    #[test]
    fn device_keys_belong_to_exact_tls_origins() {
        assert_eq!(
            key_origin(ONLINE_SERVER_URL, false).unwrap(),
            OFFICIAL_ORIGIN
        );
        assert_eq!(
            key_origin("wss://Rooms.PieceOff.app:443/ws", false).unwrap(),
            OFFICIAL_ORIGIN
        );
        assert_eq!(
            key_origin("wss://rooms.pieceoff.app:8443/ws", false).unwrap(),
            "https://rooms.pieceoff.app:8443"
        );
        assert_eq!(
            key_origin("wss://evil.example/ws", false).unwrap(),
            "https://evil.example"
        );
        for plaintext in [
            "ws://rooms.pieceoff.app/ws",
            "ws://127.0.0.1:8787/ws",
            "ws://x:443/ws",
        ] {
            assert_eq!(
                key_origin(plaintext, false).unwrap_err(),
                INSECURE_SERVER_MESSAGE,
                "{plaintext}"
            );
        }
        assert_eq!(
            key_origin("ws://127.0.0.1:80/ws", true).unwrap(),
            "http://127.0.0.1"
        );
    }

    #[test]
    fn link_answers_and_polls_are_read() {
        let start = parse_link_start(
            r#"{"link_code":"abcd2345","poll_secret":"s3cret","url":"https://pieceoff.app/link?c=ABCD2345","expires_in":600}"#,
        )
        .unwrap();
        assert_eq!(start.link_code, "ABCD2345");
        assert!(!format!("{start:?}").contains("s3cret"));
        assert!(
            parse_link_start(r#"{"link_code":"ABCD2345","poll_secret":"s","url":"javascript:1"}"#)
                .is_err()
        );
        assert!(
            parse_link_start(r#"{"link_code":"BAD","poll_secret":"s","url":"https://x"}"#).is_err()
        );
        assert_eq!(
            classify_link_poll(202, r#"{"status":"pending"}"#),
            Ok(LinkPoll::Pending)
        );
        assert_eq!(classify_link_poll(404, ""), Ok(LinkPoll::Expired));
        let approved = classify_link_poll(
            200,
            r#"{"status":"approved","device_key":"pok_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG"}"#,
        )
        .unwrap();
        assert!(matches!(approved, LinkPoll::Approved(key) if key.is_set()));
        assert!(classify_link_poll(200, r#"{"status":"approved","device_key":"nope"}"#).is_err());
        assert_eq!(
            classify_link_poll(429, "").unwrap_err(),
            RATE_LIMITED_MESSAGE
        );
    }

    #[test]
    fn room_creation_answers_and_relink() {
        let created = classify_create(
            200,
            r#"{"code":"ABCD2345","client_id":"c","resume_token":"t"}"#,
        )
        .unwrap();
        assert!(
            matches!(&created, CreateOutcome::Created(room) if room.code == "ABCD2345" && room.resume_token == "t")
        );
        assert!(!format!("{created:?}").contains("\"t\""));
        assert_eq!(
            classify_create(401, r#"{"error":"x","relink":true}"#),
            Ok(CreateOutcome::Relink)
        );
        assert_eq!(classify_create(429, "").unwrap_err(), RATE_LIMITED_MESSAGE);
        assert!(
            classify_create(200, r#"{"code":"??","client_id":"c","resume_token":"t"}"#).is_err()
        );
        assert!(
            classify_create(403, r#"{"error":"Verification failed"}"#)
                .unwrap_err()
                .contains("Verification failed")
        );
        let uploads_off = format!(r#"{{"error":"{BUILTIN_ONLY_MESSAGE}"}}"#);
        assert_eq!(
            classify_create(403, &uploads_off).unwrap_err(),
            BUILTIN_ONLY_MESSAGE
        );
    }

    fn room(code: &str, players: u32, max: u32, created_at: u64) -> LobbyRoom {
        LobbyRoom {
            code: code.into(),
            title: "Slow Orbit".into(),
            players,
            max_players: max,
            created_at,
            ..LobbyRoom::default()
        }
    }

    #[test]
    fn lobby_order_puts_open_busy_new_rooms_first() {
        let mut rooms = vec![
            room("AAAAAAAA", 8, 8, 9),
            room("BBBBBBBB", 1, 8, 5),
            room("CCCCCCCC", 3, 8, 1),
            room("DDDDDDDD", 1, 8, 7),
        ];
        sort_lobby(&mut rooms);
        let order: Vec<&str> = rooms.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(order, ["CCCCCCCC", "DDDDDDDD", "BBBBBBBB", "AAAAAAAA"]);
    }

    #[test]
    fn quick_match_joins_an_open_room_or_starts_one() {
        let rooms = vec![
            room("AAAAAAAA", 8, 8, 9),
            room("BBBBBBBB", 2, 8, 1),
            room("CCCCCCCC", 2, 8, 3),
        ];
        assert_eq!(quick_match_pick(&rooms).unwrap().code, "CCCCCCCC");
        assert!(quick_match_pick(&[room("AAAAAAAA", 8, 8, 1)]).is_none());
        assert!(quick_match_pick(&[]).is_none());
    }

    #[test]
    fn lobby_entries_are_validated_and_tidied() {
        let body = r#"{"rooms":[
            {"code":"abcd2345","title":"Tidal\u0000 Glass","host_name":"A very long host name that goes on","players":1,"max_players":8,"pieces":48,"placed":12,"cut_style":"hexagon"},
            {"code":"bad","players":1,"max_players":8},
            {"code":"WXYZ6789","players":"x"},
            {"code":"WXYZ6789","players":8,"max_players":8}
        ]}"#;
        let rooms = parse_lobby(body).unwrap();
        assert_eq!(rooms.len(), 2);
        assert_eq!(rooms[0].code, "ABCD2345");
        assert_eq!(rooms[0].title, "Tidal Glass");
        assert_eq!(rooms[0].host_name.chars().count(), 24);
        assert_eq!(rooms[0].progress_percent(), 25);
        assert_eq!(rooms[0].cut_label(), CutStyle::Hexagon.label());
        assert!(!rooms[1].has_free_slot());
        assert!(parse_lobby("nope").is_err());
    }

    #[test]
    fn multipart_parts_cover_the_file_and_stay_within_limits() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(plan_part_size(1).unwrap(), 1);
        assert_eq!(plan_part_size(8 * MIB).unwrap(), 8 * MIB);
        assert_eq!(plan_part_size(8 * MIB + 1).unwrap(), 8 * MIB);
        // 200 GB would need more than 10 000 parts of 8 MiB.
        let big = 100 * 1024 * MIB;
        let part = plan_part_size(big).unwrap();
        assert!(big.div_ceil(part) <= 10_000 && part.is_multiple_of(MIB) && part <= 64 * MIB);
        assert!(plan_part_size(700 * 1024 * 1024 * MIB).is_err());
        assert!(plan_part_size(0).is_err());
        let size = 20 * MIB + 5;
        let part = plan_part_size(size).unwrap();
        let ranges: Vec<_> = (1..=3)
            .map(|n| part_range(size, part, n).unwrap())
            .collect();
        assert_eq!(
            ranges,
            [(0, 8 * MIB), (8 * MIB, 16 * MIB), (16 * MIB, size)]
        );
        assert_eq!(part_range(size, part, 4), None);
        assert_eq!(part_range(size, part, 0), None);
    }

    #[test]
    fn sessions_send_the_resume_token_and_learn_a_new_one() {
        let session = HostedSession::new(
            "ABCD2345".into(),
            Some("secret".into()),
            ServiceInfo::default(),
        );
        assert_eq!(
            session.socket_url("wss://h/ws?protocol=15"),
            "wss://h/ws?protocol=15&code=ABCD2345"
        );
        let join = ClientMessage::JoinRoom {
            invite_code: "ABCD2345".into(),
            name: "P".into(),
            media_sha256: String::new(),
            media_extension: String::new(),
            spectator: false,
            player_key: String::new(),
        };
        let json = session.first_message_json(&join).unwrap();
        assert!(json.contains(r#""resume_token":"secret""#), "{json}");
        assert!(!format!("{session:?}").contains("secret"));
        session.absorb_welcome(r#"{"type":"welcome","resume_token":"fresh"}"#);
        assert_eq!(session.token().as_deref(), Some("fresh"));
        let fresh = HostedSession::new("ABCD2345".into(), None, ServiceInfo::default());
        assert!(
            !fresh
                .first_message_json(&join)
                .unwrap()
                .contains("resume_token")
        );
    }
}
