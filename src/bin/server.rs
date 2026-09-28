use std::error::Error;

/// Entry point — starts the Tokio runtime and launches the async server.
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if let Err(e) = homevault::web::run().await {
            eprintln!("Server error: {}", e);
        }
    });

    Ok(())
}
