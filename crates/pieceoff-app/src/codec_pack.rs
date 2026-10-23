//! Installing a full FFmpeg on request.
//!
//! Release bundles carry an FFmpeg without the patent-encumbered decoders
//! (H.264, HEVC, AAC and friends; see docs/FFMPEG.md). When a player opens a
//! video that needs them, the app offers to download a full build from a
//! third party, straight to their machine. PieceOff does not host or
//! redistribute those binaries: it fetches a pinned file from the URL below,
//! checks its SHA-256, and unpacks it into the player's own data folder.

use crate::media_tools::Tool;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A downloaded file is never larger than this once unpacked.
const MAX_UNPACKED_BYTES: u64 = 400 * 1024 * 1024;

/// What to fetch for one platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pack {
    /// Where it comes from, for the consent text.
    pub(crate) host: String,
    /// What it is, for the consent text.
    pub(crate) description: String,
    pub(crate) license: String,
    pub(crate) files: Vec<PackFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackFile {
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) extract: Extract,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Extract {
    /// A gzip-compressed single program.
    Gzip(Tool),
    /// Named members of a zip archive, each saved as the given program.
    Zip(Vec<(String, Tool)>),
}

fn gzip_file(url: &str, sha256: &str, size: u64, tool: Tool) -> PackFile {
    PackFile {
        url: url.to_owned(),
        sha256: sha256.to_owned(),
        size,
        extract: Extract::Gzip(tool),
    }
}

