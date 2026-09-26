//! Shared connection for the live tests.
//!
//! Live tests run against a hosted connection by default so they never touch
//! the local GPU servers. The config comes from `LETHETIC_LIVE_CONFIG` (default:
//! the per-user `~/.config/lethetic/config.yml`, with its `config.local.yml`
//! overlay for the key) and the connection from `LETHETIC_LIVE_SERVER`
//! (default: `openrouter`).

use lethetic::config::Config;

pub fn live_config() -> Result<Config, String> {
    let path = std::env::var("LETHETIC_LIVE_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| lethetic::platform::lethetic_config_dir().join("config.yml"));
    let server_id = std::env::var("LETHETIC_LIVE_SERVER").unwrap_or_else(|_| "openrouter".into());
    let mut config = Config::load(&path)?;
    config.merge_matching_server_settings();
    let model = config
        .model_servers
        .iter()
        .find(|server| server.connection_id() == server_id)
        .map(|server| server.model.clone())
        .ok_or_else(|| format!("live server '{server_id}' is not in {}", path.display()))?;
    config.activate_model(&server_id, &model)?;
    config.validate()?;
    Ok(config)
}
