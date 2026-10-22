//! The puzzle library: a local shelf of self-contained `.pieceoff` packages
//! with thumbnails, so a puzzle can be resumed or replayed without hunting for
//! its file. The index is a small JSON file; each entry owns one package and
//! one thumbnail named after its id.

use super::*;

pub(crate) const MAX_LIBRARY_ENTRIES: usize = 500;

const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;

const THUMBNAIL_EDGE: u32 = 240;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LibraryEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) rows: u16,
    pub(crate) columns: u16,
    pub(crate) is_video: bool,
    pub(crate) media_sha256: String,
    pub(crate) progress_percent: u8,
    pub(crate) added_ms: u64,
    pub(crate) updated_ms: u64,
    pub(crate) package_bytes: u64,
}

impl LibraryEntry {
    pub(crate) fn pieces(&self) -> u32 {
        u32::from(self.rows) * u32::from(self.columns)
    }

    pub(crate) fn finished(&self) -> bool {
        self.progress_percent >= 100
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LibrarySort {
    #[default]
    Recent,
    Title,
    Progress,
}

impl LibrarySort {
    pub(crate) const ALL: [Self; 3] = [Self::Recent, Self::Title, Self::Progress];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recently played",
            Self::Title => "Title",
            Self::Progress => "Progress",
        }
    }
}

/// The result of a background library job: the entry to record, or why it
/// failed.
pub(crate) struct LibraryJob {
    pub(crate) label: String,
    pub(crate) outcome: mpsc::Receiver<Result<LibraryEntry, String>>,
}

pub(crate) struct Library {
    pub(crate) dir: PathBuf,
    pub(crate) entries: Vec<LibraryEntry>,
}

pub(crate) fn library_dir() -> PathBuf {
    client_data_dir().join("library")
}