impl Pack {
    /// The pack for the machine running the app, if there is one.
    pub(crate) fn current() -> Option<Self> {
        Self::for_target(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Every pin is a file in a GitHub release with a fixed tag, so it does
    /// not rot. To update, pick a new tag, copy the SHA-256 GitHub shows for
    /// each asset (or compute it), and run the manifest tests.
    pub(crate) fn for_target(os: &str, arch: &str) -> Option<Self> {
        const STATIC: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
        let static_pack = |platform: &str, digests: [(&str, u64); 2]| {
            let [(ffmpeg_sha, ffmpeg_size), (ffprobe_sha, ffprobe_size)] = digests;
            Self {
                host: "github.com/eugeneware/ffmpeg-static".into(),
                description: "a static FFmpeg build, release b6.1.1".into(),
                license: "GPL v3".into(),
                files: vec![
                    gzip_file(
                        &format!("{STATIC}/ffmpeg-{platform}.gz"),
                        ffmpeg_sha,
                        ffmpeg_size,
                        Tool::Ffmpeg,
                    ),
                    gzip_file(
                        &format!("{STATIC}/ffprobe-{platform}.gz"),
                        ffprobe_sha,
                        ffprobe_size,
                        Tool::Ffprobe,
                    ),
                ],
            }
        };
        match (os, arch) {
            ("windows", "x86_64") => Some(Self {
                host: "github.com/GyanD/codexffmpeg (gyan.dev)".into(),
                description: "FFmpeg 9.0.2, essentials build".into(),
                license: "GPL v3".into(),
                files: vec![PackFile {
                    url: "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip".into(),
                    sha256: "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba".into(),
                    size: 114_768_076,
                    extract: Extract::Zip(vec![
                        ("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe".into(), Tool::Ffmpeg),
                        ("ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe".into(), Tool::Ffprobe),
                    ]),
                }],
            }),
            ("linux", "x86_64") => Some(static_pack(
                "linux-x64",
                [
                    ("bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa", 29_354_986),
                    ("25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067", 29_276_839),
                ],
            )),
            ("linux", "aarch64") => Some(static_pack(
                "linux-arm64",
                [
                    ("754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0", 25_568_691),
                    ("2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8", 25_493_573),
                ],
            )),
            ("macos", "aarch64") => Some(static_pack(
                "darwin-arm64",
                [
                    ("8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa", 19_246_198),
                    ("d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be", 19_207_077),
                ],
            )),
            ("macos", "x86_64") => Some(static_pack(
                "darwin-x64",
                [
                    ("929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106", 25_296_431),
                    ("d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186", 25_239_438),
                ],
            )),
            _ => None,
        }
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// The text the player agrees to before anything is downloaded.
    pub(crate) fn consent_text(&self) -> String {
        format!(
            "PieceOff does not include the H.264, HEVC and AAC decoders that most \
             MP4, MOV and MKV videos need, because they are covered by patents \
             that PieceOff does not license.\n\n\
             Install them now? PieceOff will download {} (about {} MB) from {} \
             and keep it in its own data folder. It is third-party software under \
             the {}; PieceOff checks its checksum before using it.\n\n\
             Using these codecs may require a patent license where you live. \
             That is your responsibility, as with any media player you install \
             yourself.",
            self.description,
            self.total_bytes().div_ceil(1_000_000),
            self.host,
            self.license,
        )
    }
}

/// The folder an installed pack lives in.
pub(crate) fn install_dir() -> PathBuf {
    crate::storage::client_data_dir().join("codecs")
}

/// Download, verify, unpack and sanity-check `pack` into `dest`, replacing
/// whatever was there. Nothing is left behind on failure. `progress` gets
/// `(bytes so far, total bytes)`; set `cancel` to stop.
pub(crate) fn install(
    pack: &Pack,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let staging = dest.with_extension("installing");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|error| format!("Could not create a folder: {error}"))?;
    let result = install_into(pack, &staging, cancel, &mut progress).and_then(|()| {
        let _ = fs::remove_dir_all(dest);
        fs::rename(&staging, dest).map_err(|error| format!("Could not finish installing: {error}"))
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn install_into(
    pack: &Pack,
    staging: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    let total = pack.total_bytes();
    let mut finished = 0;
    for (index, file) in pack.files.iter().enumerate() {
        let archive = staging.join(format!("download-{index}.part"));
        download(&file.url, &file.sha256, &archive, cancel, |bytes| {
            progress(finished + bytes.min(file.size), total);
        })?;
        extract(&archive, &file.extract, staging)?;
        let _ = fs::remove_file(&archive);
        finished += file.size;
        progress(finished, total);
    }
    verify_tools(staging)?;
    let mut notes = format!(
        "Installed by PieceOff from {} ({}, {}).\n",
        pack.host, pack.description, pack.license
    );
    for file in &pack.files {
        notes.push_str(&format!("{}  {}\n", file.sha256, file.url));
    }
    fs::write(staging.join("INSTALLED.txt"), notes)
        .map_err(|error| format!("Could not write the install notes: {error}"))
}

/// Fetch `url` into `out`, failing (and deleting `out`) unless its SHA-256
/// is `sha256`. `on_bytes` gets the bytes read so far.
pub(crate) fn download(
    url: &str,
    sha256: &str,
    out: &Path,
    cancel: &AtomicBool,
    mut on_bytes: impl FnMut(u64),
) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(30 * 60))
        .user_agent(concat!("PieceOff/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Could not start the download: {error}"))?;
    let mut response = client
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Could not download {url}: {error}"))?;
    let mut file =
        File::create(out).map_err(|error| format!("Could not save the download: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut read_total = 0u64;
    let outcome = (|| loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled.".to_owned());
        }
        let count = response
            .read(&mut buffer)
            .map_err(|error| format!("The download was interrupted: {error}"))?;
        if count == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..count]);
        file.write_all(&buffer[..count])
            .map_err(|error| format!("Could not save the download: {error}"))?;
        read_total += count as u64;
        if read_total > MAX_UNPACKED_BYTES {
            return Err("The download is larger than expected.".to_owned());
        }
        on_bytes(read_total);
    })();
    drop(file);
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let outcome = outcome.and_then(|()| {
        if digest.eq_ignore_ascii_case(sha256) {
            Ok(())
        } else {
            Err(
                "The download did not match its expected checksum, so it was discarded. \
                 Nothing was installed."
                    .to_owned(),
            )
        }
    });
    if outcome.is_err() {
        let _ = fs::remove_file(out);
    }
    outcome
}

/// Copy at most `MAX_UNPACKED_BYTES` from `from` into a new file at `to`.
fn save_program(from: &mut impl Read, to: &Path) -> Result<(), String> {
    let mut file =
        File::create(to).map_err(|error| format!("Could not save a program: {error}"))?;
    let written = io::copy(&mut from.take(MAX_UNPACKED_BYTES + 1), &mut file)
        .map_err(|error| format!("Could not unpack a program: {error}"))?;
    if written > MAX_UNPACKED_BYTES {
        return Err("A downloaded program is larger than expected.".into());
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(to, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("Could not make a program runnable: {error}"))?;
    }
    Ok(())
}

/// Unpack `archive` into `dir`, saving only the programs named in `extract`
/// (never a path taken from the archive itself).
fn extract(archive: &Path, extract: &Extract, dir: &Path) -> Result<(), String> {
    let open =
        || File::open(archive).map_err(|error| format!("Could not read the download: {error}"));
    match extract {
        Extract::Gzip(tool) => {
            let mut decoder = flate2::read::GzDecoder::new(open()?);
            save_program(&mut decoder, &dir.join(tool.file_name()))
        }
        Extract::Zip(members) => {
            let mut zip = zip::ZipArchive::new(open()?)
                .map_err(|error| format!("The download is not a valid zip file: {error}"))?;
            for (member, tool) in members {
                let mut entry = zip
                    .by_name(member)
                    .map_err(|_| format!("The download does not contain {member}."))?;
                save_program(&mut entry, &dir.join(tool.file_name()))?;
            }
            Ok(())
        }
    }
}

/// Both programs run, and the decoders the install exists for are there.
fn verify_tools(dir: &Path) -> Result<(), String> {
    let run = |tool: Tool, args: &[&str]| {
        Command::new(dir.join(tool.file_name()))
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    run(Tool::Ffprobe, &["-version"])
        .ok_or("The downloaded ffprobe does not run on this computer.")?;
    let decoders = run(Tool::Ffmpeg, &["-hide_banner", "-decoders"])
        .ok_or("The downloaded ffmpeg does not run on this computer.")?;
    let names = crate::media_tools::parse_codec_names(&decoders);
    if ["h264", "hevc", "aac"]
        .iter()
        .all(|name| names.contains(*name))
    {
        Ok(())
    } else {
        Err("The downloaded FFmpeg lacks the codecs it was installed for.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU64;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pieceoff-codec-pack-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// Serve `routes` (path -> body) on a local port until the process ends.
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = [0u8; 2048];
                let count = stream.read(&mut request).unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..count]).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let body = routes.iter().find(|(route, _)| *route == path);
                let response = match body {
                    Some((_, body)) => {
                        let mut response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        response.extend_from_slice(body);
                        response
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = stream.write_all(&response);
            }
        });
        format!("http://{address}")
    }

