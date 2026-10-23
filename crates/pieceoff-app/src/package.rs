use super::*;

/// The most this client will send or accept; each server enforces its own,
/// usually lower, limit.
pub(crate) const MAX_ROOM_MEDIA_BYTES: u64 = 64 * 1024 * 1024 * 1024;

pub(crate) const PIECEOFF_PACKAGE_MAGIC: &[u8; 8] = b"PCOFFPKG";

pub(crate) const PIECEOFF_PACKAGE_VERSION: u16 = 1;

/// Packages that also carry the media of every other queue item. The layout
/// is the version 1 layout followed by one `u64` length plus bytes per entry
/// of the manifest's `extra` list.
pub(crate) const PIECEOFF_PACKAGE_VERSION_QUEUE: u16 = 2;

pub(crate) const MAX_PACKAGE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn http_media_url(ws_url: &str, path_query: &str) -> Result<String, String> {
    let (scheme, rest) = ws_url
        .strip_prefix("ws://")
        .map(|rest| ("http", rest))
        .or_else(|| ws_url.strip_prefix("wss://").map(|rest| ("https", rest)))
        .ok_or("Room server URL must start with ws:// or wss://")?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('\r')
        || authority.contains('\n')
    {
        return Err("Invalid room server address".into());
    }
    if authority.contains(':') && !authority.starts_with('[') && authority.matches(':').count() > 1
    {
        return Err("IPv6 room server addresses must use brackets".into());
    }
    Ok(format!("{scheme}://{authority}{path_query}"))
}

/// How long a media transfer may stall. There is deliberately no limit on
/// the whole transfer: a multi-gigabyte file legitimately takes minutes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TransferTimeouts {
    pub(crate) connect: Duration,
    /// Longest wait for the next bytes.
    pub(crate) idle: Duration,
}

impl TransferTimeouts {
    pub(crate) const DEFAULT: Self = Self {
        connect: Duration::from_secs(15),
        idle: Duration::from_secs(60),
    };
}

/// Uploads stream a file through the blocking client: no total limit, and
/// keepalive so a dead peer is noticed.
fn media_upload_client(timeouts: TransferTimeouts) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(timeouts.connect)
        .tcp_keepalive(Duration::from_secs(30))
        .timeout(None)
        .build()
        .map_err(|error| error.to_string())
}

/// Downloads use the async client for its idle (read) timeout.
fn media_download_client(timeouts: TransferTimeouts) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(timeouts.connect)
        .read_timeout(timeouts.idle)
        .build()
        .map_err(|error| error.to_string())
}

/// Bytes moved so far by the current transfer in one direction, for the
/// status line. `total` is 0 while nothing is in flight.
pub(crate) struct TransferProgress {
    done: std::sync::atomic::AtomicU64,
    total: std::sync::atomic::AtomicU64,
}

impl TransferProgress {
    const fn new() -> Self {
        Self {
            done: std::sync::atomic::AtomicU64::new(0),
            total: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn begin(&self, done: u64, total: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    fn advance(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn finish(&self) {
        self.total.store(0, Ordering::Relaxed);
    }

    /// `(done, total)` while a transfer is running.
    pub(crate) fn current(&self) -> Option<(u64, u64)> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed).min(total), total))
    }
}

pub(crate) static UPLOAD_PROGRESS: TransferProgress = TransferProgress::new();
pub(crate) static DOWNLOAD_PROGRESS: TransferProgress = TransferProgress::new();

/// 7_800_000_000 reads as "7.8 GB".
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

struct CountingReader<R> {
    inner: R,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        UPLOAD_PROGRESS.advance(count as u64);
        Ok(count)
    }
}