fn valid_entry_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn new_entry_id(media_sha256: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = Sha256::new();
    hasher.update(media_sha256.as_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hex(hasher.finalize())[..16].to_owned()
}

/// Write `bytes` to `path` through a flushed temporary file, so a crash never
/// leaves a half-written index.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        path.file_name().and_then(OsStr::to_str).unwrap_or("file"),
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

impl Library {
    pub(crate) fn empty(dir: PathBuf) -> Self {
        Self {
            dir,
            entries: Vec::new(),
        }
    }

    fn index_path(dir: &Path) -> PathBuf {
        dir.join("index.json")
    }

    pub(crate) fn package_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.pieceoff"))
    }

    pub(crate) fn thumbnail_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.png"))
    }

    /// Read the index, dropping entries that are malformed or whose package
    /// has gone missing. A damaged index yields an empty library plus the
    /// reason, rather than blocking the app.
    pub(crate) fn load(dir: PathBuf) -> (Self, Option<String>) {
        let path = Self::index_path(&dir);
        let mut library = Self::empty(dir);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (library, None),
            Err(error) => return (library, Some(error.to_string())),
        };
        if metadata.len() > MAX_INDEX_BYTES {
            return (library, Some("Library index exceeds its size limit".into()));
        }
        let parsed = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Vec<LibraryEntry>>(&bytes)
                    .map_err(|error| error.to_string())
            });
        match parsed {
            Ok(mut entries) => {
                entries.retain(|entry| {
                    valid_entry_id(&entry.id)
                        && !entry.title.trim().is_empty()
                        && entry.title.chars().count() <= 80
                        && !entry.title.chars().any(char::is_control)
                        && entry.progress_percent <= 100
                        && library.package_path(&entry.id).is_file()
                });
                entries.truncate(MAX_LIBRARY_ENTRIES);
                library.entries = entries;
                (library, None)
            }
            Err(error) => (
                library,
                Some(format!("Library index is unreadable: {error}")),
            ),
        }
    }

    fn save_index(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.entries).map_err(|error| error.to_string())?;
        write_atomic(&Self::index_path(&self.dir), &bytes)
    }

    /// Record (or replace) an entry whose files are already in place.
    pub(crate) fn upsert(&mut self, entry: LibraryEntry) -> Result<(), String> {
        let mut updated = self.entries.clone();
        match updated.iter_mut().find(|existing| existing.id == entry.id) {
            Some(existing) => {
                // Keep the original add date when progress is re-saved.
                let added_ms = existing.added_ms;
                *existing = LibraryEntry { added_ms, ..entry };
            }
            None => {
                if updated.len() >= MAX_LIBRARY_ENTRIES {
                    return Err(format!(
                        "The library is full ({MAX_LIBRARY_ENTRIES} puzzles). Delete one first."
                    ));
                }
                updated.push(entry);
            }
        }
        let previous = std::mem::replace(&mut self.entries, updated);
        if let Err(error) = self.save_index() {
            self.entries = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: &str) -> Result<(), String> {
        if !valid_entry_id(id) {
            return Err("Unknown library entry".into());
        }
        let mut updated = self.entries.clone();
        let before = updated.len();
        updated.retain(|entry| entry.id != id);
        if updated.len() == before {
            return Err("That puzzle is no longer in the library".into());
        }
        let previous = std::mem::replace(&mut self.entries, updated);
        if let Err(error) = self.save_index() {
            self.entries = previous;
            return Err(error);
        }
        let _ = std::fs::remove_file(self.package_path(id));
        let _ = std::fs::remove_file(self.thumbnail_path(id));
        Ok(())
    }

    /// Entries matching `query` (every word must appear in the title), in
    /// the requested order.
    pub(crate) fn filtered(&self, query: &str, sort: LibrarySort) -> Vec<&LibraryEntry> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|word| word.to_lowercase())
            .collect();
        let mut matches: Vec<&LibraryEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                let title = entry.title.to_lowercase();
                words.iter().all(|word| title.contains(word))
            })
            .collect();
        match sort {
            LibrarySort::Recent => matches.sort_by_key(|entry| std::cmp::Reverse(entry.updated_ms)),
            LibrarySort::Title => matches.sort_by_key(|entry| entry.title.to_lowercase()),
            LibrarySort::Progress => matches.sort_by_key(|entry| {
                (
                    std::cmp::Reverse(entry.progress_percent),
                    std::cmp::Reverse(entry.updated_ms),
                )
            }),
        }
        matches
    }
}

/// A small PNG preview of `media`, or `None` when none can be made (an
/// unreadable file, or video without FFmpeg). For video the frame comes from
/// `at_ms` so the shelf shows the part of the clip that is being played.
pub(crate) fn make_thumbnail(
    media: &Path,
    is_video: bool,
    at_ms: u64,
    ffmpeg: &OsStr,
) -> Option<Vec<u8>> {
    let decoded = if is_video && !is_animated_image_extension(media) {
        let seconds = format!("{:.3}", at_ms as f64 / 1000.0);
        let grab = |seek: &str| {
            Command::new(ffmpeg)
                .args(["-v", "error", "-ss", seek, "-i"])
                .arg(media)
                .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success() && !output.stdout.is_empty())
                .and_then(|output| image::load_from_memory(&output.stdout).ok())
        };
        grab(&seconds).or_else(|| grab("0"))?
    } else {
        image::open(media).ok()?
    };
    let small = decoded.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// Everything a worker needs to put a puzzle on the shelf.
pub(crate) struct SaveRequest {
    pub(crate) dir: PathBuf,
    /// Replace this entry's package (progress re-save) instead of adding one.
    pub(crate) existing_id: Option<String>,
    pub(crate) snapshot: RoomSnapshot,
    pub(crate) media_path: PathBuf,
    pub(crate) is_video: bool,
    pub(crate) ffmpeg: OsString,
}

