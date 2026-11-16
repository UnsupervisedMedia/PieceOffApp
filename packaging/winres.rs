// Shared by the app and launcher build scripts (include!). Windows only: embed the icon and version metadata so Explorer, the taskbar
// and SmartScreen show a named, branded program instead of a blank one.

fn embed_windows_resources(original: &str) {
    println!("cargo:rerun-if-changed=../../packaging/icon/pieceoff.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packaging/winres.rs");
    // `cfg!(windows)` in a build script describes the host, not the target, so
    // read the target from Cargo. This also covers the Linux -> Windows cross build.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    embed(original);
    #[cfg(not(windows))]
    embed_cross(original);
}

#[cfg(windows)]
fn embed(original: &str) {
    let mut res = winresource::WindowsResource::new();
    configure(&mut res, original);
    res.compile().expect("embedding Windows resources");
}

#[cfg(not(windows))]
fn embed_cross(original: &str) {
    // Cross-compiling from Linux: winresource is only a Windows build-dependency,
    // so drive the MinGW resource compiler directly.
    use std::{env, fs, path::PathBuf, process::Command};
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("pieceoff.rc");
    let ico = fs::canonicalize("../../packaging/icon/pieceoff.ico").unwrap();
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (a, b, c) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let script = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "ProductName", "PieceOff"
      VALUE "FileDescription", "PieceOff - shared jigsaw puzzles"
      VALUE "CompanyName", "PieceOff contributors"
      VALUE "LegalCopyright", "Copyright 2026 PieceOff contributors. Apache-2.0"
      VALUE "OriginalFilename", "{original}"
      VALUE "InternalName", "{internal}"
      VALUE "ProductVersion", "{version}"
      VALUE "FileVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        internal = original.trim_end_matches(".exe"),
        ico = ico.display().to_string().replace('\\', "/"),
    );
    fs::write(&rc, script).unwrap();
    let obj = out.join("pieceoff-res.o");
    let status = Command::new("x86_64-w64-mingw32-windres")
        .arg("--input")
        .arg(&rc)
        .arg("--output")
        .arg(&obj)
        .status()
        .expect("x86_64-w64-mingw32-windres is required to cross-build for Windows");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={}", obj.display());
}

#[cfg(windows)]
fn configure(res: &mut winresource::WindowsResource, original: &str) {
    res.set_icon("../../packaging/icon/pieceoff.ico");
    res.set("ProductName", "PieceOff");
    res.set("FileDescription", "PieceOff - shared jigsaw puzzles");
    res.set("CompanyName", "PieceOff contributors");
    res.set(
        "LegalCopyright",
        "Copyright 2026 PieceOff contributors. Apache-2.0",
    );
    res.set("OriginalFilename", original);
}