pub(crate) fn upload_room_media(
    ws_url: &str,
    sha256: &str,
    token: &str,
    path: &Path,
) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length == 0 || length > MAX_ROOM_MEDIA_BYTES {
        return Err(format!(
            "Room media must be between 1 byte and {}",
            format_bytes(MAX_ROOM_MEDIA_BYTES)
        ));
    }
    // Ask first whether the server already holds this asset: it answers
    // without reading a body, so streaming one regardless ends in a broken
    // pipe. Older servers treat the empty probe as a failed upload; carry on.
    let probe = http_media_url(ws_url, &format!("/media/{sha256}?token={token}&probe=1"))?;
    let held = media_upload_client(TransferTimeouts::DEFAULT)
        .ok()
        .and_then(|client| {
            client
                .put(probe)
                .header(reqwest::header::CONTENT_LENGTH, 0)
                .body(Vec::new())
                .send()
                .ok()
        })
        .is_some_and(|response| response.status() == reqwest::StatusCode::CREATED);
    if held {
        return Ok(());
    }
    let url = http_media_url(ws_url, &format!("/media/{sha256}?token={token}"))?;
    UPLOAD_PROGRESS.begin(0, length);
    let result = media_upload_client(TransferTimeouts::DEFAULT).and_then(|client| {
        client
            .put(url)
            .header(reqwest::header::CONTENT_LENGTH, length)
            .body(reqwest::blocking::Body::new(CountingReader { inner: file }))
            .send()
            .map_err(|error| format!("Could not upload room media: {error}"))
    });
    UPLOAD_PROGRESS.finish();
    let response = result?;
    match response.status() {
        reqwest::StatusCode::CREATED => Ok(()),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => {
            Err("this room server does not accept media that large".into())
        }
        reqwest::StatusCode::INSUFFICIENT_STORAGE => {
            Err("the room server is out of media storage".into())
        }
        status => Err(format!(
            "room server rejected the media upload (HTTP {status})"
        )),
    }
}

pub(crate) fn download_room_media(
    ws_url: &str,
    invite_code: &str,
    sha256: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    download_room_media_with(
        ws_url,
        invite_code,
        sha256,
        extension,
        TransferTimeouts::DEFAULT,
    )
}

/// Like `download_room_media`, trying again after a dropped connection. Each
/// attempt resumes from the bytes already on disk.
pub(crate) fn download_room_media_retrying(
    ws_url: &str,
    invite_code: &str,
    sha256: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let mut delay = Duration::from_secs(1);
    let mut last_error = String::new();
    for attempt in 0..5 {
        match download_room_media(ws_url, invite_code, sha256, extension) {
            Ok(path) => return Ok(path),
            // A file that fails its hash will not get better by asking again.
            Err(error) if error.contains("SHA-256") || error.contains("invalid") => {
                return Err(error);
            }
            Err(error) => last_error = error,
        }
        if attempt < 4 {
            thread::sleep(delay);
            delay = (delay * 2).min(Duration::from_secs(15));
        }
    }
    Err(last_error)
}

