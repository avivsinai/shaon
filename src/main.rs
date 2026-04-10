use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    shaon::app::cli::run().await
}
