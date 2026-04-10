use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    hilan::app::cli::run().await
}
