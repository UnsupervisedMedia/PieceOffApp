//! PieceOff Online in the app: the Online / Local or LAN choice, hosted
//! detection, linking this computer, hosted create and join, the public
//! lobby and quick match. Every request runs on a worker thread and reports
//! back through `OnlineEvent`s drained once a frame by `poll_online`.

use super::*;
use crate::online::{
    CreateOutcome, DeviceKey, HostedSession, LinkPoll, LobbyRoom, ONLINE_SERVER_URL, ServiceInfo,
    ServiceKind,
};
use eframe::egui::RichText;

/// How often the open lobby list refreshes itself.
const LOBBY_REFRESH: Duration = Duration::from_secs(10);
/// How often a link code is checked for approval.
const LINK_POLL: Duration = Duration::from_secs(2);
/// An unreachable server is asked again after this long.
const UNREACHABLE_RETRY: Duration = Duration::from_secs(15);

pub(crate) enum OnlineEvent {
    Detected {
        url: String,
        result: Result<ServiceKind, String>,
    },
    Created {
        url: String,
        info: ServiceInfo,
        result: Result<CreateOutcome, String>,
    },
    Link(LinkEvent),
    Lobby {
        url: String,
        result: Result<Vec<LobbyRoom>, String>,
        quick_match: bool,
    },
}

pub(crate) enum LinkEvent {
    Started {
        code: String,
        url: String,
        expires_in: u64,
    },
    Approved(DeviceKey),
    Expired,
    Failed(String),
}

/// What the player asked for while the server is checked or this computer
/// is linked.
pub(crate) enum PendingAction {
    Create(ClientMessage),
    Join(ClientMessage),
}

pub(crate) enum LinkStage {
    Starting,
    Waiting {
        code: String,
        url: String,
        expires_at: Instant,
    },
    Expired,
    Failed(String),
}

pub(crate) struct LinkFlow {
    pub(crate) stage: LinkStage,
    /// Why linking is needed again, after the service refused the old key.
    pub(crate) reason: Option<&'static str>,
    cancel: Arc<AtomicBool>,
    url: String,
    /// The origin asked for the key; the key is stored under it alone.
    origin: String,
}

impl Drop for LinkFlow {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct LobbyView {
    pub(crate) rooms: Vec<LobbyRoom>,
    url: String,
    fetched_at: Option<Instant>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

pub(crate) struct OnlineState {
    /// What each room server turned out to be, for this session.
    services: HashMap<String, ServiceKind>,
    detecting: HashSet<String>,
    unreachable: HashMap<String, (String, Instant)>,
    tx: mpsc::Sender<OnlineEvent>,
    rx: mpsc::Receiver<OnlineEvent>,
    pending: Option<(String, PendingAction)>,
    /// A pending action is being checked or created.
    pub(crate) busy: bool,
    pub(crate) link: Option<LinkFlow>,
    pub(crate) lobby: LobbyView,
    /// The hosted room we are in (or joining), if any.
    pub(crate) session: Option<HostedSession>,
    /// The create card's "List publicly" box.
    pub(crate) list_publicly: bool,
    /// Quick match found no open room: create a public one once the random
    /// built-in loop has loaded.
    pub(crate) quick_match_create: bool,
    /// The recent room picked for rejoining: its code and resume token.
    pub(crate) recent_resume: Option<(String, String)>,
    /// Tests run stub servers without TLS; real builds never allow keys on
    /// a plaintext origin.
    #[cfg(test)]
    pub(crate) allow_plaintext_keys: bool,
}

impl Default for OnlineState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            services: HashMap::new(),
            detecting: HashSet::new(),
            unreachable: HashMap::new(),
            tx,
            rx,
            pending: None,
            busy: false,
            link: None,
            lobby: LobbyView::default(),
            session: None,
            list_publicly: false,
            quick_match_create: false,
            recent_resume: None,
            #[cfg(test)]
            allow_plaintext_keys: false,
        }
    }
}

impl OnlineState {
    #[cfg(test)]
    pub(crate) fn remember_service(&mut self, url: &str, kind: ServiceKind) {
        self.services.insert(url.to_owned(), kind);
    }
}

impl PieceOffApp {
    fn online_sender(&self) -> WakingSender<OnlineEvent> {
        WakingSender {
            tx: self.online.tx.clone(),
            repaint: self.repaint.clone(),
        }
    }

    /// The origin whose device key applies to `url`, if keys may be used there.
    pub(crate) fn device_key_origin(&self, url: &str) -> Result<String, String> {
        #[cfg(test)]
        let allow_plaintext = self.online.allow_plaintext_keys;
        #[cfg(not(test))]
        let allow_plaintext = false;
        crate::online::key_origin(url, allow_plaintext)
    }