/// Write a package and thumbnail for a live puzzle. Runs on a worker thread:
/// hashing and copying media can take seconds.
pub(crate) fn save_to_library(request: SaveRequest) -> Result<LibraryEntry, String> {
    let SaveRequest {
        dir,
        existing_id,
        snapshot,
        media_path,
        is_video,
        ffmpeg,
    } = request;
    let id = match existing_id {
        Some(id) if valid_entry_id(&id) => id,
        Some(_) => return Err("Unknown library entry".into()),
        None => new_entry_id(&snapshot.media_sha256),
    };
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let package = dir.join(format!("{id}.pieceoff"));
    write_puzzle_package(&package, &media_path, &snapshot)?;
    // The thumbnail only depends on the media, so keep an existing one.
    let thumbnail = dir.join(format!("{id}.png"));
    if !thumbnail.is_file() {
        if let Some(png) = make_thumbnail(
            &media_path,
            is_video,
            snapshot.playback.loop_start_ms,
            &ffmpeg,
        ) {
            let _ = write_atomic(&thumbnail, &png);
        }
    }
    let now = now_ms();
    Ok(LibraryEntry {
        id,
        title: snapshot.title.clone(),
        rows: snapshot.rows,
        columns: snapshot.columns,
        is_video,
        media_sha256: snapshot.media_sha256.clone(),
        progress_percent: snapshot.progress_percent(),
        added_ms: now,
        updated_ms: now,
        package_bytes: std::fs::metadata(&package)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
    })
}

