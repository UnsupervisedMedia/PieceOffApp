# Feature matching matrix

This is a product checklist, not a code map. It records behaviors assessed during product research and distinguishes PieceOff implementation status from the target experience.

| Area | Existing behavior | PieceOff target | Priority |
| --- | --- | --- | --- |
| Multiplayer | Shared invite-code rooms, authoritative server state, piece leases, persistence, and reconnect | Implemented prototype: invite code, shared state, presence, exclusive piece leases, multi-selection, bounded automatic reconnect/rejoin, reload | P0 |
| Playback sync | Host controls shared playback; clients follow room playback state and clock | Prototype: host play/pause, seek, loop range, speed, and timestamp; audio follows; clients periodically correct A/V drift beyond a threshold; two-client protocol tests cover host control and persistence, and an end-to-end suite runs real decoders on two clients through reconnects and a server restart | P0/P1 |
| Video queue | One media item per puzzle | Implemented: the room holds up to 64 videos; the host adds, reorders, removes, and selects them at any time (hot swap keeps puzzle progress), repeat Off / This video / All with server-side advance, an optional video lock, and Next / Previous / Cycle repeat key bindings; packages carry every queued video | P1 |
| Source media | Still images, animated images, and local video files; room media can be uploaded and shared | Prototype: still images, local videos with optional audio, and animated APNG/GIF/WebP played as loops | P0 |
| Video playback | Shared play/pause, seek, loop range, and speed; local volume/mute and audio delay | Prototype supports seek, loop range, speed, local audio volume/mute and delay, selectable decode quality, and still-frame assist; subtitles remain | P0/P1 |
| Video portability | Puzzle manifest stores `VideoSource` path | Prototype supports server upload/download and self-contained `.pieceoff` packages with SHA-256-verified media and progress rehosting; aggregate quota and inactive-room media retention cleanup are implemented | P0 |
| Creation | Wizard and selectable slicer plugins | Simple creator first; extensible slicer interface | P0/P1 |
| Slicing | Rectangular, jigsaw, Goldberg variants | Deterministic jigsaw tabs with matching shared contours; configurable rectangular row/column grid up to 400 pieces; Goldberg variants remain | P0/P1/P2 |
| Core play | Drag, merge, selection, pan/zoom | Prototype has shaped-piece picking, drag, linked groups, snapping, Ctrl-click multi-selection with atomic shared movement leases, and board pan/zoom | P0 |
| Large puzzles | Piece holders, rearrange, fast zoom | Shared loose-piece tray supports local edge/interior filters, fit/board/tray focus, center zoom controls, and a shared Arrange command (edges first or shuffle) that re-lays loose pieces into free tray slots; large-board rendering is budgeted (levels of detail, unstroked specks, rate-limited drag updates; a vertex-budget test guards a 400-piece table) | P1 |
| Controls | Configurable shortcuts and mouse | Implemented: Controls window (More → Controls…, or the start screen) to rebind every keyboard action with conflict detection, reset, and unbind; wheel speed and inversion, zoom step, multi-select modifier (Ctrl/Cmd or Alt), empty-space drag panning, and a middle- or right-button pan; saved per device, with tooltips and the shortcut sheet following the bindings | P1 |
| Restart | Restart a puzzle | Host restart with confirmation and a completion-card Play again; other players are told when the host restarts; a library puzzle can also be replayed with a new cut (New game) | P1 |
| Collection | Browse, import, export, delete, restart | Implemented: local library of `.pieceoff` packages with thumbnails, title search, sorting, progress, resume, new game from the same media, import, export, delete, and progress saved on leaving a resumed room; recent-room history with quick rejoin and forget | P1 |
| Built-in puzzles | Ready-to-play stock media included with the app | Implemented: eight original, silent procedural animated-WebP loops (psychedelic patterns, ocean, aurora, desert, botanical, glass, underwater, and geometric scenes); all bundled and available from the library start screen, with recipes and licensing documented in `BUILTIN_MEDIA.md` | P1 |
| Preview | Local still-image reference and video still-frame assist | Local still-image reference view and local video still-frame assist (playback continues) | P1 |
| Progress | Autosave to per-puzzle state | Implemented server snapshots on drop/playback; migration and richer recovery remain | P0 |
| Portability | Desktop integration | Linux, Windows, macOS desktop packages | P1 |
| Legacy files | Import and export from other puzzle formats | No legacy format support committed; consider only with a documented format and rights review | P2 |

P0 = first multiplayer/video release; P1 = parity release; P2 = later or conditional.
