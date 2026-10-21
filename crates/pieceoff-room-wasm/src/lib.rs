use pieceoff_core::{ClientMessage, RoomRuntime, RoomSnapshot, ServerMessage};
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

fn reply(reply: Reply) -> String {
    serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
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
        let snapshot: RoomSnapshot =
            serde_json::from_str(snapshot_json).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Room {
            runtime: RoomRuntime::from_snapshot(snapshot),
        })
    }

    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.runtime.snapshot).unwrap_or_else(|_| "null".into())
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

    /// Remove a player. The reply tells the host which events to send:
    /// `peer_left`, then `snapshot` if present, then an upload token for
    /// `upload_assignment` if present.
    pub fn leave(&mut self, client_id: &str, fallback_name: &str) -> String {
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
            Ok(event) => serde_json::to_string(&event)
                .unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into()),
            Err(message) => err_reply(message),
        }
    }

    /// Whether `client_id` may see an event. Unreadable events are hidden.
    pub fn visible_to(event_json: &str, client_id: &str) -> bool {
        serde_json::from_str::<ServerMessage>(event_json)
            .map(|event| event.visible_to(client_id))
            .unwrap_or(false)
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
}
