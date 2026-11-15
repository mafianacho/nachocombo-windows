//! Nacho Pet, the platform-free part: ported from the macOS app's `Sources/NachoComboCore/Pet` (one Rust file per
//! Swift file, same type names, snake_case fields). The Win32 shell supplies the network ([`http::PetHTTPClient`],
//! [`chat_client::WebSocketTransport`]), the token store and the windows; everything that decides is here and tested
//! on any OS with `cargo test --lib`.
pub mod auth;
pub mod card_flow;
pub mod card_rules;
pub mod character_assets;
pub mod chat_client;
pub mod chat_layout;
pub mod chat_state;
pub mod clock;
pub mod color_hex;
pub mod commands;
pub mod composer;
pub mod connector_client;
pub mod connections;
pub mod connectors;
pub mod crypto;
pub mod data_tools;
pub mod design_tokens;
pub mod desktop;
pub mod desktop_runner;
pub mod desktop_tidy;
pub mod dictation;
pub mod face;
pub mod frames;
pub mod gadget_guide;
pub mod gadget_tools;
pub mod http;
pub mod json;
pub mod login_browser;
pub mod markdown;
pub mod mcp_card;
pub mod media;
pub mod motion;
pub mod placement;
pub mod reconnect;
pub mod secrets;
pub mod selection;
pub mod selection_watch;
pub mod server_api;
pub mod server_models;
pub mod session;
pub mod settings;
pub mod settings_tools;
pub mod trust;
#[cfg(test)]
mod i18n_check;
pub mod socket_transport;
pub mod transcript;
pub mod voice;
pub mod wav;
pub mod window_tools;
pub mod ws_framing;