    #[test]
    fn every_pin_is_https_with_a_checksum_and_a_known_size() {
        for (os, arch) in [
            ("windows", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
        ] {
            let pack =
                Pack::for_target(os, arch).unwrap_or_else(|| panic!("no pack for {os} {arch}"));
            assert!(!pack.files.is_empty());
            let mut tools = Vec::new();
            for file in &pack.files {
                assert!(file.url.starts_with("https://github.com/"), "{}", file.url);
                assert_eq!(file.sha256.len(), 64, "{}", file.url);
                assert!(
                    file.sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                );
                assert!(file.size > 1_000_000 && file.size < MAX_UNPACKED_BYTES);
                match &file.extract {
                    Extract::Gzip(tool) => tools.push(*tool),
                    Extract::Zip(members) => tools.extend(members.iter().map(|(_, tool)| *tool)),
                }
            }
            tools.sort_by_key(|tool| tool.name());
            assert_eq!(
                tools,
                [Tool::Ffmpeg, Tool::Ffprobe],
                "{os} {arch} needs both programs"
            );
            let text = pack.consent_text();
            assert!(text.contains(&pack.host) && text.contains("patent"));
        }
        assert!(Pack::for_target("freebsd", "x86_64").is_none());
    }

    #[test]
    fn a_download_is_kept_only_when_its_checksum_matches() {
        let body = b"pretend this is ffmpeg".to_vec();
        let base = serve(vec![("/ok", body.clone())]);
        let dir = scratch("download");
        let out = dir.join("file");
        let cancel = AtomicBool::new(false);
        let mut seen = 0;
        download(
            &format!("{base}/ok"),
            &sha256_hex(&body),
            &out,
            &cancel,
            |bytes| seen = bytes,
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), body);
        assert_eq!(seen, body.len() as u64);

        let wrong = download(
            &format!("{base}/ok"),
            &sha256_hex(b"something else"),
            &out,
            &cancel,
            |_| {},
        );
        assert!(wrong.unwrap_err().contains("checksum"));
        assert!(!out.exists(), "a tampered download is deleted");

        let missing = download(
            &format!("{base}/nope"),
            &sha256_hex(&body),
            &out,
            &cancel,
            |_| {},
        );
        assert!(missing.is_err());
        assert!(!out.exists());

        cancel.store(true, Ordering::Relaxed);
        let stopped = download(
            &format!("{base}/ok"),
            &sha256_hex(&body),
            &out,
            &cancel,
            |_| {},
        );
        assert_eq!(stopped.unwrap_err(), "Cancelled.");
        assert!(!out.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn gzip_and_zip_members_unpack_to_fixed_names() {
        let dir = scratch("extract");
        let gz = dir.join("a.gz");
        fs::write(&gz, gzip(b"ffmpeg bytes")).unwrap();
        extract(&gz, &Extract::Gzip(Tool::Ffmpeg), &dir).unwrap();
        assert_eq!(
            fs::read(dir.join(Tool::Ffmpeg.file_name())).unwrap(),
            b"ffmpeg bytes"
        );

        let zip_path = dir.join("a.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("pkg/bin/ffprobe", options).unwrap();
        writer.write_all(b"probe bytes").unwrap();
        writer.start_file("../escape", options).unwrap();
        writer.write_all(b"never extracted").unwrap();
        writer.finish().unwrap();
        extract(
            &zip_path,
            &Extract::Zip(vec![("pkg/bin/ffprobe".into(), Tool::Ffprobe)]),
            &dir,
        )
        .unwrap();
        assert_eq!(
            fs::read(dir.join(Tool::Ffprobe.file_name())).unwrap(),
            b"probe bytes"
        );
        assert!(!dir.parent().unwrap().join("escape").exists());
        let absent = extract(
            &zip_path,
            &Extract::Zip(vec![("pkg/missing".into(), Tool::Ffmpeg)]),
            &dir,
        );
        assert!(absent.unwrap_err().contains("does not contain"));
        let _ = fs::remove_dir_all(dir);
    }

    /// A shell script that answers like FFmpeg, so the whole install runs.
    #[cfg(unix)]
    fn fake_program(decoders: &str) -> Vec<u8> {
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  *-decoders*) printf ' Decoders:\\n ------\\n {decoders}\\n' ;;\n  *) echo 'ffmpeg version fake' ;;\nesac\n"
        )
        .into_bytes()
    }

