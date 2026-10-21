use super::*;

/// A channel sender that also wakes the UI. eframe only repaints on input, so
/// without this a guest would not see remote moves or a finished download
/// until the mouse moved.
pub(crate) struct WakingSender<T> {
    pub(crate) tx: mpsc::Sender<T>,
    pub(crate) repaint: Option<egui::Context>,
}

impl<T> Clone for WakingSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            repaint: self.repaint.clone(),
        }
    }
}

impl<T> WakingSender<T> {
    pub(crate) fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let result = self.tx.send(value);
        if let Some(repaint) = &self.repaint {
            repaint.request_repaint();
        }
        result
    }
}

pub(crate) struct Network {
    pub(crate) outgoing: tokio_mpsc::UnboundedSender<ClientMessage>,
    pub(crate) incoming: mpsc::Receiver<ServerMessage>,
}

impl PieceOffApp {
    pub(crate) fn connect(&mut self, first: ClientMessage) {
        self.own_join_pending = matches!(first, ClientMessage::JoinRoom { .. });
        let url = self.resolved_server_url();
        if let Ok(resolved) = &url {
            self.server_url.clone_from(resolved);
        }
        let (incoming_tx, incoming) = mpsc::channel();
        let incoming_tx = WakingSender {
            tx: incoming_tx,
            repaint: self.repaint.clone(),
        };
        let media_files = self.media_files.clone();
        let (outgoing, outgoing_rx) = tokio_mpsc::unbounded_channel();
        let _ = outgoing.send(first);
        self.network = Some(Network { outgoing, incoming });
        self.status = "Connecting to room server…".into();
        thread::spawn(move || {
            let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
                let _ = incoming_tx.send(ServerMessage::Error {
                    message: "Could not start network runtime".into(),
                });
                return;
            };
            runtime.block_on(async move {
                let url = match url {
                    Ok(url) => url,
                    Err(message) => {
                        let _ = incoming_tx.send(ServerMessage::Error { message });
                        return;
                    }
                };
                let Ok((stream, _)) = connect_async(&url).await else { let _ = incoming_tx.send(ServerMessage::Error { message: format!("Could not connect to {url}. Check the address and that the server is running.") }); return; };
                let (mut writer, mut reader) = stream.split();
                let mut outgoing_rx = outgoing_rx;
                let mut media_transfer_started = false;
                let mut connected_client_id: Option<String> = None;
                let mut connected_media_hash: Option<String> = None;
                loop {
                    tokio::select! {
                        next = outgoing_rx.recv() => {
                            let Some(packet) = next else { break };
                            let Ok(text) = serde_json::to_string(&packet) else { continue };
                            if writer.send(Message::Text(text.into())).await.is_err() { break; }
                        }
                        next = reader.next() => match next {
                            Some(Ok(Message::Text(text))) => match serde_json::from_str::<ServerMessage>(&text) {
                                Ok(packet) => {
                                    if let ServerMessage::Welcome { client_id, snapshot, .. } = &packet {
                                        connected_client_id = Some(client_id.clone());
                                        connected_media_hash = Some(snapshot.media_sha256.clone());
                                    }
                                    // The host uploads every queue item the room is
                                    // still missing, from the files it knows about.
                                    if !media_transfer_started {
                                        if let ServerMessage::Welcome { snapshot, media_upload_token: Some(token), .. } = &packet {
                                            media_transfer_started = true;
                                            let pending = match media_files.lock() {
                                                Ok(files) => pending_uploads(snapshot, &files),
                                                Err(_) => Vec::new(),
                                            };
                                            spawn_item_uploads(&url, token, pending, &incoming_tx);
                                        }
                                    }
                                    if let ServerMessage::MediaUploadToken { client_id, token, sha256 } = &packet {
                                        if connected_client_id.as_deref() == Some(client_id.as_str()) {
                                            let target = match media_files.lock() {
                                                Ok(files) => token_upload(sha256.as_deref(), connected_media_hash.as_deref(), &files),
                                                Err(_) => None,
                                            };
                                            if let Some(target) = target {
                                                spawn_item_uploads(&url, token, vec![target], &incoming_tx);
                                            }
                                        }
                                    }
                                    if incoming_tx.send(packet).is_err() { break }
                                },
                                Err(error) => { let _ = incoming_tx.send(ServerMessage::Error { message: format!("Invalid server packet: {error}") }); }
                            },
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            _ => {}
                        }
                    }
                }
                let _ = incoming_tx.send(ServerMessage::Error { message: "Room server connection closed.".into() });
            });
        });
    }

    /// Download queue items we do not have yet: the current one first, then
    /// the following ones in play order so the next video is ready in time.
    pub(crate) fn fetch_missing_items(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let code = snapshot.invite_code.clone();
        let count = snapshot.queue.len();
        let wanted: Vec<(String, String)> = {
            let files = self.media_files.lock().unwrap();
            (0..count)
                .map(|offset| &snapshot.queue[(snapshot.current + offset) % count])
                .filter(|item| {
                    item.available
                        && !files.contains_key(&item.sha256)
                        && !self.downloads_in_flight.contains(&item.sha256)
                })
                .map(|item| (item.sha256.clone(), item.extension.clone()))
                .collect()
        };
        if wanted.is_empty() {
            return;
        }
        for (sha256, _) in &wanted {
            self.downloads_in_flight.insert(sha256.clone());
        }
        let Ok(url) = self.resolved_server_url() else {
            return;
        };
        let tx = WakingSender {
            tx: self.item_download_tx.clone(),
            repaint: self.repaint.clone(),
        };
        thread::spawn(move || {
            for (sha256, extension) in wanted {
                let result = download_room_media_retrying(&url, &code, &sha256, &extension);
                let _ = tx.send((sha256, result));
            }
        });
    }

    pub(crate) fn send(&mut self, packet: ClientMessage) {
        if let Some(network) = &self.network {
            if network.outgoing.send(packet).is_err() {
                self.status = "Connection to the room server closed.".into();
            }
        }
    }

    pub(crate) fn process_network(&mut self, ctx: &egui::Context) {
        let packets: Vec<ServerMessage> = self
            .network
            .as_ref()
            .map(|n| n.incoming.try_iter().collect())
            .unwrap_or_default();
        let finished: Vec<(String, Result<PathBuf, String>)> =
            self.item_download_rx.try_iter().collect();
        for (sha256, result) in finished {
            self.downloads_in_flight.remove(&sha256);
            match result {
                Ok(path) => {
                    self.media_files
                        .lock()
                        .unwrap()
                        .insert(sha256.clone(), path);
                    let is_current = self
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.media_sha256 == sha256);
                    if is_current && self.media_hash != sha256 {
                        self.switch_to_current_item(ctx);
                        if self.media_hash == sha256 {
                            self.status = "Room media downloaded and ready.".into();
                        }
                    }
                }
                Err(error) => {
                    self.status = format!("Could not download room media: {error}");
                }
            }
        }
        for packet in packets {
            match packet {
                ServerMessage::Welcome {
                    client_id,
                    name,
                    peers,
                    snapshot,
                    media_upload_token,
                } => {
                    if !valid_media_dimensions(snapshot.width, snapshot.height) {
                        self.network = None;
                        self.connected = false;
                        self.status = "Room media dimensions exceed this client's limits.".into();
                        continue;
                    }
                    self.clear_active_gesture();
                    if self
                        .snapshot
                        .as_ref()
                        .is_some_and(|current| current.id != snapshot.id)
                    {
                        self.selected_pieces.clear();
                        self.show_reference = false;
                        self.chat.reset();
                    }
                    self.client_id = Some(client_id);
                    self.peers.clone_from(&peers);
                    self.cursors.reset();
                    // The server may have tidied the name we asked for.
                    self.player_name.clone_from(&name);
                    // The server is the player's explicit default now; joining
                    // someone else's room must not replace it.
                    if self.settings.player_name.as_deref() != Some(name.as_str()) {
                        self.settings.player_name = Some(name.clone());
                        self.persist_settings();
                    }
                    self.reconnect_at = None;
                    self.reconnect_attempt = 0;
                    self.room_gone = false;
                    self.invite_code = snapshot.invite_code.clone();
                    self.title = snapshot.title.clone();
                    self.media_ready =
                        self.media_path.is_some() && self.media_hash == snapshot.media_sha256;
                    let is_host = media_upload_token.is_some();
                    self.media_uploading = is_host;
                    let downloading_media =
                        snapshot.media_available && self.media_hash != snapshot.media_sha256;
                    if downloading_media {
                        self.texture = None;
                        self.last_video_frame = None;
                        self.still_assist_texture = None;
                    }
                    self.snapshot = Some(snapshot);
                    self.connected = true;
                    self.status = if is_host {
                        "Room created. Uploading media before sharing the invite code…".into()
                    } else if downloading_media {
                        "Joined room. Downloading its media…".into()
                    } else if peers.is_empty() {
                        format!("Room {} is ready.", self.invite_code)
                    } else {
                        format!("Joined as {name}. In the room: {}", peers.join(", "))
                    };
                    self.fetch_missing_items();
                    let cached = self.snapshot.as_ref().is_some_and(|snapshot| {
                        self.media_files
                            .lock()
                            .unwrap()
                            .contains_key(&snapshot.media_sha256)
                    });
                    if cached {
                        self.switch_to_current_item(ctx);
                    }
                    if let Some(snapshot) = &self.snapshot {
                        let recent = RecentRoom {
                            title: snapshot.title.clone(),
                            server_url: self.server_url.clone(),
                            invite_code: snapshot.invite_code.clone(),
                            player_name: name,
                            last_joined_ms: SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        if let Err(error) = remember_recent_room(
                            &self.recent_rooms_path,
                            &mut self.recent_rooms,
                            recent,
                        ) {
                            self.status =
                                format!("{} Could not save recent room: {error}", self.status);
                        } else {
                            self.recent_room_selection = Some(0);
                        }
                    }
                    self.sync_decoder_playback(ctx);
                }
                ServerMessage::Snapshot { snapshot } => {
                    if !valid_media_dimensions(snapshot.width, snapshot.height) {
                        self.network = None;
                        self.connected = false;
                        self.status = "Room media dimensions exceed this client's limits.".into();
                        continue;
                    }
                    if self
                        .snapshot
                        .as_ref()
                        .is_some_and(|current| snapshot.revision < current.revision)
                    {
                        continue;
                    }
                    self.media_ready =
                        self.media_path.is_some() && self.media_hash == snapshot.media_sha256;
                    let all_uploaded = snapshot.queue.iter().all(|item| item.available);
                    // Only the moment uploading finishes is news; every later
                    // snapshot (including the one that follows an error) must
                    // not replace what the player is reading.
                    if self.media_uploading && snapshot.media_available && all_uploaded {
                        self.media_uploading = false;
                        self.status = format!(
                            "Room media is ready. Share invite code {}.",
                            snapshot.invite_code
                        );
                    }
                    let item_changed = self
                        .snapshot
                        .as_ref()
                        .is_none_or(|current| current.media_sha256 != snapshot.media_sha256);
                    self.snapshot = Some(snapshot);
                    if item_changed {
                        self.switch_to_current_item(ctx);
                    }
                    self.fetch_missing_items();
                    self.sync_decoder_playback(ctx);
                }
                ServerMessage::PeerJoined { name } => {
                    if std::mem::take(&mut self.own_join_pending) && name == self.player_name {
                        continue;
                    }
                    self.status = format!("{name} joined the puzzle.");
                    self.chat.note(format!("{name} joined"));
                    self.peers.push(name);
                }
                ServerMessage::PeerLeft { name } => {
                    self.status = format!("{name} left. Their pieces are free to move.");
                    self.chat.note(format!("{name} left"));
                    if let Some(index) = self.peers.iter().position(|peer| *peer == name) {
                        self.peers.remove(index);
                    }
                }
                ServerMessage::PiecesChanged { pieces, revision } => {
                    let is_host = self.is_host();
                    let mut restarted = false;
                    if let Some(snapshot) = &mut self.snapshot {
                        if revision >= snapshot.revision {
                            let had_progress = snapshot.progress_percent() > 0;
                            snapshot.revision = revision;
                            let everything = pieces.len() == snapshot.pieces.len();
                            for piece in pieces {
                                if let Some(existing) = snapshot.piece_mut(piece.id) {
                                    *existing = piece;
                                }
                            }
                            restarted =
                                everything && had_progress && snapshot.progress_percent() == 0;
                        }
                    }
                    if restarted {
                        self.chat.note("The puzzle was restarted");
                    }
                    if restarted && !is_host {
                        self.status = "The host restarted the puzzle.".into();
                    }
                }
                ServerMessage::PlaybackChanged { playback, revision } => {
                    if let Some(snapshot) = &mut self.snapshot {
                        if revision >= snapshot.revision {
                            snapshot.revision = revision;
                            snapshot.playback = playback;
                        }
                    }
                    self.sync_decoder_playback(ctx);
                }
                ServerMessage::MediaUploadToken {
                    client_id, sha256, ..
                } if self.client_id.as_deref() == Some(client_id.as_str()) => {
                    self.media_uploading = true;
                    // A token for a video we just queued is not a host change.
                    let queued_by_us = sha256.as_ref().is_some_and(|sha256| {
                        self.media_files.lock().unwrap().contains_key(sha256)
                            && self
                                .snapshot
                                .as_ref()
                                .is_some_and(|snapshot| snapshot.host_id == client_id)
                    });
                    self.status = if queued_by_us {
                        "Uploading the new video…".into()
                    } else {
                        "Host changed. Resuming the room media upload…".into()
                    };
                }
                ServerMessage::MediaUploadToken { .. } => {}
                ServerMessage::Cursor {
                    client_id,
                    name,
                    x,
                    y,
                } => {
                    // The server never echoes our own, but be sure.
                    if self.client_id.as_deref() != Some(client_id.as_str()) {
                        let now = ctx.input(|input| input.time);
                        self.cursors.update(client_id, name, x, y, now);
                    }
                }
                ServerMessage::CursorHidden { client_id } => self.cursors.hide(&client_id),
                ServerMessage::Chat(line) => {
                    let own = self.client_id.clone();
                    self.chat.receive(line, own.as_deref());
                }
                ServerMessage::ChatHistory { lines } => {
                    self.chat.history(lines, &self.player_name);
                }
                ServerMessage::Ping { name, x, y, .. } => {
                    let now = ctx.input(|input| input.time);
                    self.chat.add_ping(name, x, y, now);
                    ctx.request_repaint();
                }
                ServerMessage::Error { message } => {
                    if message.starts_with("Could not upload room media:") {
                        self.media_uploading = false;
                    }
                    // Keep the room snapshot and invite code after a transport
                    // failure so reconnecting can restore shared state.
                    let disconnected = message == "Room server connection closed."
                        || message.starts_with("Could not connect to ");
                    // Reconnecting to a room the server no longer has can
                    // never work; every other refusal (full, busy, rate
                    // limited) may pass, so those keep retrying.
                    if message == "Room code not found" && self.snapshot.is_some() {
                        self.room_gone = true;
                    }
                    self.status = message;
                    // Only a room we are already in can be rejoined; a failed
                    // first connection keeps its own explanation.
                    if disconnected && self.snapshot.is_some() {
                        self.network = None;
                        self.connected = false;
                        self.client_id = None;
                        self.peers.clear();
                        self.cursors.reset();
                        self.clear_active_gesture();
                        self.media_uploading = false;
                        if self.room_gone {
                            self.reconnect_at = None;
                            self.status = format!(
                                "Room {} is no longer on this server. Use More → Save package… to keep your progress, then Leave.",
                                self.invite_code
                            );
                        } else {
                            let delay = reconnect_delay(self.reconnect_attempt);
                            self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
                            self.reconnect_at = Some(Instant::now() + delay);
                            self.status = format!(
                                "Connection lost. Reconnecting to room {} in {}s…",
                                self.invite_code,
                                delay.as_secs()
                            );
                        }
                    } else if self.snapshot.is_none() {
                        self.network = None;
                        self.connected = false;
                        self.client_id = None;
                        self.media_uploading = false;
                    }
                }
            }
        }
    }
}

