//! Nacho Pet, the platform-free part: ported from the macOS app's `Sources/NachoComboCore/Pet` (one Rust file per
//! Swift file, same type names, snake_case fields). The Win32 shell supplies the network ([`http::PetHTTPClient`],
//! [`chat_client::WebSocketTransport`]), the token store and the windows; everything that decides is here and tested
//! on any OS with `cargo test --lib`.
pub mod auth;
pub mod chat_client;
pub mod chat_state;
pub mod clock;
pub mod commands;
pub mod connector_client;
pub mod connectors;
pub mod crypto;
pub mod desktop;
pub mod frames;
pub mod http;
pub mod json;
pub mod media;
pub mod reconnect;
pub mod secrets;
pub mod server_models;
pub mod settings;
pub mod transcript;
