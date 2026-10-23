//! Piece holders on the table: each player's private sorting areas. This is
//! the drawing, the move and resize gestures, the toolbar menu, and the lock
//! hint over someone else's reserved piece. The server decides what is
//! reserved (see `pieceoff_core::holders`); the client only mirrors it so a
//! refused pickup never leaves the player's hand.

use super::*;
use pieceoff_core::{
    HOLDER_DEFAULT_CELLS, HOLDER_MAX_CELLS, HOLDER_MIN_CELLS, MAX_HOLDERS_PER_PLAYER, PieceHolder,
};

/// Height of the label tab above each holder, in screen points.
pub(crate) const HOLDER_TAB_HEIGHT: f32 = 20.0;
/// The resize grip at a holder's bottom-right corner, in screen points.
const HOLDER_HANDLE_SIZE: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HolderGrip {
    /// The label tab: drag to move.
    Move,
    /// The bottom-right corner: drag to resize.
    Resize,
}

/// One of our holders being moved or resized. Only the outline moves while
/// the pointer does; the server gets one update on release.
pub(crate) struct HolderDrag {
    pub(crate) holder_id: u32,
    pub(crate) grip: HolderGrip,
    /// Board point under the pointer when the drag began.
    pub(crate) start: Pos2,
    /// The holder's area (board coordinates) when the drag began.
    pub(crate) origin: Rect,
    /// Where it would go now.
    pub(crate) preview: Rect,
}

#[derive(Default)]
pub(crate) struct HolderTool {
    pub(crate) drag: Option<HolderDrag>,
    /// The button or key asked for a new holder; made on the next board frame,
    /// where the pointer and view are known.
    pub(crate) create_requested: bool,
    /// Labels being typed in the Holders menu, by holder ID.
    pub(crate) label_edits: HashMap<u32, String>,
    /// A name the host is typing to release, in the players menu.
    pub(crate) release_input: String,
}

