//! Nacho Pet, the platform-free part: ported from the macOS app's `Sources/NachoComboCore/Pet` (one Rust file per
//! Swift file, same type names, snake_case fields). The Win32 shell supplies the network ([`http::PetHTTPClient`],
//! [`chat_client::WebSocketTransport`]), the token store and the windows; everything that decides is here and tested
//! on any OS with `cargo test --lib`.
pub mod card_rules;
pub mod chat_client;
pub mod chat_state;
pub mod clock;
pub mod crypto;
pub mod frames;
pub mod http;
pub mod json;
pub mod markdown;
pub mod media;
pub mod reconnect;
pub mod server_api;
pub mod server_models;
pub mod settings;
pub mod socket_transport;
pub mod transcript;
pub mod ws_framing;