    /// The selected server's origin has a device key.
    pub(crate) fn current_device_linked(&self) -> bool {
        self.resolved_server_url()
            .and_then(|url| self.device_key_origin(&url))
            .is_ok_and(|origin| self.settings.device_keys.contains_key(&origin))
    }

    /// Something online is in progress that a new create or join would race.
    pub(crate) fn online_busy(&self) -> bool {
        self.online.busy || self.online.link.is_some()
    }

    /// The selected server is PieceOff Online.
    pub(crate) fn online_selected(&self) -> bool {
        self.resolved_server_url().as_deref() == Ok(ONLINE_SERVER_URL)
    }

    /// What the selected server is, once known.
    pub(crate) fn current_service(&self) -> Option<&ServiceKind> {
        let url = self.resolved_server_url().ok()?;
        self.online.services.get(&url)
    }

    pub(crate) fn current_hosted(&self) -> Option<&ServiceInfo> {
        self.current_service().and_then(ServiceKind::hosted)
    }

    pub(crate) fn select_online_server(&mut self) {
        if self.online_selected() {
            return;
        }
        if let Ok(url) = self.resolved_server_url() {
            if url != ONLINE_SERVER_URL {
                self.settings.lan_server_url = Some(url);
            }
        }
        self.server_url = ONLINE_SERVER_URL.into();
        self.server_edit_dirty = false;
        self.server_url_source = "saved setting";
        self.settings.server_url = Some(ONLINE_SERVER_URL.into());
        self.persist_settings();
    }

    pub(crate) fn select_lan_server(&mut self) {
        if !self.online_selected() {
            return;
        }
        let url = self
            .settings
            .lan_server_url
            .clone()
            .unwrap_or_else(|| server_addr::LOCAL_SERVER_URL.into());
        self.server_url.clone_from(&url);
        self.server_edit_dirty = false;
        self.server_url_source = "saved setting";
        self.settings.server_url = Some(url);
        self.persist_settings();
    }

    /// Check the selected server once, in the background, so the start
    /// screen can offer the lobby and public rooms.
    pub(crate) fn ensure_service_check(&mut self) {
        if self.server_edit_dirty || self.snapshot.is_some() {
            return;
        }
        if let Ok(url) = self.resolved_server_url() {
            self.spawn_detect(&url);
        }
    }

    fn spawn_detect(&mut self, url: &str) {
        if self.online.services.contains_key(url) || self.online.detecting.contains(url) {
            return;
        }
        if let Some((_, when)) = self.online.unreachable.get(url) {
            if when.elapsed() < UNREACHABLE_RETRY {
                return;
            }
        }
        self.online.detecting.insert(url.to_owned());
        let url = url.to_owned();
        let tx = self.online_sender();
        thread::spawn(move || {
            let result = crate::online::detect_service(&url);
            let _ = tx.send(OnlineEvent::Detected { url, result });
        });
    }

    /// Forget that a server could not be reached, so it is asked again now.
    pub(crate) fn retry_service_check(&mut self) {
        if let Ok(url) = self.resolved_server_url() {
            self.online.unreachable.remove(&url);
            self.spawn_detect(&url);
        }
    }

    /// Create a room: over HTTP with this computer's key on a hosted
    /// server, through the socket on an axum server.
    pub(crate) fn begin_create(&mut self, message: ClientMessage) {
        let url = match self.resolved_server_url() {
            Ok(url) => url,
            Err(message) => {
                self.status = message;
                return;
            }
        };
        self.online.recent_resume = None;
        match self.online.services.get(&url).cloned() {
            Some(ServiceKind::SelfHosted) => {
                self.online.session = None;
                self.media_ready = false;
                self.media_uploading = true;
                self.connect(message);
            }
            Some(ServiceKind::Hosted(info)) => {
                if !info.uploads && !self.chosen_media_builtin() {
                    self.status = crate::online::BUILTIN_ONLY_MESSAGE.into();
                    return;
                }
                self.hosted_create(url, info, message)
            }
            None => self.wait_for_service(url, PendingAction::Create(message)),
        }
    }

    /// Join a room; a hosted server routes the socket by code and is sent
    /// the player's resume token when we have one.
    pub(crate) fn begin_join(&mut self, message: ClientMessage) {
        let url = match self.resolved_server_url() {
            Ok(url) => url,
            Err(message) => {
                self.status = message;
                return;
            }
        };
        match self.online.services.get(&url).cloned() {
            Some(ServiceKind::SelfHosted) => {
                self.online.session = None;
                self.connect(message);
            }
            Some(ServiceKind::Hosted(info)) => self.hosted_join(info, message),
            None => self.wait_for_service(url, PendingAction::Join(message)),
        }
    }