/// Names whose player is away and whose holders are still kept, which the
/// host may release so the player can rejoin from another device. Empty for
/// everyone but the host.
pub(crate) fn releasable_names(holders: &[PieceHolder], is_host: bool, me: &str) -> Vec<String> {
    if !is_host {
        return Vec::new();
    }
    let mut names: Vec<String> = holders
        .iter()
        .filter(|holder| holder.owner_away_since_ms.is_some() && holder.owner != me)
        .map(|holder| holder.owner.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

pub(crate) fn holder_rect(holder: &PieceHolder) -> Rect {
    Rect::from_min_size(
        Pos2::new(holder.x, holder.y),
        Vec2::new(holder.width, holder.height),
    )
}

/// The holder's label tab, on screen: a strip along its top edge.
pub(crate) fn holder_tab(screen: Rect) -> Rect {
    Rect::from_min_max(
        screen.left_top() - Vec2::new(0.0, HOLDER_TAB_HEIGHT),
        screen.right_top(),
    )
}

pub(crate) fn holder_handle(screen: Rect) -> Rect {
    Rect::from_center_size(screen.right_bottom(), Vec2::splat(HOLDER_HANDLE_SIZE))
}

/// Which grip of which of `me`'s holders is under `pointer` (screen). Other
/// players' holders have no grips.
pub(crate) fn holder_grip_at(
    holders: &[PieceHolder],
    me: &str,
    pointer: Pos2,
    to_screen: impl Fn(Rect) -> Rect,
) -> Option<(u32, HolderGrip)> {
    holders
        .iter()
        .rev()
        .filter(|holder| holder.owner == me)
        .find_map(|holder| {
            let screen = to_screen(holder_rect(holder));
            if holder_handle(screen).contains(pointer) {
                Some((holder.id, HolderGrip::Resize))
            } else if holder_tab(screen).contains(pointer) {
                Some((holder.id, HolderGrip::Move))
            } else {
                None
            }
        })
}

/// Where a dragged holder would go with the pointer at `pointer` (board
/// coordinates). Resizing keeps to the allowed size; the server checks the
/// rest.
pub(crate) fn holder_drag_preview(
    drag: &HolderDrag,
    pointer: Pos2,
    cell_width: f32,
    cell_height: f32,
) -> Rect {
    let delta = pointer - drag.start;
    match drag.grip {
        HolderGrip::Move => drag.origin.translate(delta),
        HolderGrip::Resize => {
            let size = drag.origin.size() + delta;
            let size = Vec2::new(
                size.x
                    .clamp(HOLDER_MIN_CELLS * cell_width, HOLDER_MAX_CELLS * cell_width),
                size.y.clamp(
                    HOLDER_MIN_CELLS * cell_height,
                    HOLDER_MAX_CELLS * cell_height,
                ),
            );
            Rect::from_min_size(drag.origin.min, size)
        }
    }
}

/// What the tab says: the label, then whose it is.
pub(crate) fn holder_caption(holder: &PieceHolder, me: &str) -> String {
    let owner = if holder.owner == me {
        "you"
    } else {
        holder.owner.as_str()
    };
    if holder.owner_away_since_ms.is_some() {
        format!("{} · {owner} (away)", holder.label)
    } else {
        format!("{} · {owner}", holder.label)
    }
}

/// Draw every holder, under the pieces: a tint, a border, and a label tab in
/// the owner's colour. `preview` replaces one holder's area while it is
/// being dragged; `valid` says whether that spot is allowed.
pub(crate) fn paint_holders(
    painter: &egui::Painter,
    holders: &[PieceHolder],
    me: &str,
    preview: Option<(u32, Rect, bool)>,
    to_screen: impl Fn(Rect) -> Rect,
) {
    for holder in holders {
        let color = player_color(&holder.owner);
        let (area, valid) = match preview {
            Some((id, area, valid)) if id == holder.id => (area, valid),
            _ => (holder_rect(holder), true),
        };
        let screen = to_screen(area);
        let edge = if valid {
            color
        } else {
            Color32::from_rgb(235, 90, 80)
        };
        painter.rect_filled(screen, 6.0, edge.gamma_multiply(0.13));
        painter.rect_stroke(
            screen,
            6.0,
            Stroke::new(2.0_f32, edge.gamma_multiply(0.85)),
            egui::StrokeKind::Inside,
        );
        let tab = holder_tab(screen);
        painter.rect_filled(
            tab,
            egui::CornerRadius {
                nw: 6,
                ne: 6,
                sw: 0,
                se: 0,
            },
            edge.gamma_multiply(0.9),
        );
        let text_color = Color32::from_rgb(16, 18, 22);
        let lock = if holder.owner == me { "" } else { "🔒 " };
        painter
            .with_clip_rect(tab.intersect(painter.clip_rect()))
            .text(
                tab.left_center() + Vec2::new(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{lock}{}", holder_caption(holder, me)),
                egui::FontId::proportional(13.0),
                text_color,
            );
        if holder.owner == me {
            // Three short diagonal strokes: the usual resize grip.
            let corner = screen.right_bottom() - Vec2::splat(4.0);
            for step in [4.0, 8.0, 12.0] {
                painter.line_segment(
                    [corner - Vec2::new(step, 0.0), corner - Vec2::new(0.0, step)],
                    Stroke::new(1.5_f32, edge),
                );
            }
        }
    }
}

impl PieceOffApp {
    /// Ask the room for a new holder of the default size at the free spot
    /// nearest `near` (board coordinates).
    pub(crate) fn create_holder_near(&mut self, snapshot: &RoomSnapshot, near: Pos2) {
        let mine = snapshot
            .holders
            .iter()
            .filter(|holder| holder.owner == self.player_name)
            .count();
        if mine >= MAX_HOLDERS_PER_PLAYER {
            self.status = format!(
                "You already have {MAX_HOLDERS_PER_PLAYER} holders. Remove one to make another."
            );
            return;
        }
        let width = HOLDER_DEFAULT_CELLS.0 * snapshot.cell_width();
        let height = HOLDER_DEFAULT_CELLS.1 * snapshot.cell_height();
        let Some((x, y)) = snapshot.free_holder_spot(width, height, near.x, near.y) else {
            self.status = "There is no free space on the table for a holder.".into();
            return;
        };
        self.send(ClientMessage::CreateHolder {
            x,
            y,
            width,
            height,
            label: format!("Holder {}", mine + 1),
        });
        self.status = "New holder: pieces you put in it are yours. Drag its tab to move it.".into();
    }

    /// Send a holder's new area and label.
    pub(crate) fn send_holder_update(&mut self, holder: &PieceHolder, area: Rect, label: String) {
        self.send(ClientMessage::UpdateHolder {
            holder_id: holder.id,
            x: area.min.x,
            y: area.min.y,
            width: area.width(),
            height: area.height(),
            label,
        });
    }

    /// The toolbar's Holders menu: make one, rename or remove yours, and (for
    /// the host) clear anyone's.
    pub(crate) fn holders_menu(&mut self, ui: &mut egui::Ui, is_host: bool) {
        let holders = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.holders.clone())
            .unwrap_or_default();
        if ui
            .button("New holder")
            .on_hover_text(self.hint_with_keys(
                "Make a private sorting area on the table, near the pointer",
                Action::NewHolder,
            ))
            .clicked()
        {
            ui.close();
            self.holders.create_requested = true;
        }
        ui.weak("Pieces in your holder are yours to move;\nothers see them but cannot pick them up.\nDrag the tab to move it, the corner to resize.");
        if holders.is_empty() {
            return;
        }
        ui.separator();
        for holder in &holders {
            let mine = holder.owner == self.player_name;
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.5, player_color(&holder.owner));
                if mine {
                    let text = self
                        .holders
                        .label_edits
                        .entry(holder.id)
                        .or_insert_with(|| holder.label.clone());
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .desired_width(120.0)
                            .char_limit(pieceoff_core::MAX_HOLDER_LABEL_CHARS),
                    );
                    if edit.lost_focus() {
                        let label = self
                            .holders
                            .label_edits
                            .remove(&holder.id)
                            .unwrap_or_default();
                        if label.trim() != holder.label {
                            self.send_holder_update(holder, holder_rect(holder), label);
                        }
                    }
                    if ui.button("Remove").clicked() {
                        self.send(ClientMessage::DeleteHolder {
                            holder_id: holder.id,
                        });
                    }
                } else {
                    ui.label(holder_caption(holder, &self.player_name));
                    if is_host
                        && ui
                            .button("Clear")
                            .on_hover_text("Remove this holder; its pieces stay where they are")
                            .clicked()
                    {
                        self.send(ClientMessage::DeleteHolder {
                            holder_id: holder.id,
                        });
                    }
                }
            });
        }
        let away = releasable_names(&holders, is_host, &self.player_name);
        if away.is_empty() {
            return;
        }
        ui.separator();
        for name in away {
            ui.horizontal(|ui| {
                ui.label(format!("{name} (away)"));
                if ui
                    .button("Release name")
                    .on_hover_text(format!(
                        "Free {name}'s name now and remove their holders, so they can rejoin from another device"
                    ))
                    .clicked()
                {
                    self.release_name(&name);
                }
            });
        }
    }

    /// Ask the server to free an absent player's name (host only).
    pub(crate) fn release_name(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.send(ClientMessage::ReleaseName { name: name.into() });
        self.status = format!("Released the name {name}.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(owner: &str) -> PieceHolder {
        PieceHolder {
            id: 3,
            owner: owner.into(),
            label: "Sky".into(),
            x: 100.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
            owner_away_since_ms: None,
        }
    }

    #[test]
    fn only_our_own_holders_have_grips() {
        let holders = vec![holder("Ada")];
        let screen = |rect: Rect| rect;
        let tab = Pos2::new(150.0, 200.0 - HOLDER_TAB_HEIGHT / 2.0);
        let corner = Pos2::new(500.0, 500.0);
        assert_eq!(
            holder_grip_at(&holders, "Ada", tab, screen),
            Some((3, HolderGrip::Move))
        );
        assert_eq!(
            holder_grip_at(&holders, "Ada", corner, screen),
            Some((3, HolderGrip::Resize))
        );
        assert_eq!(holder_grip_at(&holders, "Bo", tab, screen), None);
        // The inside is for pieces, not the holder.
        assert_eq!(
            holder_grip_at(&holders, "Ada", Pos2::new(300.0, 350.0), screen),
            None
        );
    }

    #[test]
    fn dragging_moves_freely_and_resizing_keeps_to_the_allowed_size() {
        let drag = |grip| HolderDrag {
            holder_id: 3,
            grip,
            start: Pos2::new(0.0, 0.0),
            origin: Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(400.0, 300.0)),
            preview: Rect::NOTHING,
        };
        let moved = holder_drag_preview(
            &drag(HolderGrip::Move),
            Pos2::new(50.0, -20.0),
            100.0,
            100.0,
        );
        assert_eq!(moved.min, Pos2::new(60.0, -10.0));
        assert_eq!(moved.size(), Vec2::new(400.0, 300.0));
        let small = holder_drag_preview(
            &drag(HolderGrip::Resize),
            Pos2::new(-1000.0, -1000.0),
            100.0,
            100.0,
        );
        assert_eq!(small.size(), Vec2::new(200.0, 200.0));
        assert_eq!(small.min, Pos2::new(10.0, 10.0));
        let big = holder_drag_preview(
            &drag(HolderGrip::Resize),
            Pos2::new(9000.0, 9000.0),
            100.0,
            100.0,
        );
        assert_eq!(big.size(), Vec2::new(1200.0, 1200.0));
    }

    #[test]
    fn only_the_host_may_release_away_players_names() {
        let mut ada = holder("Ada");
        ada.owner_away_since_ms = Some(1);
        let mut ada_too = ada.clone();
        ada_too.id = 4;
        let bo = holder("Bo");
        let mut host = holder("Host");
        host.owner_away_since_ms = Some(1);
        let holders = vec![ada, bo, ada_too, host];
        assert_eq!(
            releasable_names(&holders, true, "Host"),
            vec!["Ada".to_string()]
        );
        assert!(releasable_names(&holders, false, "Bo").is_empty());
        assert!(releasable_names(&[holder("Bo")], true, "Host").is_empty());
    }

    #[test]
    fn the_caption_names_the_owner_and_says_when_they_are_away() {
        let mut ada = holder("Ada");
        assert_eq!(holder_caption(&ada, "Ada"), "Sky · you");
        assert_eq!(holder_caption(&ada, "Bo"), "Sky · Ada");
        ada.owner_away_since_ms = Some(1);
        assert_eq!(holder_caption(&ada, "Bo"), "Sky · Ada (away)");
    }
}
