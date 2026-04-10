use anyhow::Result;

use crate::config::Config;
use crate::provider::HilanProvider;

pub mod cli;
pub mod mcp;

pub fn load_config() -> Result<Config> {
    Config::load()
}

pub fn build_provider(config: Config) -> Result<HilanProvider> {
    HilanProvider::new(config)
}

pub async fn build_authenticated_provider(config: Config) -> Result<HilanProvider> {
    let mut provider = build_provider(config)?;
    provider.client_mut().ensure_authenticated().await?;
    Ok(provider)
}
