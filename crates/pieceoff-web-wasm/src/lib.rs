use pieceoff_core::{RoomSnapshot, piece_outline};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// The protocol version this core speaks.
#[wasm_bindgen]
pub fn protocol_version() -> u16 {
    pieceoff_core::PROTOCOL_VERSION
}

/// Whether a display name may be shown in the public lobby or used in a
/// listed room.
#[wasm_bindgen]
pub fn public_name_allowed(name: &str) -> bool {
    pieceoff_core::public_name_allowed(name)
}

#[derive(Serialize)]
struct BuiltinMedia {
    sha256: &'static str,
    title: &'static str,
}

fn builtin_media_json() -> String {
    let list: Vec<BuiltinMedia> = pieceoff_core::BUILTIN_MEDIA_SHA256
        .iter()
        .map(|(sha256, title)| BuiltinMedia { sha256, title })
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

/// The bundled loops as `[{"sha256":..,"title":..}]`.
#[wasm_bindgen(js_name = builtin_media_json)]
pub fn builtin_media_json_js() -> String {
    builtin_media_json()
}

/// Plain-Rust outline data: one contour per piece, flattened.
#[derive(Debug, Default, PartialEq)]
struct OutlineData {
    ids: Vec<u32>,
    /// Start index per piece into `points`, plus the final end.
    offsets: Vec<u32>,
    /// Flat `x, y` pairs.
    points: Vec<f32>,
}

fn outlines_from_json(snapshot_json: &str) -> Result<OutlineData, String> {
    let snapshot: RoomSnapshot = serde_json::from_str(snapshot_json).map_err(|e| e.to_string())?;
    let mut data = OutlineData::default();
    data.offsets.push(0);
    for piece in &snapshot.pieces {
        data.ids.push(piece.id);
        for [x, y] in piece_outline(&snapshot, piece) {
            data.points.push(x);
            data.points.push(y);
        }
        data.offsets.push(data.points.len() as u32);
    }
    Ok(data)
}

/// Every piece's contour, in snapshot piece order.
#[wasm_bindgen]
pub struct Outlines(OutlineData);

#[wasm_bindgen]
impl Outlines {
    #[wasm_bindgen(getter)]
    pub fn ids(&self) -> Vec<u32> {
        self.0.ids.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn offsets(&self) -> Vec<u32> {
        self.0.offsets.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn points(&self) -> Vec<f32> {
        self.0.points.clone()
    }
}

#[wasm_bindgen]
pub fn piece_outlines(snapshot_json: &str) -> Result<Outlines, JsError> {
    outlines_from_json(snapshot_json)
        .map(Outlines)
        .map_err(|e| JsError::new(&e))
}

#[derive(Debug, PartialEq, Serialize)]
struct RectJson {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug, PartialEq, Serialize)]
struct SceneJson {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

/// The table around the board, in board coordinates (source pixels from the
/// board's top-left), as the desktop board draws it.
#[derive(Debug, PartialEq, Serialize)]
struct BoardLayout {
    /// The tray's slot area (core `tray_layout`, moved from content to board
    /// coordinates).
    tray: RectJson,
    /// How far the drawn tray reaches past `tray` on every side: tabs reach
    /// past a piece's cell (desktop pads it by 0.3 of the larger cell side).
    tray_pad: f32,
    /// Where pieces (and cursors and pings) may be: core `scene_bounds`.
    scene: SceneJson,
}

fn board_layout_from_json(snapshot_json: &str) -> Result<BoardLayout, String> {
    let snapshot: RoomSnapshot = serde_json::from_str(snapshot_json).map_err(|e| e.to_string())?;
    let tray = snapshot.tray_layout();
    let scene = snapshot.scene_bounds();
    Ok(BoardLayout {
        tray: RectJson {
            x: tray.tray_x,
            y: tray.tray_y - tray.board_y,
            width: tray.tray_width,
            height: tray.tray_height,
        },
        tray_pad: snapshot.cell_width().max(snapshot.cell_height()) * 0.3,
        scene: SceneJson {
            min_x: scene.min_x,
            min_y: scene.min_y,
            max_x: scene.max_x,
            max_y: scene.max_y,
        },
    })
}

/// `{"tray":{x,y,width,height},"tray_pad":..,"scene":{min_x,min_y,max_x,max_y}}`
/// for a snapshot: the tray and table geometry come from core, so the web
/// board never re-derives them.
#[wasm_bindgen]
pub fn board_layout(snapshot_json: &str) -> Result<String, JsError> {
    board_layout_from_json(snapshot_json)
        .map(|layout| serde_json::to_string(&layout).unwrap_or_else(|_| "{}".into()))
        .map_err(|e| JsError::new(&e))
}

/// A new holder of core's default size (`HOLDER_DEFAULT_CELLS`) at the free
/// spot nearest `(near_x, near_y)` (core `free_holder_spot`), or `None` when
/// the table has no room for one.
fn new_holder_rect_from_json(
    snapshot_json: &str,
    near_x: f32,
    near_y: f32,
) -> Result<Option<RectJson>, String> {
    let snapshot: RoomSnapshot = serde_json::from_str(snapshot_json).map_err(|e| e.to_string())?;
    let (cells_w, cells_h) = pieceoff_core::HOLDER_DEFAULT_CELLS;
    let width = cells_w * snapshot.cell_width();
    let height = cells_h * snapshot.cell_height();
    Ok(snapshot
        .free_holder_spot(width, height, near_x, near_y)
        .map(|(x, y)| RectJson {
            x,
            y,
            width,
            height,
        }))
}

/// `{"x","y","width","height"}` for a new default-size holder near the given
/// board point, or `null` when the table is full. The geometry (size, the
/// board, tray and holder clearances) is core's; the server still decides.
#[wasm_bindgen]
pub fn new_holder_rect(snapshot_json: &str, near_x: f32, near_y: f32) -> Result<String, JsError> {
    new_holder_rect_from_json(snapshot_json, near_x, near_y)
        .map(|rect| serde_json::to_string(&rect).unwrap_or_else(|_| "null".into()))
        .map_err(|e| JsError::new(&e))
}

/// Every `type` tag a serde internally tagged enum accepts, in declaration
/// order. Serde keeps no public list of variants, but it names them all when
/// refusing an unknown one, so this asks it to refuse one.
fn serde_variant_tags<T: serde::de::DeserializeOwned>() -> Vec<String> {
    let probe = r#"{"type":"\u0000"}"#;
    let message = match serde_json::from_str::<T>(probe) {
        Ok(_) => panic!("serde accepted the probe tag"),
        Err(err) => err.to_string(),
    };
    let (_, expected) = message
        .split_once("expected ")
        .unwrap_or_else(|| panic!("unexpected serde error: {message}"));
    // "`a`", "`a` or `b`" or "one of `a`, `b`, `c`"; a trailing
    // " at line 1 column N" holds no backticks.
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The `type` tags of `ClientMessage`.
pub fn client_message_types() -> Vec<String> {
    serde_variant_tags::<pieceoff_core::ClientMessage>()
}

/// The `type` tags of `ServerMessage`.
pub fn server_message_types() -> Vec<String> {
    serde_variant_tags::<pieceoff_core::ServerMessage>()
}

#[derive(Serialize)]
struct MessageTypes {
    client: Vec<String>,
    server: Vec<String>,
}

/// `{"client":[..],"server":[..]}`: every message `type` tag this core
/// accepts, so the web client's tests can check its generated unions.
#[wasm_bindgen]
pub fn message_types_json() -> String {
    let types = MessageTypes {
        client: client_message_types(),
        server: server_message_types(),
    };
    serde_json::to_string(&types).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_tags_come_from_serde() {
        let client = client_message_types();
        assert_eq!(client.first().map(String::as_str), Some("create_room"));
        assert!(client.iter().any(|t| t == "release_name"));
        assert!(client.iter().any(|t| t == "cursor_hidden"));
        let server = server_message_types();
        assert_eq!(server.first().map(String::as_str), Some("welcome"));
        assert!(server.iter().any(|t| t == "chat"));
        assert!(server.iter().any(|t| t == "kicked"));
        for tag in client.iter().chain(&server) {
            let probe = format!(r#"{{"type":"{tag}"}}"#);
            let err = serde_json::from_str::<pieceoff_core::ClientMessage>(&probe)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            let err2 = serde_json::from_str::<pieceoff_core::ServerMessage>(&probe)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            // A real tag is never refused as an unknown variant by its enum.
            assert!(
                !(err.contains("unknown variant") && err2.contains("unknown variant")),
                "{tag} is not a real tag"
            );
        }
    }

    use pieceoff_core::{CutStyle, RoomSnapshot, piece_outline};

    fn snapshot(style: CutStyle) -> RoomSnapshot {
        RoomSnapshot::create(
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
        .unwrap()
        .with_cut_style(style)
    }

    #[test]
    fn outlines_match_core_for_every_cut_style() {
        for style in CutStyle::ALL {
            let snap = snapshot(style);
            let json = serde_json::to_string(&snap).unwrap();
            let out = outlines_from_json(&json).unwrap();
            assert_eq!(out.ids.len(), snap.pieces.len());
            assert_eq!(out.offsets.len(), snap.pieces.len() + 1);
            assert_eq!(*out.offsets.last().unwrap() as usize, out.points.len());
            for (i, piece) in snap.pieces.iter().enumerate() {
                assert_eq!(out.ids[i], piece.id);
                let want: Vec<f32> = piece_outline(&snap, piece)
                    .iter()
                    .flat_map(|p| *p)
                    .collect();
                let (a, b) = (out.offsets[i] as usize, out.offsets[i + 1] as usize);
                assert_eq!(&out.points[a..b], &want[..], "{style:?} piece {i}");
            }
        }
    }

    #[test]
    fn builtin_json_lists_all_eight() {
        let v: serde_json::Value = serde_json::from_str(&builtin_media_json()).unwrap();
        let list = v.as_array().unwrap();
        assert_eq!(list.len(), 8);
        let (sha, title) = pieceoff_core::BUILTIN_MEDIA_SHA256[0];
        assert_eq!(list[0]["sha256"], sha);
        assert_eq!(list[0]["title"], title);
    }

    #[test]
    fn protocol_version_matches_core() {
        assert_eq!(protocol_version(), pieceoff_core::PROTOCOL_VERSION);
    }

    #[test]
    fn board_layout_is_core_tray_and_scene_in_board_coordinates() {
        for style in CutStyle::ALL {
            let snap = snapshot(style);
            let json = serde_json::to_string(&snap).unwrap();
            let got = board_layout_from_json(&json).unwrap();
            let tray = snap.tray_layout();
            let scene = snap.scene_bounds();
            assert_eq!(got.tray.x, tray.tray_x);
            assert_eq!(got.tray.y, tray.tray_y - tray.board_y);
            assert_eq!(got.tray.width, tray.tray_width);
            assert_eq!(got.tray.height, tray.tray_height);
            assert_eq!(got.scene.min_x, scene.min_x);
            assert_eq!(got.scene.max_y, scene.max_y);
            assert!(got.tray_pad > 0.0);
            // Every freshly scattered piece starts inside the drawn tray.
            for piece in &snap.pieces {
                assert!(piece.x >= got.tray.x - got.tray_pad, "{style:?}");
                assert!(
                    piece.y >= got.tray.y - got.tray_pad - snap.cell_height(),
                    "{style:?}"
                );
            }
        }
        assert!(board_layout_from_json("{").is_err());
    }

    #[test]
    fn new_holder_rect_is_a_free_default_holder_core_accepts() {
        for style in CutStyle::ALL {
            let mut snap = snapshot(style);
            let json = serde_json::to_string(&snap).unwrap();
            let got = new_holder_rect_from_json(&json, 400.0, 300.0)
                .unwrap()
                .unwrap();
            let (cw, ch) = (snap.cell_width(), snap.cell_height());
            assert_eq!(got.width, pieceoff_core::HOLDER_DEFAULT_CELLS.0 * cw);
            assert_eq!(got.height, pieceoff_core::HOLDER_DEFAULT_CELLS.1 * ch);
            assert_eq!(
                Some((got.x, got.y)),
                snap.free_holder_spot(got.width, got.height, 400.0, 300.0)
            );
            assert_eq!(
                snap.check_holder_rect(got.x, got.y, got.width, got.height, None),
                Ok(())
            );
            // With that one taken, the next lands elsewhere and is valid too.
            snap.holders.push(pieceoff_core::PieceHolder {
                id: 1,
                owner: "Ann".into(),
                label: "Holder 1".into(),
                x: got.x,
                y: got.y,
                width: got.width,
                height: got.height,
                owner_away_since_ms: None,
            });
            let json = serde_json::to_string(&snap).unwrap();
            let next = new_holder_rect_from_json(&json, 400.0, 300.0)
                .unwrap()
                .unwrap();
            assert_ne!((next.x, next.y), (got.x, got.y));
            assert_eq!(
                snap.check_holder_rect(next.x, next.y, next.width, next.height, None),
                Ok(())
            );
            assert!(
                new_holder_rect_from_json(&json, f32::NAN, 0.0)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(new_holder_rect_from_json("{", 0.0, 0.0).is_err());
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(outlines_from_json("{").is_err());
    }
}
