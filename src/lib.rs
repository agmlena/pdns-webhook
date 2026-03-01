pub mod config;
pub mod dns;
pub mod handlers;
pub mod pdns;

// AppState lives here so every module can reach it via `crate::AppState`
// without a separate module import.
use crate::{config::Config, pdns::PdnsClient};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub pdns: PdnsClient,
}
