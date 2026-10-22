# Manual multiplayer test

Use two machines on the same trusted network, or two app instances on one
machine. For a direct trusted-network test, allow TCP port `28878` through the
host firewall. For remote play, place the server behind an HTTPS reverse proxy
and set the clients' Server field to its `wss://` address. The app will use
HTTPS for media transfer automatically.

1. Start `pieceoff-server` on the host machine.
2. Open `pieceoff-app`, choose a short video or image, set the server address,
   then create a room. For video and animated APNG/GIF, install FFmpeg tools
   (`ffmpeg` and `ffprobe`) on each client. Animated WebP
   and still images use the native Rust decoder.
   The row/column controls allow grids up to 400 pieces and limit each choice
   so the total stays within that cap.
3. Wait for the media upload to finish and share the room code.
4. On another client, enter the server WebSocket address and room code, then
   choose **Join**. The guest may leave local media unselected; the app fetches
   and verifies the room media automatically.
5. Drag pieces from the loose-piece tray onto the puzzle board on both clients
   and confirm movement and snapping appear on both. Ctrl-click multiple loose
   pieces and drag the selection; the other player should see the same movement
   and be prevented from taking the lease. The host should test play, pause,
   seek, loop range, and playback speed.
6. Briefly disconnect a client, then restore the connection. PieceOff retries
   automatically with increasing delays and rejoins the saved room. You can
   also choose **Rejoin** manually after a connection failure. Separately,
   close and reopen a client, enter the same room code, and choose **Rejoin**;
   room state and shared playback should synchronize from the server.
7. Stop and restart the server, then repeat the join check. Room snapshots and
   uploaded media persist under the server data directory by default.
   To check interrupted-upload recovery, stop the server before the first media
   upload finishes, restart it with the same data directory, and rejoin from
   the host client with the same local media selected. The room host should
   receive a fresh upload token and finish uploading the asset.
8. While the puzzle is open, choose **Save package…**. On a client pointed at
   another server, choose **Open package…** and create a room. Confirm the
   embedded media loads and the solved pieces, loose-piece layout, and paused
   video playhead are preserved under a new invite code.

9. Open **Videos** and add a second video. Wait for "uploading…" to clear,
   move a few pieces, then choose the second video. Everyone should switch at
   the same time and the pieces must stay where they were. Select the first
   video again; it resumes where it was left.
10. Set repeat to **All** and seek near the end of the playing video. The room
    should move on to the next video by itself, and nobody should see the
    first seconds of the same clip flash first. Set repeat to **Off** and let
    the last video end; playback should pause on its last frame.
11. Tick **Lock video**. The host's and guests' queue controls should be
    disabled and Next / Previous video should do nothing until unlocked.
12. Save a package with two videos, open it on another client, and confirm
    both videos are in the queue.

13. With a video of several gigabytes, create a room and watch the status line
    count bytes while it uploads; other rooms on the same server should keep
    uploading meanwhile. Pull the network cable on a guest mid-download and
    restore it: the download should continue from where it stopped, not start
    over. Seek around the loop controls once playing; FFmpeg seeks by keyframe,
    so jumps should land within a second or two, not stall.
14. Move the pointer over the table on one client. The other should see an
    arrow with that player's name follow it smoothly, sitting on any piece
    being dragged. Move the pointer off the board or switch to another window:
    the cursor should vanish at once. Untick More → **Show other players'
    cursors** and they should stop being drawn, and stay off after a restart.
15. Press Enter on one client, type a line, and press Enter: it should appear on
    both clients with that player's name in their cursor colour, and the other
    client's collapsed **Chat** button should count it as new. Press Esc while
    typing: the box lets go of the keyboard and shortcuts work again. Point at
    the table and press P: both clients should see a ring with the name for
    about 3 s. Join a third client: it should get the earlier chat. Leave and
    rejoin: your cursor, chat, and ping colour should not change.
16. Before anyone picks up a piece the progress readout shows no time. Pick one
    up: both clients' clocks start and agree to the second. Have everyone
    leave for a minute and come back: the clock resumes where it stopped.
    Finish the puzzle: both clocks stop at the same time and the card lists
    each player's pieces, most first, in their colours. **Play again** clears
    it. Untick More → **Show solve timer** to hide the readout clock. Finish
    a puzzle resumed from the library and its card on the shelf shows the
    best time.
17. With sound on, snap a piece into place, join two pieces, chat, and ping:
    each should make its own short sound on both clients (chat only on the
    other one), and finishing should chime once. Arrange the tray, restart,
    and rejoin: all silent. More → **Effects** changes their volume without
    touching the clip's volume, and works in a still-image room.

For a one-machine smoke test, run a server and two app instances, choose a
different local WebSocket address in both apps, and join the room from the
second instance. GUI, audio/video codec, and cross-platform checks must be run
on the target OS; automated builds do not verify those runtime integrations.

