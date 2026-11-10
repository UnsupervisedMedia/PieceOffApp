use pieceoff_core::{ClientMessage, RoomRuntime, RoomSnapshot, ServerMessage, StoredRoom};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// JS timestamps arrive as f64. Anything that is not a sane, non-negative
/// integer-valued time (NaN, infinity, negative) becomes 0 so core never does
/// arithmetic on it; the upper clamp is JS's largest exact integer.
fn clock(now_ms: f64) -> u64 {
    if now_ms.is_finite() && now_ms >= 0.0 {
        now_ms.min(9_007_199_254_740_991.0) as u64
    } else {
        0
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Ok {
        event: Box<ServerMessage>,
        persist: bool,
    },
    Err(String),
}

#[derive(Serialize)]
struct Envelope<T: Serialize> {
    ok: T,
}

fn ok_reply<T: Serialize>(value: T) -> String {
    serde_json::to_string(&Envelope { ok: value })
        .unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
}

fn err_reply(message: impl Into<String>) -> String {
    reply(Reply::Err(message.into()))
}

#[derive(Serialize)]
struct CreateReply {
    name: String,
}

#[derive(Serialize)]
struct JoinReply {
    name: String,
    promoted_to_host: bool,
    needs_upload_token: bool,
}

#[derive(Serialize)]
struct JoinV14Reply {
    name: String,
    promoted_to_host: bool,
    needs_upload_token: bool,
    spectator: bool,
    replaced: Vec<String>,
}

#[derive(Serialize)]
struct AssignmentReply {
    client_id: String,
    sha256: String,
}

#[derive(Serialize)]
struct LeaveReply {
    name: String,
    snapshot: Option<RoomSnapshot>,
    upload_assignment: Option<AssignmentReply>,
}

#[derive(Serialize)]
struct KickedReply {
    client_id: String,
    spectator: bool,
}

#[derive(Serialize)]
struct KickReply {
    name: String,
    removed: Vec<KickedReply>,
    snapshot: Option<RoomSnapshot>,
    /// The `kicked` frame for each removed connection, in `removed` order.
    notices: Vec<ServerMessage>,
}

#[derive(Serialize)]
struct TickReply {
    events: Vec<ServerMessage>,
    persist: bool,
}

fn reply(reply: Reply) -> String {
    serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
}

/// The protocol version this core speaks.
#[wasm_bindgen]
pub fn protocol_version() -> u16 {
    pieceoff_core::PROTOCOL_VERSION
}

/// The bundled title of a built-in loop, or `undefined` for any other
/// fingerprint. Public listings show this, never a client-supplied title.
#[wasm_bindgen]
pub fn builtin_media_title(sha256: &str) -> Option<String> {
    pieceoff_core::builtin_media_title(sha256).map(str::to_owned)
}

/// Whether a display name may be shown in the public lobby or used in a
/// listed room.
#[wasm_bindgen]
pub fn public_name_allowed(name: &str) -> bool {
    pieceoff_core::public_name_allowed(name)
}

/// One room's rules, driven by JSON strings so a JavaScript Durable Object
/// can host it. Methods never throw except `from_snapshot` on unreadable JSON.
#[wasm_bindgen]
pub struct Room {
    runtime: RoomRuntime,
}

#[wasm_bindgen]
impl Room {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Room {
        Room {
            runtime: RoomRuntime::default(),
        }
    }

    pub fn from_snapshot(snapshot_json: &str) -> Result<Room, JsError> {
        let stored: StoredRoom =
            serde_json::from_str(snapshot_json).map_err(|e| JsError::new(&e.to_string()))?;
        let mut runtime = RoomRuntime::from_snapshot(stored.snapshot);
        runtime.name_claims = stored.name_claims;
        runtime.name_claims.truncate(pieceoff_core::MAX_NAME_CLAIMS);
        runtime.timelapse = stored.timelapse;
        runtime.bans = stored.bans;
        let excess = runtime
            .bans
            .len()
            .saturating_sub(pieceoff_core::MAX_ROOM_BANS);
        runtime.bans.drain(..excess);
        if let Some(snapshot) = runtime.snapshot.as_ref() {
            runtime.timelapse.sanitize(snapshot);
        }
        Ok(Room { runtime })
    }

    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.runtime.snapshot).unwrap_or_else(|_| "null".into())
    }

    /// The snapshot to save at `now_ms`, with the solve clock banked.
    pub fn stored_snapshot_json(&self, now_ms: f64) -> String {
        let stored = self.runtime.snapshot.as_ref().map(|snapshot| StoredRoom {
            snapshot: snapshot.for_storage(clock(now_ms)),
            name_claims: self.runtime.name_claims.clone(),
            timelapse: self.runtime.timelapse.clone(),
            bans: self.runtime.bans.clone(),
        });
        serde_json::to_string(&stored).unwrap_or_else(|_| "null".into())
    }

    /// Tell the room whether anyone is connected: the solve clock only runs
    /// while someone is.
    pub fn set_occupied(&mut self, occupied: bool, now_ms: f64) {
        self.runtime.set_occupied(occupied, clock(now_ms));
    }

    /// Update the solve clock after membership changes and return a snapshot
    /// when spectators or other players need to see the transition.
    pub fn sync_occupied(&mut self, now_ms: f64) -> Option<String> {
        let before = self.runtime.snapshot.as_ref()?.stats.clone();
        self.runtime.sync_clock_to_players(clock(now_ms));
        let snapshot = self.runtime.snapshot.as_mut()?;
        if snapshot.stats == before {
            return None;
        }
        snapshot.revision += 1;
        serde_json::to_string(&ServerMessage::Snapshot {
            snapshot: snapshot.clone(),
        })
        .ok()
    }

    pub fn handle(&mut self, client_id: &str, message_json: &str, now_ms: f64) -> String {
        let message: ClientMessage = match serde_json::from_str(message_json) {
            Ok(message) => message,
            Err(e) => return reply(Reply::Err(format!("Bad message: {e}"))),
        };
        match self.runtime.apply(client_id, message, clock(now_ms)) {
            Ok(out) => reply(Reply::Ok {
                event: Box::new(out.event),
                persist: out.persist,
            }),
            Err(message) => reply(Reply::Err(message)),
        }
    }

    /// The host removes a player by name (`kick_player`). The reply lists
    /// the connections to notify with `notices` and close, then the host
    /// sends `peer_left` for each and `snapshot` if present.
    pub fn kick(&mut self, client_id: &str, message_json: &str, now_ms: f64) -> String {
        let name = match serde_json::from_str::<ClientMessage>(message_json) {
            Ok(ClientMessage::KickPlayer { name }) => name,
            Ok(_) => return err_reply("Not a kick_player message"),
            Err(e) => return err_reply(format!("Bad message: {e}")),
        };
        match self.runtime.kick_player(client_id, &name, clock(now_ms)) {
            Ok(outcome) => ok_reply(KickReply {
                notices: outcome
                    .removed
                    .iter()
                    .map(|(id, _)| ServerMessage::Kicked {
                        client_id: id.clone(),
                        message: pieceoff_core::REMOVED_BY_HOST.into(),
                    })
                    .collect(),
                name: outcome.name,
                removed: outcome
                    .removed
                    .into_iter()
                    .map(|(client_id, spectator)| KickedReply {
                        client_id,
                        spectator,
                    })
                    .collect(),
                snapshot: outcome.snapshot,
            }),
            Err(message) => err_reply(message),
        }
    }

    /// Whether the room lets players chat right now.
    pub fn chat_enabled(&self) -> bool {
        self.runtime
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.chat_enabled)
    }

    /// Validate a cursor, chat line or ping without changing the saved room.
    pub fn relay(&self, client_id: &str, message_json: &str, chat_id: f64, now_ms: f64) -> String {
        let message: ClientMessage = match serde_json::from_str(message_json) {
            Ok(message) => message,
            Err(e) => return err_reply(format!("Bad message: {e}")),
        };
        match self
            .runtime
            .relay(client_id, message, clock(chat_id), clock(now_ms))
        {
            Ok(event) => ok_reply(event),
            Err(message) => err_reply(message),
        }
    }

    /// Replace this room with a new one built from a `create_room` message.
    /// The host supplies the room id and invite code.
    pub fn create(
        &mut self,
        client_id: &str,
        message_json: &str,
        room_id: &str,
        invite_code: &str,
    ) -> String {
        let message: ClientMessage = match serde_json::from_str(message_json) {
            Ok(message) => message,
            Err(e) => return err_reply(format!("Bad message: {e}")),
        };
        match RoomRuntime::new_room(client_id, message, room_id.into(), invite_code.into()) {
            Ok((runtime, name)) => {
                self.runtime = runtime;
                ok_reply(CreateReply { name })
            }
            Err(message) => err_reply(message),
        }
    }

    pub fn join(&mut self, client_id: &str, name: &str, media_sha256: &str) -> String {
        match self.runtime.join(client_id, name, media_sha256) {
            Ok(outcome) => ok_reply(JoinReply {
                name: outcome.name,
                promoted_to_host: outcome.promoted_to_host,
                needs_upload_token: outcome.needs_upload_token,
            }),
            Err(message) => err_reply(message),
        }
    }

    /// Join with protocol v14 identity and spectator semantics.
    pub fn join_v14(
        &mut self,
        client_id: &str,
        name: &str,
        media_sha256: &str,
        key_hash: &str,
        spectator: bool,
        now_ms: f64,
    ) -> String {
        let Some(snapshot) = self.runtime.snapshot.as_ref() else {
            return err_reply("Room is not ready");
        };
        if media_sha256 != snapshot.media_sha256 && !snapshot.media_available {
            return err_reply(
                "The room media is still uploading. Wait for the host and try joining again.",
            );
        }
        let name = pieceoff_core::clean_name(name);
        let key_hash = (!key_hash.is_empty()).then_some(key_hash);
        if spectator {
            return match self.runtime.admit_spectator(client_id, &name, key_hash) {
                Ok(()) => ok_reply(JoinV14Reply {
                    name,
                    promoted_to_host: false,
                    needs_upload_token: false,
                    spectator: true,
                    replaced: Vec::new(),
                }),
                Err(message) => err_reply(message),
            };
        }
        let replaced = match self
            .runtime
            .admit_player(client_id, &name, key_hash, clock(now_ms))
        {
            Ok(replaced) => replaced,
            Err(message) => return err_reply(message),
        };
        match self.runtime.join(client_id, &name, media_sha256) {
            Ok(outcome) => ok_reply(JoinV14Reply {
                name: outcome.name,
                promoted_to_host: outcome.promoted_to_host,
                needs_upload_token: outcome.needs_upload_token,
                spectator: false,
                replaced,
            }),
            Err(message) => err_reply(message),
        }
    }

    pub fn claim_creator(&mut self, client_id: &str, key_hash: &str, now_ms: f64) -> String {
        let Some(name) = self.runtime.names.get(client_id).cloned() else {
            return err_reply("Creator is not in this room");
        };
        match self.runtime.admit_player(
            client_id,
            &name,
            (!key_hash.is_empty()).then_some(key_hash),
            clock(now_ms),
        ) {
            Ok(_) => ok_reply(()),
            Err(message) => err_reply(message),
        }
    }

    /// Remove a player. The reply tells the host which events to send:
    /// `peer_left`, then `snapshot` if present, then an upload token for
    /// `upload_assignment` if present.
    pub fn leave(&mut self, client_id: &str, fallback_name: &str) -> String {
        if let Some(name) = self.runtime.spectators.remove(client_id) {
            return ok_reply(LeaveReply {
                name,
                snapshot: None,
                upload_assignment: None,
            });
        }
        let outcome = self.runtime.leave(client_id, fallback_name);
        ok_reply(LeaveReply {
            name: outcome.name,
            snapshot: outcome.snapshot,
            upload_assignment: outcome.upload_assignment.map(|a| AssignmentReply {
                client_id: a.client_id,
                sha256: a.sha256,
            }),
        })
    }

    /// The `welcome` event JSON for a client that has just joined, or an
    /// `{"err":..}` reply.
    pub fn welcome(
        &self,
        client_id: &str,
        name: &str,
        media_upload_token: Option<String>,
    ) -> String {
        match self.runtime.welcome(client_id, name, media_upload_token) {
            Ok(mut event) => {
                if let ServerMessage::Welcome {
                    spectator,
                    spectators,
                    server_now_ms,
                    ..
                } = &mut event
                {
                    *spectator = self.runtime.is_spectator(client_id);
                    *spectators = self
                        .runtime
                        .spectators
                        .iter()
                        .filter(|(id, _)| id.as_str() != client_id)
                        .map(|(_, name)| name.clone())
                        .collect();
                    *server_now_ms = 0;
                }
                serde_json::to_string(&event)
                    .unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
            }
            Err(message) => err_reply(message),
        }
    }

    pub fn welcome_v14(
        &self,
        client_id: &str,
        name: &str,
        media_upload_token: Option<String>,
        now_ms: f64,
    ) -> String {
        let json = self.welcome(client_id, name, media_upload_token);
        let Ok(mut event) = serde_json::from_str::<ServerMessage>(&json) else {
            return json;
        };
        if let ServerMessage::Welcome { server_now_ms, .. } = &mut event {
            *server_now_ms = clock(now_ms);
        }
        serde_json::to_string(&event).unwrap_or(json)
    }

    /// The current solve's replay, answered only to the requesting connection.
    pub fn timelapse_event(&self) -> String {
        serde_json::to_string(&ServerMessage::Timelapse {
            timelapse: self.runtime.timelapse.clone(),
        })
        .unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
    }

    /// Media with this fingerprint finished uploading. The reply carries the
    /// `snapshot` event to broadcast, or an `{"err":..}` if there is no room.
    pub fn mark_item_available(&mut self, sha256: &str) -> String {
        match self.runtime.mark_item_available(sha256) {
            Some(snapshot) => serde_json::to_string(&ServerMessage::Snapshot { snapshot })
                .unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into()),
            None => err_reply("Room is not initialized"),
        }
    }

    /// Whether `client_id` may see an event. Unreadable events are hidden.
    pub fn visible_to(event_json: &str, client_id: &str) -> bool {
        serde_json::from_str::<ServerMessage>(event_json)
            .map(|event| event.visible_to(client_id))
            .unwrap_or(false)
    }

    /// Milliseconds timestamp (same clock as `now_ms`) when `tick` will next
    /// advance the queue, or -1 when nothing will advance on its own.
    pub fn next_tick_deadline(&self, now_ms: f64) -> f64 {
        self.runtime
            .next_tick_deadline(clock(now_ms))
            .map_or(-1.0, |deadline| deadline as f64)
    }

    /// Record a player whose WebSocket survived hibernation, without the
    /// checks `join` makes. Used when rebuilding a room from storage.
    pub fn restore_player(&mut self, client_id: &str, name: &str, spectator: bool) {
        let roster = if spectator {
            &mut self.runtime.spectators
        } else {
            &mut self.runtime.names
        };
        roster.insert(client_id.to_owned(), pieceoff_core::clean_name(name));
    }

    pub fn peer_count(&self) -> u32 {
        self.runtime.names.len() as u32
    }

    /// The `Snapshot` event as JSON when the queue auto-advanced.
    pub fn tick(&mut self, now_ms: f64) -> Option<String> {
        self.runtime
            .tick_advance(clock(now_ms))
            .and_then(|snapshot| serde_json::to_string(&ServerMessage::Snapshot { snapshot }).ok())
    }

    /// Server-owned room maintenance, including claims and holders.
    pub fn tick_v14(&mut self, now_ms: f64) -> String {
        let now = clock(now_ms);
        let mut events = Vec::new();
        if let Some(snapshot) = self.runtime.tick_advance(now) {
            events.push(ServerMessage::Snapshot { snapshot });
        }
        let claims_changed = self.runtime.tick_claims(now);
        if let Some(event) = self.runtime.tick_holders(now) {
            events.push(event);
        }
        let persist = claims_changed || !events.is_empty();
        ok_reply(TickReply { events, persist })
    }
}