pub(crate) fn download_room_media_with(
    ws_url: &str,
    invite_code: &str,
    sha256: &str,
    extension: &str,
    timeouts: TransferTimeouts,
) -> Result<PathBuf, String> {
    if !valid_media_hash(sha256) || supported_extension(extension).is_none() {
        return Err("Room sent invalid media metadata".into());
    }
    let cache_dir = std::env::var_os("PIECEOFF_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pieceoff-media-cache"));
    std::fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let path = cache_dir.join(format!("{sha256}.{extension}"));
    if path.is_file() && sha256_file(&path).as_deref() == Ok(sha256) {
        return Ok(path);
    }
    if invite_code.is_empty() || !invite_code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Room sent an invalid invite code".into());
    }
    // One partial file per hash, kept across attempts so a dropped
    // connection costs only what was not yet received.
    let temporary = cache_dir.join(format!(".{sha256}.part"));
    let url = http_media_url(ws_url, &format!("/media/{sha256}?code={invite_code}"))?;
    let client = media_download_client(timeouts)?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let mut have = std::fs::metadata(&temporary)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let mut response = loop {
            let mut request = client.get(url.clone());
            if have > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
            }
            let response = request
                .send()
                .await
                .map_err(|error| format!("Could not download room media: {error}"))?;
            match response.status() {
                reqwest::StatusCode::OK | reqwest::StatusCode::PARTIAL_CONTENT => break response,
                // Our partial file is not a prefix of anything the server has.
                reqwest::StatusCode::RANGE_NOT_SATISFIABLE if have > 0 => {
                    let _ = std::fs::remove_file(&temporary);
                    have = 0;
                }
                status => return Err(format!("room media download returned HTTP {status}")),
            }
        };
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let remaining_length = response
            .content_length()
            .ok_or("room media response did not include its size")?;
        let start = if resumed { have } else { 0 };
        let total = start + remaining_length;
        if remaining_length == 0 || total > MAX_ROOM_MEDIA_BYTES {
            return Err(format!(
                "room media must be between 1 byte and {}",
                format_bytes(MAX_ROOM_MEDIA_BYTES)
            ));
        }
        let mut hasher = Sha256::new();
        let mut file = if resumed {
            // Hash what we already have, then carry on after it.
            let mut existing =
                std::fs::File::open(&temporary).map_err(|error| error.to_string())?;
            let mut buffer = vec![0u8; 256 * 1024];
            let mut seen = 0u64;
            while seen < have {
                let count = existing
                    .read(&mut buffer)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                seen += count as u64;
            }
            if seen != have {
                let _ = std::fs::remove_file(&temporary);
                return Err("the partial download changed; starting over".into());
            }
            std::fs::OpenOptions::new()
                .append(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?
        } else {
            std::fs::File::create(&temporary).map_err(|error| error.to_string())?
        };
        DOWNLOAD_PROGRESS.begin(start, total);
        let result = async {
            let mut remaining = remaining_length;
            while remaining > 0 {
                let chunk = response
                    .chunk()
                    .await
                    .map_err(|error| format!("Could not download room media: {error}"))?
                    .ok_or_else(|| {
                        "room media response ended before all bytes arrived".to_owned()
                    })?;
                if chunk.len() as u64 > remaining {
                    return Err("room media response was longer than announced".to_owned());
                }
                hasher.update(&chunk);
                file.write_all(&chunk).map_err(|error| error.to_string())?;
                DOWNLOAD_PROGRESS.advance(chunk.len() as u64);
                remaining -= chunk.len() as u64;
            }
            file.sync_all().map_err(|error| error.to_string())
        }
        .await;
        DOWNLOAD_PROGRESS.finish();
        drop(file);
        // On an interruption the partial file stays for the next attempt.
        result?;
        if hex(hasher.finalize()) != sha256 {
            let _ = std::fs::remove_file(&temporary);
            return Err("downloaded media failed its SHA-256 check".to_owned());
        }
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        Ok(path.clone())
    })
}

pub(crate) fn valid_media_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn supported_extension(value: &str) -> Option<&'static str> {
    valid_media_extension(value)
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(hasher.finalize()))
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PuzzlePackageManifest {
    pub(crate) version: u16,
    pub(crate) snapshot: RoomSnapshot,
    /// SHA-256 of each trailing media entry, in order (version 2 only).
    #[serde(default)]
    pub(crate) extra: Vec<String>,
}

pub(crate) fn portable_snapshot(snapshot: &RoomSnapshot) -> RoomSnapshot {
    let mut portable = snapshot.clone();
    portable.id.clear();
    portable.invite_code.clear();
    portable.host_id.clear();
    portable.revision = 0;
    portable.media_available = false;
    portable.playback.playing = false;
    portable.playback.changed_at_ms = 0;
    // Keep the solve time and credits, banked and stopped.
    portable.stats.pause(
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    );
    for piece in &mut portable.pieces {
        piece.held_by = None;
    }
    portable
}

pub(crate) fn write_puzzle_package(
    destination: &Path,
    media_path: &Path,
    snapshot: &RoomSnapshot,
) -> Result<(), String> {
    write_puzzle_package_with_extras(destination, media_path, snapshot, &[])
}

/// Stream `length` bytes of `reader` into `output`, returning the SHA-256.
fn copy_hashed(
    reader: &mut impl Read,
    output: &mut impl Write,
    length: u64,
    truncated: &str,
) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let wanted = buffer.len().min(remaining as usize);
        let count = reader
            .read(&mut buffer[..wanted])
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err(truncated.to_owned());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hex(hasher.finalize()))
}

