#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[cfg(not(target_os = "linux"))]
compile_error!("EndlessVibe 0.3 targets Linux (openat2 / process groups / bubblewrap). Use Linux or a Linux VM.");
pub mod config;
pub mod config_edit;
pub mod error;
pub mod mcp;
pub mod runtime;
pub mod security;
pub mod server;
pub mod store;
pub mod storage;
pub mod tools;
pub mod util;
pub mod web;
pub mod workspace;
