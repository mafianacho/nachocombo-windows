//! Platform-free logic of NachoCombo for Windows. The Win32 shell (`src/main.rs` and the files next to it) uses it;
//! nothing here calls Windows, so `cargo test --lib` runs on Linux and macOS too.
pub mod core;
// The translation table is compiled into the library too, so the languages are tested on any OS.
#[path = "i18n.rs"]
pub mod i18n;