pub(crate) fn write_puzzle_package_with_extras(
    destination: &Path,
    media_path: &Path,
    snapshot: &RoomSnapshot,
    extras: &[(String, PathBuf)],
) -> Result<(), String> {
    let mut media_file = std::fs::File::open(media_path).map_err(|error| error.to_string())?;
    let media_length = media_file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    if media_length == 0 || media_length > MAX_ROOM_MEDIA_BYTES {
        return Err(format!(
            "Package media must be between 1 byte and {}",
            format_bytes(MAX_ROOM_MEDIA_BYTES)
        ));
    }
    if !valid_media_hash(&snapshot.media_sha256)
        || supported_extension(&snapshot.media_extension).is_none()
    {
        return Err("The selected media does not match this puzzle".into());
    }
    for (hash, _) in extras {
        if !valid_media_hash(hash)
            || snapshot.queue.iter().all(|item| item.sha256 != *hash)
            || *hash == snapshot.media_sha256
        {
            return Err("Package media does not match this puzzle's queue".into());
        }
    }
    let manifest = serde_json::to_vec(&PuzzlePackageManifest {
        version: if extras.is_empty() {
            PIECEOFF_PACKAGE_VERSION
        } else {
            PIECEOFF_PACKAGE_VERSION_QUEUE
        },
        snapshot: portable_snapshot(snapshot),
        extra: extras.iter().map(|(hash, _)| hash.clone()).collect(),
    })
    .map_err(|error| error.to_string())?;
    if manifest.len() > MAX_PACKAGE_MANIFEST_BYTES {
        return Err("Puzzle package manifest is too large".into());
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary =
        destination.with_extension(format!("pieceoff-{}-{stamp}.part", std::process::id()));
    let result = (|| {
        let mut output = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        output
            .write_all(PIECEOFF_PACKAGE_MAGIC)
            .and_then(|_| output.write_all(&(manifest.len() as u32).to_le_bytes()))
            .and_then(|_| output.write_all(&media_length.to_le_bytes()))
            .and_then(|_| output.write_all(&manifest))
            .map_err(|error| error.to_string())?;
        let written = copy_hashed(
            &mut media_file,
            &mut output,
            media_length,
            "Selected media changed while writing the package",
        )?;
        if written != snapshot.media_sha256 {
            return Err("The selected media does not match this puzzle".into());
        }
        for (hash, path) in extras {
            let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
            let length = file.metadata().map_err(|error| error.to_string())?.len();
            if length == 0 || length > MAX_ROOM_MEDIA_BYTES {
                return Err(format!(
                    "Package media must be between 1 byte and {}",
                    format_bytes(MAX_ROOM_MEDIA_BYTES)
                ));
            }
            output
                .write_all(&length.to_le_bytes())
                .map_err(|error| error.to_string())?;
            let written = copy_hashed(
                &mut file,
                &mut output,
                length,
                "Queued media changed while writing the package",
            )?;
            if written != *hash {
                return Err("A queued video does not match this puzzle".into());
            }
        }
        output.sync_all().map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn read_puzzle_package(path: &Path) -> Result<(RoomSnapshot, PathBuf), String> {
    read_puzzle_package_with_extras(path).map(|(snapshot, media, _)| (snapshot, media))
}

/// Install the package's media into the cache, returning the snapshot, the
/// current item's file, and every other queued item's `(sha256, file)`.
#[allow(clippy::type_complexity)]
pub(crate) fn read_puzzle_package_with_extras(
    path: &Path,
) -> Result<(RoomSnapshot, PathBuf, Vec<(String, PathBuf)>), String> {
    let mut package = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let total_length = package.metadata().map_err(|error| error.to_string())?.len();
    let mut magic = [0; 8];
    package
        .read_exact(&mut magic)
        .map_err(|_| "Package header is incomplete".to_owned())?;
    if &magic != PIECEOFF_PACKAGE_MAGIC {
        return Err("File is not a PieceOff puzzle package".into());
    }
    let mut header = [0; 12];
    package
        .read_exact(&mut header)
        .map_err(|_| "Package header is incomplete".to_owned())?;
    let manifest_length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let media_length = u64::from_le_bytes(header[4..].try_into().unwrap());
    if manifest_length == 0 || manifest_length > MAX_PACKAGE_MANIFEST_BYTES {
        return Err("Package manifest length is invalid".into());
    }
    if media_length == 0 || media_length > MAX_ROOM_MEDIA_BYTES {
        return Err(format!(
            "Package media must be between 1 byte and {}",
            format_bytes(MAX_ROOM_MEDIA_BYTES)
        ));
    }
    let expected_length = 20_u64
        .checked_add(manifest_length as u64)
        .and_then(|length| length.checked_add(media_length))
        .ok_or("Package length overflow")?;
    // Version 2 appends further entries; the exact length is checked below.
    if total_length < expected_length {
        return Err("Package is truncated or contains unexpected trailing data".into());
    }
    let mut manifest_bytes = vec![0; manifest_length];
    package
        .read_exact(&mut manifest_bytes)
        .map_err(|_| "Package manifest is truncated".to_owned())?;
    let manifest: PuzzlePackageManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
    if manifest.version != PIECEOFF_PACKAGE_VERSION
        && manifest.version != PIECEOFF_PACKAGE_VERSION_QUEUE
    {
        return Err(format!(
            "Unsupported puzzle package version {}",
            manifest.version
        ));
    }
    if manifest.version == PIECEOFF_PACKAGE_VERSION && total_length != expected_length {
        return Err("Package is truncated or contains unexpected trailing data".into());
    }
    let extra_hashes = manifest.extra.clone();
    let queue_media: Vec<(String, String)> = manifest
        .snapshot
        .queue
        .iter()
        .map(|item| (item.sha256.clone(), item.extension.clone()))
        .collect();
    if supported_extension(&manifest.snapshot.media_extension).is_none() {
        return Err("Package uses an unsupported media extension".into());
    }
    let snapshot = manifest
        .snapshot
        .rehost(String::new(), String::new(), String::new())?;

    let cache_dir = std::env::var_os("PIECEOFF_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pieceoff-media-cache"));
    std::fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let destination = cache_dir.join(format!(
        "{}.{}",
        snapshot.media_sha256, snapshot.media_extension
    ));
    let temporary = cache_dir.join(format!(
        ".{}.{}.part",
        snapshot.media_sha256,
        std::process::id()
    ));
    let install = |reader: &mut std::fs::File,
                   length: u64,
                   expected: &str,
                   temporary: &Path,
                   destination: &Path,
                   truncated: &str,
                   failed: &str|
     -> Result<(), String> {
        let mut output = std::fs::File::create(temporary).map_err(|error| error.to_string())?;
        let result = copy_hashed(reader, &mut output, length, truncated).and_then(|hash| {
            output.sync_all().map_err(|error| error.to_string())?;
            if hash == expected {
                Ok(())
            } else {
                Err(failed.to_owned())
            }
        });
        drop(output);
        if let Err(error) = result {
            let _ = std::fs::remove_file(temporary);
            return Err(error);
        }
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(temporary, destination).map_err(|error| error.to_string())
    };
    install(
        &mut package,
        media_length,
        &snapshot.media_sha256,
        &temporary,
        &destination,
        "Package media is truncated",
        "Package media failed its SHA-256 check",
    )?;

    let mut extras = Vec::new();
    for hash in &extra_hashes {
        let extension = queue_media
            .iter()
            .find(|(sha, _)| sha == hash)
            .map(|(_, extension)| extension.clone())
            .filter(|extension| valid_media_hash(hash) && supported_extension(extension).is_some())
            .ok_or("Package lists media that is not in its queue")?;
        let mut length = [0u8; 8];
        package
            .read_exact(&mut length)
            .map_err(|_| "Package media is truncated".to_owned())?;
        let length = u64::from_le_bytes(length);
        if length == 0 || length > MAX_ROOM_MEDIA_BYTES {
            return Err(format!(
                "Package media must be between 1 byte and {}",
                format_bytes(MAX_ROOM_MEDIA_BYTES)
            ));
        }
        let extra_destination = cache_dir.join(format!("{hash}.{extension}"));
        let extra_temporary = cache_dir.join(format!(".{hash}.{}.part", std::process::id()));
        install(
            &mut package,
            length,
            hash,
            &extra_temporary,
            &extra_destination,
            "Package media is truncated",
            "Package media failed its SHA-256 check",
        )?;
        extras.push((hash.clone(), extra_destination));
    }
    let position =
        std::io::Seek::stream_position(&mut package).map_err(|error| error.to_string())?;
    if position != total_length {
        return Err("Package is truncated or contains unexpected trailing data".into());
    }
    Ok((snapshot, destination, extras))
}

/// The progress counters above are process-wide, so a transfer in one test
/// overwrites the status line other tests assert on. Tests that move media,
/// or that check the status while a server is running, hold this lock.
#[cfg(test)]
pub(crate) fn transfer_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
