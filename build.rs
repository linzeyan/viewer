use std::path::PathBuf;

fn main() {
    slint_build::compile("ui/app.slint").expect("Slint build failed");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

/// The exe's icon and version, which Explorer, the taskbar, Task Manager and the file's
/// Details tab show. Generated so the version always matches Cargo.toml, like the macOS
/// bundle's Info.plist.
fn windows_resources() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let number = |part: &str| std::env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap();
    let numeric = format!("{},{},{},0", number("MAJOR"), number("MINOR"), number("PATCH"));
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // Forward slashes: rc reads backslashes in strings as escapes.
    let icon = manifest_dir
        .join("packaging/windows/viewer.ico")
        .to_string_lossy()
        .replace('\\', "/");
    println!("cargo:rerun-if-changed=packaging/windows/viewer.ico");

    // The first icon group is the one Windows shows for the exe.
    // 0x40004 = VOS_NT_WINDOWS32, 0x1 = VFT_APP; numbers avoid needing winver.h.
    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Viewer"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "viewer"
      VALUE "OriginalFilename", "viewer.exe"
      VALUE "ProductName", "Viewer"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("viewer.rc");
    std::fs::write(&path, rc).expect("write viewer.rc");
    // Required, so a missing rc compiler fails the build instead of shipping an exe
    // without an icon.
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows resources");
}
