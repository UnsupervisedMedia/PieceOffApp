//! `PieceOff.exe`: what a player double-clicks.
//!
//! Starts the local room server with no console window on the first free port
//! (so a busy port never blocks the app), starts the app pointed at it, and
//! stops the server when the app closes. If the server cannot start the app
//! still opens: online rooms do not need it.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    env,
    fs::{self, File},
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

const FIRST_PORT: u16 = 28878;
const PORT_ATTEMPTS: u16 = 100;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn free_port() -> Option<u16> {
    (FIRST_PORT..FIRST_PORT + PORT_ATTEMPTS).find(|p| TcpListener::bind(("0.0.0.0", *p)).is_ok())
}

/// A stable per-user folder, so rooms and media survive upgrades that replace
/// the install folder.
fn data_dir(install: &Path) -> PathBuf {
    if let Some(dir) = env::var_os("PIECEOFF_DATA_DIR").filter(|d| !d.is_empty()) {
        return dir.into();
    }
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.map(|b| b.join("PieceOff").join("data"))
        .unwrap_or_else(|| install.join("pieceoff-data"))
}

fn log(dir: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("launcher.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

fn hide(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn start_server(install: &Path, data: &Path) -> Result<(Child, u16), String> {
    let port = free_port().ok_or("no free port between 28878 and 28977")?;
    let server = install.join(exe("pieceoff-server"));
    let out = File::create(data.join("server.log")).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new(&server);
    command
        .env("PIECEOFF_BIND", format!("0.0.0.0:{port}"))
        .env("PIECEOFF_DATA_DIR", data)
        .current_dir(install)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    let mut child = hide(&mut command)
        .spawn()
        .map_err(|e| format!("{}: {e}", server.display()))?;
    // Give it a moment to fail fast (bad config, port raced away).
    sleep(Duration::from_millis(700));
    match child.try_wait() {
        Ok(Some(status)) => Err(format!("room server exited at once ({status})")),
        _ => Ok((child, port)),
    }
}

fn main() {
    let install = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let data = data_dir(&install);
    let _ = fs::create_dir_all(&data);

    let server = match start_server(&install, &data) {
        Ok(started) => Some(started),
        Err(reason) => {
            log(
                &data,
                &format!("local room server not started: {reason} (online rooms still work)"),
            );
            None
        }
    };

    let app = install.join(exe("pieceoff-app"));
    let mut command = Command::new(&app);
    command
        .current_dir(&install)
        .env("PIECEOFF_DATA_DIR", &data);
    if let Some((_, port)) = &server {
        command.env("PIECEOFF_SERVER_URL", format!("ws://127.0.0.1:{port}/ws"));
    }
    let result = command.spawn().and_then(|mut child| child.wait());
    if let Err(error) = &result {
        log(&data, &format!("could not run {}: {error}", app.display()));
    }
    if let Some((mut child, _)) = server {
        let _ = child.kill();
        let _ = child.wait();
    }
    if result.is_err() {
        std::process::exit(1);
    }
}
