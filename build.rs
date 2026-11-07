// Embeds the icon, the per-monitor DPI manifest and a version resource that File Explorer shows under Properties ▸ Details.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let parts: Vec<&str> = version.split(['.', '-']).take(3).collect();
    let num = format!("{},{},{},0", parts.first().unwrap_or(&"0"), parts.get(1).unwrap_or(&"0"), parts.get(2).unwrap_or(&"0"));
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap().replace('\\', "/");
    let rc = format!(r#"1 ICON "{dir}/assets/app.ico"
1 24 "{dir}/assets/app.manifest"
1 VERSIONINFO
FILEVERSION {num}
PRODUCTVERSION {num}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Mafianacho"
      VALUE "FileDescription", "NachoCombo for Windows"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "NachoCombo"
      VALUE "LegalCopyright", "MIT License. The Nacho Combo name and logo are not covered by it."
      VALUE "OriginalFilename", "NachoCombo.exe"
      VALUE "ProductName", "NachoCombo for Windows"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#);
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