    fn wait_for_service(&mut self, url: String, action: PendingAction) {
        self.online.pending = Some((url.clone(), action));
        self.online.busy = true;
        self.online.unreachable.remove(&url);
        self.status = if url == ONLINE_SERVER_URL {
            "Connecting to PieceOff Online…".into()
        } else {
            "Checking the room server…".into()
        };
        self.spawn_detect(&url);
    }

    fn hosted_join(&mut self, info: ServiceInfo, message: ClientMessage) {
        if let Some(problem) = info.version_problem() {
            self.status = problem;
            return;
        }
        let ClientMessage::JoinRoom { invite_code, .. } = &message else {
            return;
        };
        let code = invite_code.trim().to_ascii_uppercase();
        if !crate::online::valid_room_code(&code) {
            self.status = "PieceOff Online room codes are 8 letters and digits.".into();
            return;
        }
        // Rejoining the room we were in keeps our identity; so does a
        // recent room picked from the list.
        let session = match self.online.session.take() {
            Some(session) if session.code == code => session,
            _ => {
                let token = self
                    .online
                    .recent_resume
                    .take()
                    .filter(|(recent, _)| *recent == code)
                    .map(|(_, token)| token);
                HostedSession::new(code.clone(), token, info)
            }
        };
        self.online.session = Some(session);
        let message = match message {
            ClientMessage::JoinRoom {
                name,
                media_sha256,
                media_extension,
                spectator,
                player_key,
                ..
            } => ClientMessage::JoinRoom {
                invite_code: code,
                name,
                media_sha256,
                media_extension,
                spectator,
                player_key,
            },
            other => other,
        };
        self.connect(message);
    }

    fn hosted_create(&mut self, url: String, info: ServiceInfo, message: ClientMessage) {
        if let Some(problem) = info.version_problem() {
            self.status = problem;
            return;
        }
        // A key goes only to the exact TLS origin that issued it.
        let origin = match self.device_key_origin(&url) {
            Ok(origin) => origin,
            Err(error) => {
                self.online.quick_match_create = false;
                self.media_uploading = false;
                self.status = error;
                return;
            }
        };
        let Some(key) = self.settings.device_keys.get(&origin).cloned() else {
            self.online.pending = Some((url.clone(), PendingAction::Create(message)));
            self.start_link(url, None);
            return;
        };
        self.online.pending = Some((url.clone(), PendingAction::Create(message.clone())));
        self.online.busy = true;
        self.status = "Creating your room on PieceOff Online…".into();
        let tx = self.online_sender();
        thread::spawn(move || {
            let result = crate::online::create_room(&origin, &key, &message);
            let _ = tx.send(OnlineEvent::Created { url, info, result });
        });
    }