/// Validate a `.pieceoff` file and copy it onto the shelf.
pub(crate) fn import_into_library(
    dir: PathBuf,
    source: PathBuf,
    ffmpeg: OsString,
) -> Result<LibraryEntry, String> {
    let (snapshot, media_path) = read_puzzle_package(&source)?;
    let is_video = inspect_media_basic(&media_path)
        .map(|metadata| metadata.is_video)
        .unwrap_or(false);
    let id = new_entry_id(&snapshot.media_sha256);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let package = dir.join(format!("{id}.pieceoff"));
    let partial = dir.join(format!(".{id}.part"));
    std::fs::copy(&source, &partial).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, &package).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        error.to_string()
    })?;
    if let Some(png) = make_thumbnail(
        &media_path,
        is_video,
        snapshot.playback.loop_start_ms,
        &ffmpeg,
    ) {
        let _ = write_atomic(&dir.join(format!("{id}.png")), &png);
    }
    let now = now_ms();
    Ok(LibraryEntry {
        id,
        title: snapshot.title.clone(),
        rows: snapshot.rows,
        columns: snapshot.columns,
        is_video,
        media_sha256: snapshot.media_sha256.clone(),
        progress_percent: snapshot.progress_percent(),
        added_ms: now,
        updated_ms: now,
        package_bytes: std::fs::metadata(&package)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pieceoff-library-test-{name}-{}-{}",
            std::process::id(),
            new_entry_id(name)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small PNG on disk plus a fresh puzzle snapshot for it.
    fn sample_puzzle(dir: &Path, title: &str, shade: u8) -> (PathBuf, RoomSnapshot) {
        let media = dir.join(format!("{title}.png"));
        image::RgbaImage::from_fn(64, 48, |x, y| {
            image::Rgba([shade, (x * 4) as u8, (y * 5) as u8, 255])
        })
        .save(&media)
        .unwrap();
        let hash = sha256_file(&media).unwrap();
        let snapshot = RoomSnapshot::create(
            "room".into(),
            "ABCD".into(),
            title.into(),
            hash,
            "png".into(),
            64,
            48,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        (media, snapshot)
    }

    fn save(dir: &Path, library: &Library, title: &str, shade: u8) -> LibraryEntry {
        let (media, snapshot) = sample_puzzle(dir, title, shade);
        save_to_library(SaveRequest {
            dir: library.dir.clone(),
            existing_id: None,
            snapshot,
            media_path: media,
            is_video: false,
            ffmpeg: OsString::from("ffmpeg"),
        })
        .unwrap()
    }

    #[test]
    fn saved_puzzles_survive_a_reload_with_thumbnails_and_progress() {
        let scratch = scratch_dir("reload");
        let shelf = scratch.join("shelf");
        let (mut library, notice) = Library::load(shelf.clone());
        assert!(notice.is_none() && library.entries.is_empty());

        let entry = save(&scratch, &library, "Harbour", 10);
        assert_eq!(entry.pieces(), 12);
        assert_eq!(entry.progress_percent, 0);
        assert!(library.thumbnail_path(&entry.id).is_file());
        library.upsert(entry.clone()).unwrap();

        let (reloaded, notice) = Library::load(shelf);
        assert!(notice.is_none());
        assert_eq!(reloaded.entries, vec![entry]);
    }

    #[test]
    fn resaving_progress_keeps_the_entry_and_its_added_date() {
        let scratch = scratch_dir("resave");
        let (mut library, _) = Library::load(scratch.join("shelf"));
        let (media, mut snapshot) = sample_puzzle(&scratch, "Forest", 40);
        let first = save_to_library(SaveRequest {
            dir: library.dir.clone(),
            existing_id: None,
            snapshot: snapshot.clone(),
            media_path: media.clone(),
            is_video: false,
            ffmpeg: OsString::from("ffmpeg"),
        })
        .unwrap();
        library.upsert(first.clone()).unwrap();

        // Snap the first row into place and save over the same entry.
        for piece in snapshot.pieces.iter_mut().take(4) {
            piece.placed = true;
        }
        let mut second = save_to_library(SaveRequest {
            dir: library.dir.clone(),
            existing_id: Some(first.id.clone()),
            snapshot,
            media_path: media,
            is_video: false,
            ffmpeg: OsString::from("ffmpeg"),
        })
        .unwrap();
        second.added_ms += 10_000;
        library.upsert(second).unwrap();

        assert_eq!(library.entries.len(), 1);
        assert_eq!(library.entries[0].progress_percent, 33);
        assert_eq!(library.entries[0].added_ms, first.added_ms);
    }

    #[test]
    fn search_matches_every_word_and_sort_orders_the_shelf() {
        let scratch = scratch_dir("search");
        let (mut library, _) = Library::load(scratch.join("shelf"));
        for (index, title) in ["Harbour at dawn", "Forest path", "Dawn over the forest"]
            .into_iter()
            .enumerate()
        {
            let mut entry = save(&scratch, &library, title, index as u8 * 30);
            entry.updated_ms = 1_000 + index as u64;
            entry.progress_percent = [50, 10, 90][index];
            library.upsert(entry).unwrap();
        }
        let titles = |query: &str, sort| {
            library
                .filtered(query, sort)
                .into_iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles("dawn forest", LibrarySort::Title),
            ["Dawn over the forest"]
        );
        assert_eq!(
            titles("FOREST", LibrarySort::Title),
            ["Dawn over the forest", "Forest path"]
        );
        assert!(titles("mountain", LibrarySort::Recent).is_empty());
        assert_eq!(
            titles("", LibrarySort::Recent),
            ["Dawn over the forest", "Forest path", "Harbour at dawn"]
        );
        assert_eq!(
            titles("", LibrarySort::Progress),
            ["Dawn over the forest", "Harbour at dawn", "Forest path"]
        );
    }

    #[test]
    fn removing_an_entry_deletes_its_files_and_unknown_ids_are_rejected() {
        let scratch = scratch_dir("remove");
        let (mut library, _) = Library::load(scratch.join("shelf"));
        let entry = save(&scratch, &library, "Lake", 5);
        library.upsert(entry.clone()).unwrap();
        assert!(library.remove("../../etc/passwd").is_err());
        assert!(library.remove("0123456789abcdef").is_err());
        library.remove(&entry.id).unwrap();
        assert!(library.entries.is_empty());
        assert!(!library.package_path(&entry.id).exists());
        assert!(!library.thumbnail_path(&entry.id).exists());
    }

    #[test]
    fn import_copies_a_package_onto_the_shelf() {
        let scratch = scratch_dir("import");
        let (media, snapshot) = sample_puzzle(&scratch, "Castle", 99);
        let package = scratch.join("castle.pieceoff");
        write_puzzle_package(&package, &media, &snapshot).unwrap();
        let entry = import_into_library(
            scratch.join("shelf"),
            package.clone(),
            OsString::from("ffmpeg"),
        )
        .unwrap();
        let (mut library, _) = Library::load(scratch.join("shelf"));
        library.upsert(entry.clone()).unwrap();
        assert_eq!(entry.title, "Castle");
        assert!(library.thumbnail_path(&entry.id).is_file());
        assert_eq!(
            std::fs::read(library.package_path(&entry.id)).unwrap(),
            std::fs::read(&package).unwrap()
        );
        // A file that is not a package is refused and leaves nothing behind.
        let bogus = scratch.join("bogus.pieceoff");
        std::fs::write(&bogus, b"not a package").unwrap();
        assert!(
            import_into_library(scratch.join("shelf"), bogus, OsString::from("ffmpeg")).is_err()
        );
        assert_eq!(std::fs::read_dir(scratch.join("shelf")).unwrap().count(), 3);
    }

    #[test]
    fn a_damaged_index_or_missing_package_does_not_block_the_library() {
        let scratch = scratch_dir("damaged");
        let shelf = scratch.join("shelf");
        let (mut library, _) = Library::load(shelf.clone());
        let kept = save(&scratch, &library, "Kept", 1);
        let lost = save(&scratch, &library, "Lost", 2);
        library.upsert(kept.clone()).unwrap();
        library.upsert(lost.clone()).unwrap();
        std::fs::remove_file(library.package_path(&lost.id)).unwrap();
        let (reloaded, notice) = Library::load(shelf.clone());
        assert!(notice.is_none());
        assert_eq!(reloaded.entries, vec![kept]);

        std::fs::write(shelf.join("index.json"), b"{ not json").unwrap();
        let (broken, notice) = Library::load(shelf);
        assert!(broken.entries.is_empty());
        assert!(notice.unwrap().contains("unreadable"));
    }

    /// Developer aid for `scripts/visual-dev.sh`: fill a client directory's
    /// library with sample puzzles. Run with
    /// `PIECEOFF_SEED_DIR=<client dir> cargo test seed_sample_library -- --ignored`.
    #[test]
    #[ignore = "writes sample data for screenshots"]
    fn seed_sample_library() {
        let client =
            PathBuf::from(std::env::var_os("PIECEOFF_SEED_DIR").expect("PIECEOFF_SEED_DIR"));
        let scratch = scratch_dir("seed");
        let (mut library, _) = Library::load(client.join("library"));
        let samples = [
            ("Harbour at dawn", 30, 0usize, 3u16, 4u16),
            ("Forest path", 120, 5, 6, 8),
            ("Mountain lake", 200, 12, 4, 6),
            ("Old castle", 70, 99, 3, 4),
            ("City lights", 160, 40, 8, 10),
        ];
        for (index, (title, shade, settled, rows, columns)) in samples.into_iter().enumerate() {
            let (media, _) = sample_puzzle(&scratch, title, shade);
            let hash = sha256_file(&media).unwrap();
            let mut snapshot = RoomSnapshot::create(
                "room".into(),
                "ABCD".into(),
                title.into(),
                hash,
                "png".into(),
                64,
                48,
                rows,
                columns,
                "host".into(),
            )
            .unwrap();
            let total = snapshot.pieces.len();
            for piece in snapshot.pieces.iter_mut().take(settled.min(total)) {
                piece.placed = true;
            }
            let mut entry = save_to_library(SaveRequest {
                dir: library.dir.clone(),
                existing_id: None,
                snapshot,
                media_path: media,
                is_video: index % 2 == 1,
                ffmpeg: OsString::from("ffmpeg"),
            })
            .unwrap();
            entry.updated_ms -= index as u64 * 86_400_000;
            library.upsert(entry).unwrap();
        }
    }
}
