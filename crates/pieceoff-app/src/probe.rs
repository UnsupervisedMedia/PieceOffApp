use super::*;

pub(crate) struct ProbeResult {
    pub(crate) path: PathBuf,
    pub(crate) outcome: Result<ProbeOutcome, String>,
    /// Codecs in the file that the current FFmpeg cannot decode.
    pub(crate) undecodable: Vec<String>,
}

/// Which of the file's codecs the current FFmpeg lacks a decoder for. Asked
/// on the probe's worker thread, so it never waits on the UI's tool list.
pub(crate) fn undecodable_codecs(outcome: &Result<ProbeOutcome, String>) -> Vec<String> {
    let codecs = match outcome {
        Ok(ProbeOutcome::ChooseSetup { codecs, .. }) => codecs.as_slice(),
        Ok(ProbeOutcome::Ready(metadata)) => metadata.codecs.as_slice(),
        Err(_) => return Vec::new(),
    };
    if codecs.is_empty() {
        return Vec::new();
    }
    media_tools::decoder_names(&media_tools::ffmpeg())
        .map(|decoders| media_tools::undecodable(&decoders, codecs))
        .unwrap_or_default()
}

pub(crate) enum ProbeOutcome {
    Ready(MediaMetadata),
    /// A video: offer the quick start (full video or a loop).
    ChooseSetup {
        duration_ms: u64,
        has_audio: bool,
        size: (u32, u32),
        /// ffprobe's codec name for every stream, to see what must be decoded.
        codecs: Vec<String>,
    },
}

#[derive(Debug)]
pub(crate) struct MediaMetadata {
    pub(crate) size: (u32, u32),
    pub(crate) duration_ms: u64,
    pub(crate) is_video: bool,
    pub(crate) has_audio: bool,
    pub(crate) sha256: String,
    /// ffprobe's codec name for every stream (empty for still images).
    pub(crate) codecs: Vec<String>,
}

pub(crate) fn supported_media_extension(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if supported_extension(&extension).is_some() {
        Ok(extension)
    } else {
        Err("Use a PNG/APNG, JPEG, WebP, GIF, or FFmpeg-supported video file (MP4, MOV, MKV, WebM, AVI, M4V, MPEG, OGV, FLV, 3GP, WMV, TS, MTS, or M2TS).".into())
    }
}

pub(crate) fn image_to_color(image: image::DynamicImage) -> ColorImage {
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

pub(crate) fn capture_still_frame(frame: &ColorImage) -> ColorImage {
    frame.clone()
}

/// `skip_setup` is set for files PieceOff just produced (a cut clip) or that
/// the player already chose to use whole.
pub(crate) fn probe_media(path: &Path, skip_setup: bool) -> Result<ProbeOutcome, String> {
    let metadata = inspect_media_basic(path)?;
    if metadata.is_video && !skip_setup && !is_animated_image_extension(path) {
        return Ok(ProbeOutcome::ChooseSetup {
            duration_ms: metadata.duration_ms,
            has_audio: metadata.has_audio,
            size: metadata.size,
            codecs: metadata.codecs,
        });
    }
    Ok(ProbeOutcome::Ready(hash_media(path, metadata)?))
}

/// Animated WebP/GIF/APNG loops are already short and have no timeline to cut.
pub(crate) fn is_animated_image_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|x| x.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("webp" | "gif" | "png" | "apng")
    )
}

pub(crate) fn inspect_media(path: &Path) -> Result<MediaMetadata, String> {
    hash_media(path, inspect_media_basic(path)?)
}