    /// Show the link window and ask the service for a code to approve.
    pub(crate) fn start_link(&mut self, url: String, reason: Option<&'static str>) {
        // Keys come only from a TLS origin's own link flow, stored under it.
        let origin = match self.device_key_origin(&url) {
            Ok(origin) => origin,
            Err(error) => {
                self.online.pending = None;
                self.online.busy = false;
                self.online.quick_match_create = false;
                self.media_uploading = false;
                self.status = error;
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.online.link = Some(LinkFlow {
            stage: LinkStage::Starting,
            reason,
            cancel: cancel.clone(),
            url: url.clone(),
            origin: origin.clone(),
        });
        self.status = "Link this computer to PieceOff Online to create rooms.".into();
        let base = origin;
        let tx = self.online_sender();
        thread::spawn(move || run_link(&base, &cancel, &tx));
    }

    pub(crate) fn retry_link(&mut self) {
        if let Some(link) = self.online.link.take() {
            let (url, reason) = (link.url.clone(), link.reason);
            drop(link);
            self.start_link(url, reason);
        }
    }

    pub(crate) fn cancel_link(&mut self) {
        self.online.link = None;
        if matches!(self.online.pending, Some((_, PendingAction::Create(_)))) {
            self.online.pending = None;
            self.online.busy = false;
            self.media_uploading = false;
        }
        self.online.quick_match_create = false;
        self.status = "Linking cancelled. Joining rooms works without it.".into();
    }

    /// Forget this computer's key for the selected server.
    pub(crate) fn unlink_device(&mut self) {
        if let Ok(origin) = self
            .resolved_server_url()
            .and_then(|url| self.device_key_origin(&url))
        {
            self.settings.device_keys.remove(&origin);
        }
        self.persist_settings();
        self.status = "This computer is no longer linked to PieceOff Online.".into();
    }

    pub(crate) fn request_lobby(&mut self, quick_match: bool) {
        let Some(url) = self.resolved_server_url().ok() else {
            return;
        };
        if self.online.lobby.loading && !quick_match {
            return;
        }
        let Ok(base) = crate::online::http_base(&url) else {
            return;
        };
        if self.online.lobby.url != url {
            self.online.lobby = LobbyView {
                url: url.clone(),
                ..LobbyView::default()
            };
        }
        self.online.lobby.loading = true;
        let tx = self.online_sender();
        thread::spawn(move || {
            let result = crate::online::fetch_lobby(&base);
            let _ = tx.send(OnlineEvent::Lobby {
                url,
                result,
                quick_match,
            });
        });
    }

    /// Join the best open public room, or start one with a random built-in
    /// loop when none has a free seat.
    pub(crate) fn quick_match(&mut self) {
        self.status = "Looking for an open room…".into();
        self.online.busy = true;
        self.request_lobby(true);
    }

    pub(crate) fn join_listed(&mut self, code: &str, watch: bool) {
        self.invite_code = code.to_owned();
        self.watch_only = watch;
        self.join_room();
    }

    /// The join box was given an invite link: take its code and switch to
    /// PieceOff Online.
    pub(crate) fn absorb_invite_paste(&mut self) {
        if let Some(invite) = crate::online::parse_invite(&self.invite_code) {
            if invite.online {
                self.invite_code = invite.code;
                if self.network.is_none() {
                    self.select_online_server();
                }
                self.status = "Invite link read. Press Join to enter the room.".into();
            }
        }
    }

    /// Why the create card cannot list this room publicly, if it cannot.
    pub(crate) fn listing_block_reason(&self) -> Option<&'static str> {
        let Some(info) = self.current_hosted() else {
            return Some("Public rooms are only on PieceOff Online.");
        };
        if !info.has_feature("lobby") {
            return Some("PieceOff Online has no public lobby yet.");
        }
        if !self.chosen_media_builtin() {
            return Some(
                "Public rooms use built-in puzzles only. Pick one of the built-in puzzles to list this room.",
            );
        }
        None
    }

    /// The server is hosted and takes no uploads: only built-in loops.
    pub(crate) fn custom_media_blocked(&self) -> bool {
        let info = if self.snapshot.is_some() {
            self.online.session.as_ref().map(|session| &session.info)
        } else {
            self.current_hosted()
        };
        info.is_some_and(|info| !info.uploads)
    }

    /// The chosen media (and any package queue) is all built-in.
    pub(crate) fn chosen_media_builtin(&self) -> bool {
        self.media_path.is_some()
            && pieceoff_core::is_builtin_media(&self.media_hash)
            && self.pending_snapshot.as_ref().is_none_or(|snapshot| {
                snapshot
                    .queue
                    .iter()
                    .all(|item| pieceoff_core::is_builtin_media(&item.sha256))
            })
    }

    pub(crate) fn wants_listing(&self) -> bool {
        self.online.list_publicly && self.listing_block_reason().is_none()
    }

    /// Drain finished online requests.
    pub(crate) fn poll_online(&mut self, ctx: &egui::Context) {
        let events: Vec<OnlineEvent> = self.online.rx.try_iter().collect();
        for event in events {
            match event {
                OnlineEvent::Detected { url, result } => self.on_detected(url, result),
                OnlineEvent::Created { url, info, result } => self.on_created(url, info, result),
                OnlineEvent::Link(event) => self.on_link(event),
                OnlineEvent::Lobby {
                    url,
                    result,
                    quick_match,
                } => self.on_lobby(url, result, quick_match, ctx),
            }
        }
        if let Some(link) = &self.online.link {
            if let LinkStage::Waiting { .. } | LinkStage::Starting = link.stage {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    fn on_detected(&mut self, url: String, result: Result<ServiceKind, String>) {
        self.online.detecting.remove(&url);
        // PieceOff Online that does not answer as itself is down or not yet
        // deployed, never an axum server: report it and ask again later.
        let result = match result {
            Ok(ServiceKind::SelfHosted) if url == ONLINE_SERVER_URL => {
                Err(crate::online::UNAVAILABLE_MESSAGE.to_owned())
            }
            other => other,
        };
        let waiting = self
            .online
            .pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == url);
        match result {
            Ok(kind) => {
                self.online.services.insert(url.clone(), kind);
                if waiting {
                    let Some((_, action)) = self.online.pending.take() else {
                        return;
                    };
                    self.online.busy = false;
                    match action {
                        PendingAction::Create(message) => self.begin_create(message),
                        PendingAction::Join(message) => self.begin_join(message),
                    }
                }
            }
            Err(error) => {
                self.online
                    .unreachable
                    .insert(url.clone(), (error.clone(), Instant::now()));
                if !waiting {
                    return;
                }
                let Some((_, action)) = self.online.pending.take() else {
                    return;
                };
                self.online.busy = false;
                if url == ONLINE_SERVER_URL {
                    self.media_uploading = false;
                    self.online.quick_match_create = false;
                    self.status = error;
                    return;
                }
                // Not reachable over HTTP: let the socket try, as before,
                // so the player gets the usual connection message.
                self.online.session = None;
                if let PendingAction::Create(_) = action {
                    self.media_ready = false;
                    self.media_uploading = true;
                }
                match action {
                    PendingAction::Create(message) | PendingAction::Join(message) => {
                        self.connect(message)
                    }
                }
            }
        }
    }

    fn on_created(
        &mut self,
        url: String,
        info: ServiceInfo,
        result: Result<CreateOutcome, String>,
    ) {
        self.online.busy = false;
        let Some((_, PendingAction::Create(_))) = &self.online.pending else {
            return;
        };
        match result {
            Ok(CreateOutcome::Created(room)) => {
                self.online.pending = None;
                let message = ClientMessage::JoinRoom {
                    invite_code: room.code.clone(),
                    name: self.player_name.clone(),
                    media_sha256: self.media_hash.clone(),
                    media_extension: self.media_extension.clone(),
                    spectator: false,
                    player_key: self.settings.player_key.clone(),
                };
                self.invite_code.clone_from(&room.code);
                self.watch_only = false;
                self.online.session =
                    Some(HostedSession::new(room.code, Some(room.resume_token), info));
                self.media_ready = false;
                self.media_uploading = true;
                self.connect(message);
            }
            Ok(CreateOutcome::Relink) => {
                if let Ok(origin) = self.device_key_origin(&url) {
                    self.settings.device_keys.remove(&origin);
                }
                self.persist_settings();
                self.start_link(
                    url,
                    Some("This computer's link to PieceOff Online was revoked or has expired. Link it again to create rooms."),
                );
            }
            Err(error) => {
                self.online.pending = None;
                self.online.quick_match_create = false;
                self.media_uploading = false;
                self.status = format!("Could not create the room: {error}");
            }
        }
    }

    fn on_link(&mut self, event: LinkEvent) {
        let Some(link) = &mut self.online.link else {
            return;
        };
        match event {
            LinkEvent::Started {
                code,
                url,
                expires_in,
            } => {
                link.stage = LinkStage::Waiting {
                    code,
                    url,
                    expires_at: Instant::now() + Duration::from_secs(expires_in),
                };
            }
            LinkEvent::Expired => link.stage = LinkStage::Expired,
            LinkEvent::Failed(error) => link.stage = LinkStage::Failed(error),
            LinkEvent::Approved(key) => {
                let origin = link.origin.clone();
                self.online.link = None;
                self.settings.device_keys.insert(origin, key);
                self.persist_settings();
                self.status = "This computer is linked to PieceOff Online.".into();
                if let Some((url, PendingAction::Create(message))) = self.online.pending.take() {
                    match self.online.services.get(&url).cloned() {
                        Some(ServiceKind::Hosted(info)) => self.hosted_create(url, info, message),
                        _ => self.begin_create(message),
                    }
                }
            }
        }
    }

    fn on_lobby(
        &mut self,
        url: String,
        result: Result<Vec<LobbyRoom>, String>,
        quick_match: bool,
        ctx: &egui::Context,
    ) {
        if self.online.lobby.url == url {
            self.online.lobby.loading = false;
            self.online.lobby.fetched_at = Some(Instant::now());
            match &result {
                Ok(rooms) => {
                    self.online.lobby.rooms.clone_from(rooms);
                    self.online.lobby.error = None;
                }
                Err(error) => self.online.lobby.error = Some(error.clone()),
            }
        }
        if !quick_match {
            return;
        }
        self.online.busy = false;
        if self.network.is_some() || self.snapshot.is_some() {
            return;
        }
        match result {
            Ok(rooms) => match crate::online::quick_match_pick(&rooms) {
                Some(room) => {
                    let code = room.code.clone();
                    self.status = format!("Joining {} …", room.title);
                    self.join_listed(&code, false);
                }
                None => self.quick_match_start_room(ctx),
            },
            Err(error) => self.status = format!("Quick match failed: {error}"),
        }
    }

    /// Nobody has a free seat: load a random built-in loop, then create a
    /// public room with it (see `poll_probe`).
    fn quick_match_start_room(&mut self, ctx: &egui::Context) {
        let mut byte = [0u8; 1];
        let _ = getrandom::fill(&mut byte);
        let (title, _, filename, bytes) =
            crate::library_ui::BUILT_INS[usize::from(byte[0]) % crate::library_ui::BUILT_INS.len()];
        self.online.quick_match_create = true;
        self.online.list_publicly = true;
        self.pending_snapshot = None;
        self.status = format!("No open rooms right now. Starting a public {title} room…");
        self.open_builtin(title, filename, bytes, ctx);
    }

    /// The loop picked by quick match has loaded: create its public room.
    pub(crate) fn finish_quick_match(&mut self) {
        if !std::mem::take(&mut self.online.quick_match_create) {
            return;
        }
        let aspect = self.media_size.0 as f32 / self.media_size.1.max(1) as f32;
        (self.rows, self.columns) = grid_for_style(48, aspect, self.cut_style);
        self.online.list_publicly = true;
        self.create_room();
    }

    // ---- UI ---------------------------------------------------------------

    /// Where rooms are hosted: PieceOff Online, or a local or LAN server.
    pub(crate) fn server_card(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let idle = self.network.is_none() && !self.online_busy();
        card(ui, language.text("Room server"), |ui| {
            let online = self.online_selected();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::selectable(online, language.text("Online")),
                    )
                    .on_hover_text(
                        language.text("Play on PieceOff Online with anyone, no server needed"),
                    )
                    .clicked()
                {
                    self.select_online_server();
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::selectable(!online, language.text("Local or LAN")),
                    )
                    .on_hover_text(
                        language.text("Use a room server on this computer or your network"),
                    )
                    .clicked()
                {
                    self.select_lan_server();
                }
            });
            if online {
                self.online_status_lines(ui);
            } else {
                self.lan_server_field(ui, idle);
            }
        });
    }

