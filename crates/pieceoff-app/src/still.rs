//! Releasing a finished video to a completion still, and bringing it back
//! (protocol v17). The server owns the queue; this captures the last decoded
//! frame, uploads it with the host's token, then asks the room to adopt it.

use super::*;
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};

/// How long a finished puzzle keeps its video before the host's app releases it.
pub(crate) const AUTO_RELEASE_AFTER: Duration = Duration::from_secs(60 * 60);

/// JPEG qualities to try, best first, until the still fits the server's cap.
const STILL_QUALITIES: [u8; 4] = [92, 85, 75, 60];

/// Encode a decoded frame as a JPEG no larger than `MAX_STILL_BYTES`.
pub(crate) fn encode_still(frame: &ColorImage) -> Result<Vec<u8>, String> {
    let [width, height] = frame.size;
    let rgb: Vec<u8> = frame
        .pixels
        .iter()
        .flat_map(|pixel| [pixel.r(), pixel.g(), pixel.b()])
        .collect();
    for quality in STILL_QUALITIES {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, quality)
            .encode(&rgb, width as u32, height as u32, ExtendedColorType::Rgb8)
            .map_err(|error| format!("Could not encode the completion still: {error}"))?;
        if out.len() as u64 <= pieceoff_core::MAX_STILL_BYTES {
            return Ok(out);
        }
    }
    Err("The completion still is too large to upload".into())
}

impl PieceOffApp {
    /// The host may release the current video once the puzzle is finished: a
    /// frame is on screen, the room is on an axum server (the hosted service
    /// has no release yet) and we hold the upload token.
    pub(crate) fn can_release_video(&self) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        self.is_host()
            && self.online.session.is_none()
            && self.host_upload_token.is_some()
            && self.still_rx.is_none()
            && snapshot.completed()
            && self.last_video_frame.is_some()
            && snapshot
                .queue
                .get(snapshot.current)
                .is_some_and(|item| item.is_video() && item.available && !item.released())
    }

    /// Capture the frame, upload it on a worker thread, then
    /// `poll_still_release` sends `ReleaseToStill`.
    pub(crate) fn release_video_to_still(&mut self, automatic: bool) {
        if !self.can_release_video() {
            return;
        }
        let (Some(frame), Some(token), Some(snapshot)) = (
            self.last_video_frame.clone(),
            self.host_upload_token.clone(),
            self.snapshot.as_ref(),
        ) else {
            return;
        };
        let Ok(url) = self.resolved_server_url() else {
            return;
        };
        let index = snapshot.current;
        let (tx, rx) = mpsc::channel();
        self.still_rx = Some(rx);
        self.status = "Saving the completed puzzle as a picture…".into();
        let repaint = self.repaint.clone();
        thread::spawn(move || {
            let result = encode_still(&frame).and_then(|bytes| {
                let sha256 = hex(Sha256::digest(&bytes));
                crate::package::upload_room_still(&url, &sha256, "jpg", &token, bytes)?;
                Ok(ClientMessage::ReleaseToStill {
                    index,
                    sha256,
                    extension: "jpg".into(),
                    width: frame.size[0] as u32,
                    height: frame.size[1] as u32,
                    automatic,
                })
            });
            let _ = tx.send(result);
            if let Some(repaint) = repaint {
                repaint.request_repaint();
            }
        });
    }

    /// Once the puzzle has stood finished for `AUTO_RELEASE_AFTER`, the host's
    /// app releases the video on its own, marked automatic so the room cannot
    /// replay a video that is gone. Anyone who finishes then starts a new game.
    pub(crate) fn auto_release_finished_video(&mut self) {
        let finished =
            self.is_host() && self.snapshot.as_ref().is_some_and(RoomSnapshot::completed);
        if !finished {
            self.finished_since = None;
            return;
        }
        let since = *self.finished_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= AUTO_RELEASE_AFTER && self.can_release_video() {
            self.release_video_to_still(true);
        }
    }

    pub(crate) fn poll_still_release(&mut self) {
        let Some(rx) = &self.still_rx else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.still_rx = None;
                return;
            }
        };
        self.still_rx = None;
        match result {
            Ok(message) => {
                self.send(message);
                self.status = "Releasing the video; the picture stays in the room.".into();
            }
            Err(error) => self.status = error,
        }
    }

    /// The host can put a released item's video back, from the file this
    /// machine already holds or one it picks: the hash must match the original.
    pub(crate) fn restore_released_video(&mut self, index: usize) {
        let Some(item) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.queue.get(index))
        else {
            return;
        };
        if !item.released() {
            return;
        }
        let seed = item.seed_sha256.clone();
        let known = self.media_files.lock().unwrap().get(&seed).cloned();
        let path = known.or_else(|| {
            rfd::FileDialog::new()
                .set_title("Choose the original video")
                .add_filter("Video", pieceoff_core::SUPPORTED_MEDIA_EXTENSIONS)
                .pick_file()
        });
        let Some(path) = path else {
            return;
        };
        self.restore_rx = Some(spawn_restore_probe(index, seed, path, self.repaint.clone()));
        self.status = "Reading the video before bringing it back…".into();
    }

    pub(crate) fn poll_restore(&mut self) {
        let Some(rx) = &self.restore_rx else {
            return;
        };
        let (index, path, result) = match rx.try_recv() {
            Ok(done) => done,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.restore_rx = None;
                return;
            }
        };
        self.restore_rx = None;
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let extension = match supported_media_extension(&path) {
            Ok(extension) => extension,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        // The server then hands the host an upload token for this hash and
        // the network thread uploads from `media_files`.
        self.media_files
            .lock()
            .unwrap()
            .insert(metadata.sha256.clone(), path);
        self.send(ClientMessage::RestoreVideo {
            index,
            sha256: metadata.sha256,
            extension,
            width: metadata.size.0,
            height: metadata.size.1,
            duration_ms: metadata.duration_ms,
        });
        self.status = "Bringing the video back…".into();
    }
}

pub(crate) type RestoreProbe = (usize, PathBuf, Result<MediaMetadata, String>);

/// Hash `path` off the UI thread and refuse any file but the original.
fn spawn_restore_probe(
    index: usize,
    seed: String,
    path: PathBuf,
    repaint: Option<egui::Context>,
) -> mpsc::Receiver<RestoreProbe> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = inspect_media(&path).and_then(|metadata| {
            if metadata.sha256.eq_ignore_ascii_case(&seed) {
                Ok(metadata)
            } else {
                Err("That is not the original video for this puzzle.".into())
            }
        });
        let _ = tx.send((index, path, result));
        if let Some(repaint) = repaint {
            repaint.request_repaint();
        }
    });
    rx
}