/// Queue items the room still lacks that we hold a file for, by hash. A file
/// is only ever offered under its own hash.
pub(crate) fn pending_uploads(
    snapshot: &RoomSnapshot,
    files: &HashMap<String, PathBuf>,
) -> Vec<(String, PathBuf)> {
    snapshot
        .queue
        .iter()
        .filter(|item| !item.available)
        .filter_map(|item| {
            files
                .get(&item.sha256)
                .map(|path| (item.sha256.clone(), path.clone()))
        })
        .collect()
}

/// What to upload for an upload token. Older servers send no hash, meaning
/// the room's own media.
pub(crate) fn token_upload(
    token_hash: Option<&str>,
    room_media_hash: Option<&str>,
    files: &HashMap<String, PathBuf>,
) -> Option<(String, PathBuf)> {
    let hash = token_hash.or(room_media_hash)?;
    files.get(hash).map(|path| (hash.to_owned(), path.clone()))
}

/// Upload each file in order on one background thread.
fn spawn_item_uploads(
    url: &str,
    token: &str,
    items: Vec<(String, PathBuf)>,
    errors: &WakingSender<ServerMessage>,
) {
    if items.is_empty() {
        return;
    }
    let url = url.to_owned();
    let token = token.to_owned();
    let errors = errors.clone();
    thread::spawn(move || {
        for (hash, path) in items {
            if let Err(error) = upload_room_media(&url, &hash, &token, &path) {
                let _ = errors.send(ServerMessage::Error {
                    message: format!("Could not upload room media: {error}"),
                });
                return;
            }
        }
    });
}

pub(crate) fn reconnect_delay(attempt: u8) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(5)).min(30))
}