    fn online_status_lines(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let url = self.resolved_server_url().unwrap_or_default();
        match self.online.services.get(&url) {
            Some(ServiceKind::Hosted(info)) => {
                if let Some(problem) = info.version_problem() {
                    ui.colored_label(WARNING, problem);
                } else if self.current_device_linked() {
                    ui.horizontal_wrapped(|ui| {
                        status_dot(ui, SUCCESS);
                        ui.weak(language.text("PieceOff Online · this computer is linked"));
                        if ui
                            .small_button(language.text("Unlink"))
                            .on_hover_text(
                                language.text("Forget this computer's PieceOff Online key"),
                            )
                            .clicked()
                        {
                            self.unlink_device();
                        }
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        status_dot(ui, SUCCESS);
                        ui.weak(language.text("PieceOff Online is ready"));
                    });
                    ui.weak(language.text(
                        "Creating your first room links this computer once, in your browser. Joining never needs it.",
                    ));
                }
            }
            Some(ServiceKind::SelfHosted) => {
                ui.colored_label(
                    WARNING,
                    language.text("rooms.pieceoff.app did not answer as PieceOff Online."),
                );
            }
            None => {
                if let Some((error, _)) = self.online.unreachable.get(&url) {
                    let shown = [
                        crate::online::OFFLINE_MESSAGE,
                        crate::online::UNAVAILABLE_MESSAGE,
                    ]
                    .into_iter()
                    .find(|known| known == error)
                    .map_or(error.as_str(), |known| language.text(known));
                    ui.colored_label(WARNING, shown);
                    if ui.small_button(language.text("Try again")).clicked() {
                        self.retry_service_check();
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(language.text("Checking PieceOff Online…"));
                    });
                }
            }
        }
    }

    fn lan_server_field(&mut self, ui: &mut egui::Ui, idle: bool) {
        let language = self.settings.language;
        ui.horizontal(|ui| {
            let field = ui
                .add_enabled(
                    idle,
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
                .add_enabled(idle, egui::Button::new(language.text("Reset")))
                .on_hover_text(language.text("Use the server on this device"))
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
        if self.current_hosted().is_some() {
            ui.weak(language.text("This is a PieceOff Online server."));
        }
    }

    /// The create card's public listing box.
    pub(crate) fn list_publicly_checkbox(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let blocked = self.listing_block_reason();
        let mut checked = self.online.list_publicly && blocked.is_none();
        let response = ui.add_enabled(
            blocked.is_none(),
            egui::Checkbox::new(&mut checked, language.text("List publicly")),
        );
        let response = match blocked {
            Some(reason) => response.on_disabled_hover_text(language.text(reason)),
            None => response.on_hover_text(language.text(
                "Anyone can find this room in Online rooms and join it. Invite codes still work.",
            )),
        };
        if response.changed() {
            self.online.list_publicly = checked;
        }
    }

    /// The start screen's public lobby: open rooms on PieceOff Online.
    pub(crate) fn online_rooms_card(&mut self, ui: &mut egui::Ui) {
        let Some(info) = self.current_hosted() else {
            return;
        };
        if !info.has_feature("lobby") || info.version_problem().is_some() {
            return;
        }
        let url = self.resolved_server_url().unwrap_or_default();
        let stale = self.online.lobby.url != url
            || self
                .online
                .lobby
                .fetched_at
                .is_none_or(|at| at.elapsed() >= LOBBY_REFRESH);
        if stale && !self.online.lobby.loading {
            self.request_lobby(false);
        }
        ui.ctx().request_repaint_after(LOBBY_REFRESH);
        let language = self.settings.language;
        let idle = self.network.is_none() && !self.online_busy() && self.probe_rx.is_none();
        card(ui, language.text("Online rooms"), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(
                            RichText::new(language.text("Quick match"))
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(ACCENT_FILL),
                    )
                    .on_hover_text(
                        language
                            .text("Join an open public room, or start one with a built-in puzzle"),
                    )
                    .clicked()
                {
                    self.quick_match();
                }
                if ui
                    .add_enabled(
                        !self.online.lobby.loading,
                        egui::Button::new(language.text("Refresh")),
                    )
                    .clicked()
                {
                    self.request_lobby(false);
                }
                if self.online.lobby.loading {
                    ui.spinner();
                }
            });
            if let Some(error) = &self.online.lobby.error {
                ui.colored_label(WARNING, error.as_str());
            } else if self.online.lobby.rooms.is_empty() && self.online.lobby.fetched_at.is_some() {
                ui.weak(language.text("No public rooms right now. Quick match starts one."));
            }
            let mut chosen = None;
            egui::ScrollArea::vertical()
                .id_salt("online-rooms")
                .max_height(260.0)
                .show(ui, |ui| {
                    for room in &self.online.lobby.rooms {
                        ui.separator();
                        ui.label(RichText::new(&room.title).strong());
                        let mut details = format!(
                            "{} · {}/{} · {}% · {}",
                            if room.host_name.is_empty() {
                                "?"
                            } else {
                                room.host_name.as_str()
                            },
                            room.players,
                            room.max_players,
                            room.progress_percent(),
                            room.cut_label()
                        );
                        if room.spectators > 0 {
                            details.push_str(&format!(" · {} 👁", room.spectators));
                        }
                        ui.weak(details);
                        ui.horizontal(|ui| {
                            let join = ui
                                .add_enabled(
                                    idle && room.has_free_slot(),
                                    egui::Button::new(language.text("Join")),
                                )
                                .on_disabled_hover_text(
                                    language.text("This room is full; you can still watch"),
                                );
                            if join.clicked() {
                                chosen = Some((room.code.clone(), false));
                            }
                            if ui
                                .add_enabled(idle, egui::Button::new(language.text("Watch")))
                                .on_hover_text(language.text("Watch without playing"))
                                .clicked()
                            {
                                chosen = Some((room.code.clone(), true));
                            }
                        });
                    }
                });
            if let Some((code, watch)) = chosen {
                self.join_listed(&code, watch);
            }
        });
        ui.add_space(8.0);
    }

    /// The device link window, while linking.
    pub(crate) fn link_window(&mut self, ctx: &egui::Context) {
        let Some(link) = &self.online.link else {
            return;
        };
        let language = self.settings.language;
        enum Choice {
            Open(String),
            Copy(String),
            Retry,
            Cancel,
        }
        let mut choice = None;
        let reason = link.reason;
        egui::Window::new(language.text("Link this computer to PieceOff Online"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                if let Some(reason) = reason {
                    ui.colored_label(WARNING, language.text(reason));
                    ui.add_space(4.0);
                }
                ui.label(language.text(
                    "To keep rooms free of spam, each computer passes a quick check in your browser once before it creates rooms. Joining rooms never needs this.",
                ));
                ui.add_space(6.0);
                match &link.stage {
                    LinkStage::Starting => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(language.text("Getting a link code…"));
                        });
                    }
                    LinkStage::Waiting {
                        code,
                        url,
                        expires_at,
                    } => {
                        ui.weak(language.text("Check that the browser shows this code:"));
                        ui.label(RichText::new(code).monospace().size(28.0).strong().color(ACCENT));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(language.text("Open browser"))
                                            .strong()
                                            .color(Color32::WHITE),
                                    )
                                    .fill(ACCENT_FILL),
                                )
                                .clicked()
                            {
                                choice = Some(Choice::Open(url.clone()));
                            }
                            if ui.button(language.text("Copy link")).clicked() {
                                choice = Some(Choice::Copy(url.clone()));
                            }
                        });
                        let left = expires_at.saturating_duration_since(Instant::now()).as_secs();
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(format!(
                                "{} ({}:{:02})",
                                language.text("Waiting for approval in the browser…"),
                                left / 60,
                                left % 60
                            ));
                        });
                    }
                    LinkStage::Expired => {
                        ui.colored_label(WARNING, language.text("That link code expired before it was approved."));
                        if ui.button(language.text("Get a new code")).clicked() {
                            choice = Some(Choice::Retry);
                        }
                    }
                    LinkStage::Failed(error) => {
                        ui.colored_label(WARNING, error.as_str());
                        if ui.button(language.text("Try again")).clicked() {
                            choice = Some(Choice::Retry);
                        }
                    }
                }
                ui.add_space(6.0);
                if ui.button(language.text("Cancel")).clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        match choice {
            Some(Choice::Open(url)) => {
                ctx.open_url(egui::OpenUrl::new_tab(url));
                self.status = "Approve the code in your browser, then come back here.".into();
            }
            Some(Choice::Copy(url)) => {
                ctx.copy_text(url);
                self.status =
                    "Link copied. Open it in your browser to approve this computer.".into();
            }
            Some(Choice::Retry) => self.retry_link(),
            Some(Choice::Cancel) => self.cancel_link(),
            None => {}
        }
    }

    /// The room toolbar's "Copy invite link", on hosted rooms.
    pub(crate) fn invite_link_button(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        code: &str,
    ) {
        let Some(session) = &self.online.session else {
            return;
        };
        let link = crate::online::invite_link(session.info.share_url(), code);
        let language = self.settings.language;
        if ui
            .button(language.text("Copy invite link"))
            .on_hover_text(link.as_str())
            .clicked()
        {
            ctx.copy_text(link);
            self.status = "Invite link copied. Anyone with PieceOff can open it to join.".into();
        }
    }

    /// The queue window's add control on a hosted server without uploads:
    /// a menu of built-in loops instead of a file picker.
    pub(crate) fn add_builtin_menu(&mut self, ui: &mut egui::Ui, can_edit: bool) {
        let language = self.settings.language;
        let mut chosen = None;
        ui.add_enabled_ui(can_edit, |ui| {
            ui.menu_button(language.text("Add built-in puzzle…"), |ui| {
                for (title, description, filename, bytes) in crate::library_ui::BUILT_INS {
                    if ui.button(title).on_hover_text(description).clicked() {
                        chosen = Some((filename, bytes));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(language.text(crate::online::BUILTIN_ONLY_MESSAGE));
        });
        if let Some((filename, bytes)) = chosen {
            match self.builtin_media_file(filename, bytes) {
                Ok(path) => self.send_queue_add_from_file(path),
                Err(error) => self.status = format!("Could not prepare built-in puzzle: {error}"),
            }
        }
    }

    /// The host's "Listed publicly" switch in the More menu, on hosted rooms.
    pub(crate) fn listed_toggle(&mut self, ui: &mut egui::Ui, is_host: bool) {
        let Some(session) = &self.online.session else {
            return;
        };
        if !is_host || self.spectating || !session.info.has_feature("lobby") {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let language = self.settings.language;
        let allowed = snapshot.only_builtin_media();
        let mut listed = snapshot.listed;
        let response = ui.add_enabled(
            allowed || listed,
            egui::Checkbox::new(&mut listed, language.text("Listed publicly")),
        );
        let response = if allowed || listed {
            response.on_hover_text(language.text(
                "Show this room in Online rooms so anyone can join. Invite codes still work.",
            ))
        } else {
            response.on_disabled_hover_text(language.text(
                "Public rooms use built-in puzzles only. Remove other videos from the queue to list this room.",
            ))
        };
        if response.changed() {
            self.send(ClientMessage::SetListed { listed });
            self.status = if listed {
                "This room is now listed in Online rooms.".into()
            } else {
                "This room is no longer listed publicly.".into()
            };
        }
    }
}

/// The link worker: ask for a code, then poll until it is approved, expires,
/// or the window is closed.
fn run_link(base: &str, cancel: &AtomicBool, tx: &WakingSender<OnlineEvent>) {
    let start = match crate::online::start_link(base) {
        Ok(start) => start,
        Err(error) => {
            let _ = tx.send(OnlineEvent::Link(LinkEvent::Failed(error)));
            return;
        }
    };
    let _ = tx.send(OnlineEvent::Link(LinkEvent::Started {
        code: start.link_code.clone(),
        url: start.url.clone(),
        expires_in: start.expires_in,
    }));
    let deadline = Instant::now() + Duration::from_secs(start.expires_in);
    let mut failures = 0;
    loop {
        let wake = Instant::now() + LINK_POLL;
        while Instant::now() < wake {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if Instant::now() >= deadline {
            let _ = tx.send(OnlineEvent::Link(LinkEvent::Expired));
            return;
        }
        match crate::online::poll_link(base, &start) {
            Ok(LinkPoll::Pending) => failures = 0,
            Ok(LinkPoll::Approved(key)) => {
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(OnlineEvent::Link(LinkEvent::Approved(key)));
                }
                return;
            }
            Ok(LinkPoll::Expired) => {
                let _ = tx.send(OnlineEvent::Link(LinkEvent::Expired));
                return;
            }
            // A dropped poll or two is not worth bothering the player with.
            Err(error) => {
                failures += 1;
                if failures >= 5 {
                    let _ = tx.send(OnlineEvent::Link(LinkEvent::Failed(error)));
                    return;
                }
            }
        }
    }
}
