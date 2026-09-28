//! Signs in to OpenAI Codex with a ChatGPT account and saves the
//! credentials where tau looks for them.
//!
//! ```sh
//! cargo run -p tau-ai --example codex_login            # browser
//! cargo run -p tau-ai --example codex_login -- device  # headless
//! cargo run -p tau-ai --example codex_login -- import  # from ~/.codex
//! ```

use std::path::PathBuf;

use tau_ai::codex::{
    BrowserLogin,
    CodexCredentials,
    DeviceLogin,
    ORIGINATOR,
    oauth,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = CodexCredentials::default_path().ok_or("no home directory")?;
    let credentials = match std::env::args().nth(1).as_deref() {
        Some("device") => {
            let login = DeviceLogin::start().await?;
            println!(
                "Open {} and enter the code {}",
                oauth::DEVICE_VERIFICATION_URL,
                login.user_code
            );
            login.wait().await?
        }
        Some("import") => {
            let home = std::env::var_os("HOME").ok_or("no home directory")?;
            CodexCredentials::from_codex_cli(
                &PathBuf::from(home).join(".codex/auth.json"),
            )?
        }
        _ => {
            let login = BrowserLogin::start(ORIGINATOR);
            println!("Open this page to sign in:\n\n{}\n", login.url);
            login.wait().await?
        }
    };
    credentials.save(&path)?;
    println!(
        "Signed in as account {}. Saved to {}",
        credentials.account_id,
        path.display()
    );
    Ok(())
}