    #[cfg(unix)]
    fn fake_pack(base: &str, ffmpeg: &[u8], ffprobe: &[u8]) -> Pack {
        let file = |name: &str, bytes: &[u8], tool| PackFile {
            url: format!("{base}/{name}"),
            sha256: sha256_hex(&gzip(bytes)),
            size: gzip(bytes).len() as u64,
            extract: Extract::Gzip(tool),
        };
        Pack {
            host: "localhost".into(),
            description: "a fake build".into(),
            license: "no license".into(),
            files: vec![
                file("ffmpeg.gz", ffmpeg, Tool::Ffmpeg),
                file("ffprobe.gz", ffprobe, Tool::Ffprobe),
            ],
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_full_install_replaces_the_old_one_and_a_bad_one_changes_nothing() {
        let good = fake_program("V....D h264 H.264\\n A....D aac AAC\\n V....D hevc HEVC");
        let base = serve(vec![
            ("/ffmpeg.gz", gzip(&good)),
            ("/ffprobe.gz", gzip(&fake_program(""))),
        ]);
        let root = scratch("install");
        let dest = root.join("codecs");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old"), "old install").unwrap();

        let pack = fake_pack(&base, &good, &fake_program(""));
        let cancel = AtomicBool::new(false);
        let mut last = (0, 0);
        install(&pack, &dest, &cancel, |done, total| last = (done, total)).unwrap();
        assert_eq!(last, (pack.total_bytes(), pack.total_bytes()));
        assert!(dest.join("ffmpeg").is_file() && dest.join("ffprobe").is_file());
        assert!(!dest.join("old").exists(), "the old install is replaced");
        assert!(
            fs::read_to_string(dest.join("INSTALLED.txt"))
                .unwrap()
                .contains("localhost")
        );
        assert!(!root.join("codecs.installing").exists());

        // A build without the codecs is refused and the working install stays.
        let lacking = fake_program("V....D mpeg4 MPEG-4");
        let base = serve(vec![
            ("/ffmpeg.gz", gzip(&lacking)),
            ("/ffprobe.gz", gzip(&fake_program(""))),
        ]);
        let bad = fake_pack(&base, &lacking, &fake_program(""));
        let error = install(&bad, &dest, &cancel, |_, _| {}).unwrap_err();
        assert!(error.contains("lacks the codecs"), "{error}");
        assert!(
            dest.join("ffmpeg").is_file(),
            "the previous install survives a failed one"
        );
        assert!(
            !root.join("codecs.installing").exists(),
            "staging is cleaned up"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// Downloads the real pin for this machine (tens of MB) and runs it. Not
    /// part of the normal run: `cargo test -p pieceoff-app -- --ignored real_pack`.
    #[test]
    #[ignore = "downloads the real FFmpeg pack"]
    fn real_pack_installs_and_decodes_h264() {
        let pack = Pack::current().expect("this platform has a pack");
        let root = scratch("real");
        let dest = root.join("codecs");
        let cancel = AtomicBool::new(false);
        install(&pack, &dest, &cancel, |_, _| {}).unwrap();
        let ffmpeg = dest.join(Tool::Ffmpeg.file_name());
        // Make an H.264 clip with the installed build, then decode it.
        let clip = root.join("clip.mp4");
        let made = Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=1:s=64x48:r=10",
                "-an",
                "-c:v",
                "libx264",
                "-y",
            ])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(made.success());
        let decoded = Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&clip)
            .args(["-f", "null", "-"])
            .status()
            .unwrap();
        assert!(decoded.success());
        let _ = fs::remove_dir_all(root);
    }
}