pub(crate) fn inspect_media_basic(path: &Path) -> Result<MediaMetadata, String> {
    let extension = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let raster_dimensions = if matches!(
        extension.as_str(),
        "png" | "apng" | "jpg" | "jpeg" | "webp" | "gif"
    ) {
        let dimensions = image::image_dimensions(path)
            .map_err(|error| format!("Could not read image dimensions: {error}"))?;
        if !valid_media_dimensions(dimensions.0, dimensions.1) {
            return Err(
                "Images must be at most 8192 pixels per side and 40 megapixels total.".into(),
            );
        }
        Some(dimensions)
    } else {
        None
    };
    let webp_metadata = if extension == "webp" {
        Some(webp_animation_metadata(path)?)
    } else {
        None
    };
    let animated_webp = webp_metadata.as_ref().filter(|(animated, _, _)| *animated);
    let is_image = match extension.as_str() {
        "png" | "apng" => !png_is_animated(path)?,
        "jpg" | "jpeg" => true,
        "gif" => !gif_is_animated(path)?,
        "webp" => webp_metadata
            .as_ref()
            .is_some_and(|(animated, _, _)| !animated),
        _ => false,
    };
    let mut codecs = Vec::new();
    let (size, duration, is_video, has_audio) = if let Some((_, size, duration)) = animated_webp {
        (*size, *duration, true, false)
    } else if is_image {
        (
            raster_dimensions.expect("recognized image has dimensions"),
            0,
            false,
            false,
        )
    } else {
        let output = Command::new(media_tools::ffprobe())
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,codec_name,width,height:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .map_err(|_| {
                "FFmpeg was not found. Install it (the ffmpeg and ffprobe commands), or put both next to PieceOff, to use video puzzles.".to_owned()
            })?;
        if !output.status.success() {
            return Err("FFprobe could not read this video file.".into());
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Invalid video metadata: {error}"))?;
        let (size, duration, has_audio) = parse_video_metadata(&value)?;
        codecs = parse_stream_codecs(&value);
        (size, duration, true, has_audio)
    };
    if !valid_media_dimensions(size.0, size.1) {
        return Err("Media must be at most 8192 pixels per side and 40 megapixels total.".into());
    }
    Ok(MediaMetadata {
        size,
        duration_ms: duration,
        is_video,
        has_audio,
        sha256: String::new(),
        codecs,
    })
}

pub(crate) fn hash_media(
    path: &Path,
    mut metadata: MediaMetadata,
) -> Result<MediaMetadata, String> {
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("Could not open media: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not read media: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    metadata.sha256 = hex(hasher.finalize());
    Ok(metadata)
}

pub(crate) fn png_is_animated(path: &Path) -> Result<bool, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    image::codecs::png::PngDecoder::new(BufReader::new(file))
        .and_then(|decoder| decoder.is_apng())
        .map_err(|error| format!("Could not inspect PNG animation: {error}"))
}

pub(crate) fn gif_is_animated(path: &Path) -> Result<bool, String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(file))
        .map_err(|error| format!("Could not inspect GIF animation: {error}"))?;
    let mut frames = decoder.into_frames();
    let _first = frames
        .next()
        .transpose()
        .map_err(|error| format!("Could not read GIF frame: {error}"))?;
    frames
        .next()
        .transpose()
        .map(|frame| frame.is_some())
        .map_err(|error| format!("Could not read GIF frame: {error}"))
}

pub(crate) fn webp_animation_metadata(path: &Path) -> Result<(bool, (u32, u32), u64), String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(file))
        .map_err(|error| format!("Could not inspect WebP animation: {error}"))?;
    let dimensions = image::ImageDecoder::dimensions(&decoder);
    if !decoder.has_animation() {
        return Ok((false, dimensions, 0));
    }
    let mut duration_ms = 0_u64;
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|error| format!("Could not read WebP animation: {error}"))?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = if denominator == 0 {
            1
        } else {
            (numerator as u64).div_ceil(denominator as u64).max(1)
        };
        duration_ms = duration_ms.saturating_add(delay_ms);
    }
    if duration_ms == 0 {
        return Err("Animated WebP contains no frames".into());
    }
    Ok((true, dimensions, duration_ms))
}

/// ffprobe's `codec_name` for every stream that has one.
pub(crate) fn parse_stream_codecs(value: &serde_json::Value) -> Vec<String> {
    value
        .get("streams")
        .and_then(|x| x.as_array())
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| stream.get("codec_name").and_then(|x| x.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_video_metadata(
    value: &serde_json::Value,
) -> Result<((u32, u32), u64, bool), String> {
    let streams = value
        .get("streams")
        .and_then(|x| x.as_array())
        .ok_or("No video stream found")?;
    let stream = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(|x| x.as_str()) == Some("video"))
        .ok_or("No video stream found")?;
    let has_audio = streams
        .iter()
        .any(|stream| stream.get("codec_type").and_then(|x| x.as_str()) == Some("audio"));
    let width = stream
        .get("width")
        .and_then(|x| x.as_u64())
        .ok_or("Could not read video width")? as u32;
    let height = stream
        .get("height")
        .and_then(|x| x.as_u64())
        .ok_or("Could not read video height")? as u32;
    let seconds = value
        .get("format")
        .and_then(|x| x.get("duration"))
        .and_then(|x| x.as_str())
        .and_then(|x| x.parse::<f64>().ok())
        .unwrap_or(0.0);
    Ok((
        (width, height),
        (seconds.max(0.0) * 1000.0) as u64,
        has_audio,
    ))
}