The automated server integration test
`two_clients_share_video_playback_and_saved_puzzle_progress` generates a short
MP4 with FFmpeg, creates a room, uploads it as host, downloads the verified
asset as guest, shares a piece move and host play/pause/seek/rate/loop updates
with both WebSocket clients, then verifies the moved piece and paused playback
state survive reload. The app test
`ffmpeg_probes_decodes_loops_and_seeks_a_generated_video` separately checks
decoded frames, loop wrap, and seeking. These use local generated media; they do
not replace a GUI/audio-device playtest with real users.

## Silent test runs

`cargo test` never plays audio: audio output, sound effects included, is
disabled under `cfg(test)`; effects are tested by inspecting their samples and
which events trigger them. For
visual/GUI runs use `scripts/visual-dev.sh`, which forces `PIECEOFF_NO_AUDIO=1`.
When generating test clips by hand, use no audio track (`-an`) or a silent one
(`-f lavfi -i anullsrc`), never a tone.
Audio plays inside the app process, so nothing can keep sounding after it exits.

## Automated end-to-end suite

`cargo test -p pieceoff-app e2e::` runs the multiplayer video slice without
people or a window: a real room server runs in the test process, two or three
real app instances run the same frame function the window calls
(`PieceOffApp::tick`), and real FFmpeg decodes a generated silent video. Players
talk to the server over real WebSockets and HTTP. It needs `ffmpeg` and
`ffprobe` on `PATH` (tests skip with a message otherwise) and takes about six
seconds. Covered:

- Create a room from a video, upload it, join from a second client that has no
  copy, download and verify it, and decode on both.
- The host sets a loop, plays, pauses, and seeks; the guest receives each, both
  show moving frames while playing, and the paused guest shows none.
- Both players solve the puzzle by moving alternating pieces.
- The guest leaves and rejoins from its recent rooms with progress, loop, and
  media intact.
- The server stops and restarts with both players connected: both reconnect on
  their own with progress, loop, and media intact, and playback resumes.
- A server crash (no shutdown save): every dropped piece survives, a piece being
  carried is freed, and it can be picked up again.
- A room the server no longer has (expired): the client stops retrying, says so,
  and keeps its local puzzle so it can be saved.
- The host's connection dies mid-upload (a throttled proxy is cut): a guest who
  has the file is made host, finishes the upload, a later joiner downloads a
  verified copy, and no partial file is left on the server.
- FFmpeg killed mid-playback is restarted by the worker; a decoder that gave up
  after its retries is restarted by the app once FFmpeg works again; a hung FFmpeg
  is killed by a watchdog; a video that decodes to nothing is reported, not
  retried forever.

Not covered, because it needs people or other machines: whether two listeners
hear the same moment (audio is off in every automated run), real network paths
and latency between machines, real pointer and keyboard input in two windows
(drag and snap are covered separately by `DragRig`), and Windows and macOS.

## Playtest log

### 2026-10-08: automated two-client video run (real server, real FFmpeg, silent)

Ran the suite above. It found and fixed four defects that the 2026-10-05 manual
run could not have shown:

- **Events overtaking each other.** The server broadcast a change after
  releasing the room lock, so two concurrent changes could reach clients in the
  wrong order, and a client discards anything older than what it has. Under load
  the host never learned its media upload had finished (the status stayed on
  "Uploading media…"), and two players' piece moves could in principle drop one
  another. Events are now sent under the lock, in revision order; a regression
  test fails three times in six against the old code.
- **Reconnect forever.** A client reconnecting to a room the server had expired
  retried every 30 seconds without end. It now stops and says the room is gone.
- **A hung FFmpeg froze the worker**, which then ignored seeks and leaving. A
  watchdog kills an FFmpeg stuck in a read for 10 s.
- **A decoder that gave up stayed dead** unless the host happened to send a
  playback update. The app now restarts it every 2 s within a small budget, and
  More → Restart video is the manual way out.

The gate for milestone 3 (create, solve, leave, rejoin a video puzzle with
progress and loop settings intact) passes in this setup, including across a
server restart and a crash.


### 2026-10-05: scripted two-client run (virtual displays, silent)

Host and guest ran as two `pieceoff-app` processes on separate Xvfb displays
against one local server (`scripts/visual-dev.sh`; guest on a second display).

Verified:

- Host picks a video, chooses "Use the full video", creates a room; media
  uploads and the room auto-plays its loop.
- Guest joins by invite code with no local media, downloads and verifies the
  media, and shows the loop in step with the host (position readout matches).
- A piece dragged by the host moves from the tray onto the board on the guest
  without any guest input (this exposed that network threads did not wake the
  UI; fixed with `WakingSender`).
- Host restart and "Play again" scatter all pieces for everyone, including when
  the host was promoted after a server restart.

Covered later by the automated suite (2026-10-08, above): reconnect after a drop,
host handoff during an upload, and progress recovery after a server restart.

Still needing people and an audio device:

- Audible audio sync between two machines, and audio after seek/loop wrap.
- Play on two real machines over a real network, on each operating system.
