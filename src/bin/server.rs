/* =============================================================================
 * File          : server.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Binary entry point: starts the Tokio runtime and launches the async
 *   server (web::run), exiting non-zero on failure.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   `cargo run --bin server` / the homevault systemd unit.
 * =============================================================================
 */

use std::error::Error;

/// Entry point — starts the Tokio runtime and launches the async server.
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // Propagate the failure instead of swallowing it: a refused start (for
        // example "the database was written by a newer HomeVault") must exit
        // non-zero, otherwise systemd's `Restart=on-failure` cannot tell the
        // difference and an operator sees a clean exit on a server that never
        // came up.
        if let Err(e) = homevault::web::run().await {
            eprintln!("Server error: {}", e);
            return Err(e);
        }
        Ok(())
    })
}