impl Default for Room {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created() -> Room {
        let snapshot = pieceoff_core::RoomSnapshot::create(
            "id".into(),
            "invite".into(),
            "test".into(),
            "hash".into(),
            "png".into(),
            800,
            600,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        Room::from_snapshot(&serde_json::to_string(&snapshot).unwrap()).unwrap()
    }

    #[test]
    fn builtin_titles_and_protocol_are_exported() {
        let (sha256, title) = pieceoff_core::BUILTIN_MEDIA_SHA256[0];
        assert_eq!(builtin_media_title(sha256).as_deref(), Some(title));
        assert_eq!(builtin_media_title("nope"), None);
        assert_eq!(protocol_version(), pieceoff_core::PROTOCOL_VERSION);
    }

    #[test]
    fn a_listed_create_with_uploaded_media_is_refused() {
        let mut room = Room::new();
        let create = create_json().replace(
            r#""duration_ms":5000"#,
            r#""duration_ms":5000,"listed":true"#,
        );
        let reply = room.create("host", &create, "room-id", "ABCDEFGH");
        assert!(
            reply.contains(pieceoff_core::PUBLIC_ROOMS_BUILTIN_ONLY),
            "{reply}"
        );
    }

    #[test]
    fn kick_removes_and_bans_and_survives_storage() {
        let mut room = hosted();
        let joined = room.join_v14("p1", "Troll", SHA, "troll-key", false, 1.0);
        assert!(joined.starts_with(r#"{"ok""#), "{joined}");
        assert!(
            room.kick("p1", r#"{"type":"kick_player","name":"Host"}"#, 2.0)
                .starts_with(r#"{"err""#)
        );
        let reply = room.kick("host", r#"{"type":"kick_player","name":"troll"}"#, 2.0);
        assert!(
            reply.contains(r#""removed":[{"client_id":"p1","spectator":false}]"#),
            "{reply}"
        );
        assert!(
            reply.contains(
                r#"{"type":"kicked","client_id":"p1","message":"The host removed you from this room."}"#
            ),
            "{reply}"
        );
        let mut again = Room::from_snapshot(&room.stored_snapshot_json(3.0)).unwrap();
        let refused = again.join_v14("p2", "Someone", SHA, "troll-key", false, 4.0);
        assert_eq!(refused, r#"{"err":"The host removed you from this room."}"#);
        assert!(public_name_allowed("Ada") && !public_name_allowed("p0rn"));
        assert!(again.chat_enabled());
    }

    #[test]
    fn handle_applies_a_message_and_returns_the_event() {
        let mut room = created();
        let reply = room.handle("host", r#"{"type":"drag_start","piece_id":0}"#, 0.0);
        assert!(reply.contains(r#""ok""#), "{reply}");
        assert!(reply.contains(r#""persist":false"#), "{reply}");
    }

    #[test]
    fn handle_reports_rule_errors_as_err_replies() {
        let mut room = Room::new();
        let reply = room.handle("host", r#"{"type":"drag_start","piece_id":0}"#, 0.0);
        assert_eq!(reply, r#"{"err":"Room is not initialized"}"#);
    }

    #[test]
    fn garbage_json_is_an_error_reply_not_a_panic() {
        let mut room = created();
        for bad in ["", "{", "null", r#"{"type":"nope"}"#, "\u{0}"] {
            let reply = room.handle("host", bad, 0.0);
            assert!(reply.starts_with(r#"{"err""#), "{bad:?} -> {reply}");
        }
    }

    #[test]
    fn non_finite_time_does_not_trap() {
        let mut room = created();
        for now in [f64::NAN, f64::INFINITY, -1.0] {
            let reply = room.handle("host", r#"{"type":"drag_start","piece_id":1}"#, now);
            assert!(reply.starts_with('{'), "{now} -> {reply}");
        }
    }

    #[test]
    fn snapshot_round_trips() {
        let room = created();
        let json = room.snapshot_json();
        let again = Room::from_snapshot(&json).unwrap();
        assert_eq!(again.snapshot_json(), json);
    }

    #[test]
    fn the_solve_clock_pauses_when_empty_and_is_banked_for_storage() {
        let mut room = created();
        room.handle("host", r#"{"type":"drag_start","piece_id":0}"#, 1_000.0);
        let stored: pieceoff_core::RoomSnapshot =
            serde_json::from_str(&room.stored_snapshot_json(3_000.0)).unwrap();
        assert_eq!(stored.stats.elapsed_ms, 2_000);
        assert!(!stored.stats.running());
        room.set_occupied(false, 4_000.0);
        room.set_occupied(true, 10_000.0);
        let live: pieceoff_core::RoomSnapshot =
            serde_json::from_str(&room.snapshot_json()).unwrap();
        assert_eq!(live.stats.elapsed_at(11_000), 4_000);
    }

    #[test]
    fn tick_is_quiet_when_nothing_is_due() {
        let mut room = created();
        assert!(room.tick(0.0).is_none());
    }

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn create_json() -> String {
        format!(
            r#"{{"type":"create_room","name":"Host","title":"Puzzle","media_sha256":"{SHA}","media_extension":"mp4","width":640,"height":360,"rows":3,"columns":4,"duration_ms":5000}}"#
        )
    }

    fn hosted() -> Room {
        let mut room = Room::new();
        let reply = room.create("host", &create_json(), "room-id", "ABCDEFGH");
        assert_eq!(reply, r#"{"ok":{"name":"Host"}}"#);
        room
    }

    #[test]
    fn create_join_leave_round_trip() {
        let mut room = hosted();
        assert_eq!(room.peer_count(), 1);
        let joined = room.join("p1", "Pat", SHA);
        assert_eq!(
            joined,
            r#"{"ok":{"name":"Pat","promoted_to_host":false,"needs_upload_token":false}}"#
        );
        assert_eq!(room.peer_count(), 2);
        let left = room.leave("p1", "fallback");
        assert!(
            left.starts_with(r#"{"ok":{"name":"Pat","snapshot":null"#),
            "{left}"
        );
        assert_eq!(room.peer_count(), 1);
    }

    #[test]
    fn create_rejects_a_non_create_message_and_garbage() {
        let mut room = Room::new();
        for bad in ["", "{", r#"{"type":"next_item"}"#] {
            let reply = room.create("host", bad, "id", "CODE");
            assert!(reply.starts_with(r#"{"err""#), "{bad:?} -> {reply}");
        }
        assert_eq!(room.peer_count(), 0);
    }

    #[test]
    fn join_on_an_empty_room_is_an_err_reply() {
        let mut room = Room::new();
        assert_eq!(
            room.join("p1", "Pat", SHA),
            r#"{"err":"Room is not ready"}"#
        );
    }

    #[test]
    fn welcome_carries_the_snapshot_and_hides_the_token_from_non_hosts() {
        let mut room = hosted();
        room.join("p1", "Pat", SHA);
        let host = room.welcome("host", "Host", Some("tok".into()));
        assert!(host.contains(r#""type":"welcome""#), "{host}");
        assert!(host.contains(r#""media_upload_token":"tok""#), "{host}");
        let guest = room.welcome("p1", "Pat", Some("tok".into()));
        assert!(guest.contains(r#""media_upload_token":null"#), "{guest}");
        assert!(Room::new().welcome("a", "A", None).starts_with(r#"{"err""#));
    }

    #[test]
    fn host_leaving_reports_the_upload_assignment() {
        let mut room = hosted();
        room.join("p1", "Pat", SHA);
        let left = room.leave("host", "Host");
        assert!(
            left.contains(r#""upload_assignment":{"client_id":"p1""#),
            "{left}"
        );
    }

    #[test]
    fn visible_to_hides_upload_tokens_from_other_clients() {
        let token = r#"{"type":"media_upload_token","client_id":"a","token":"t"}"#;
        assert!(Room::visible_to(token, "a"));
        assert!(!Room::visible_to(token, "b"));
        assert!(Room::visible_to(r#"{"type":"peer_left","name":"n"}"#, "b"));
        assert!(!Room::visible_to("garbage", "b"));
    }

    #[test]
    fn restore_player_rebuilds_the_roster_and_deadline_is_exposed() {
        let mut room = created();
        assert_eq!(room.peer_count(), 0);
        room.restore_player("p1", "Pat", false);
        assert_eq!(room.peer_count(), 1);
        assert_eq!(room.next_tick_deadline(0.0), -1.0);
    }

    #[test]
    fn mark_item_available_returns_the_snapshot_event() {
        let mut room = hosted();
        let event = room.mark_item_available(SHA);
        assert!(event.starts_with(r#"{"type":"snapshot""#), "{event}");
        assert!(event.contains(r#""available":true"#), "{event}");
        assert!(
            Room::new()
                .mark_item_available(SHA)
                .starts_with(r#"{"err""#)
        );
    }
}
