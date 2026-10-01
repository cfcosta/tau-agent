//! Signing in to MCP servers with OAuth (`docs/reference/mcp.md`,
//! "Signing in").
//!
//! An HTTP server without an `Authorization` header of its own may ask
//! for a sign-in by answering 401. Its connection then waits in the
//! `needs-auth` state; nothing opens a browser until the user asks, on
//! the plugin's page. [`begin`] finds the authorization server, registers
//! a client when none is configured, and makes the URL to open;
//! [`SignIn::finish`] waits for the browser on a loopback port, checks
//! what comes back, exchanges the code and saves the grant in
//! `~/.config/tau/mcp-auth.json` ([`TokenStore`]). Connections then send
//! the access token, refresh it when it expires or the server turns it
//! down, and connect again when a sign-in or sign-out (here or in
//! another process) changes the grant.

//!
//! ## Logs
//!
//! rmcp's sign-in code logs the authorization code and the token
//! response's extra fields (where an ID token may be) at debug level, in
//! [`SECRET_TARGETS`]. tau runs the code exchange with no subscriber at
//! all, so those lines never reach one; and a host that sets up a
//! subscriber adds [`secrets_filter`] to it, which keeps everything in
//! those targets below info out of every layer, whatever else its
//! filters allow.

mod callback;
mod flow;
pub(crate) mod http;
mod store;

pub use callback::{
    Callback,
    CallbackError,
    Loopback,
    pkce_challenge,
    read_callback,
    valid_verifier,
};
pub(crate) use flow::authorization;
pub use flow::{CLIENT_NAME, SIGN_IN_TIMEOUT, SignIn, SignInRequest, begin};
pub use store::{AUTH_FILE, Grant, GrantKey, TokenStore, account};

/// The tracing targets of rmcp's sign-in code, whose debug and trace
/// lines may carry an authorization code or tokens.
pub const SECRET_TARGETS: [&str; 2] =
    ["rmcp::transport::auth", "rmcp::transport::common::auth"];

/// [`SECRET_TARGETS`] clamped to info, as `EnvFilter` directives. Prefer
/// [`secrets_filter`]: a user's `RUST_LOG` cannot widen it.
pub const LOG_DIRECTIVES: &str =
    "rmcp::transport::auth=info,rmcp::transport::common::auth=info";

/// The filter every host's subscriber carries, as a global filter: it
/// lets everything through but [`SECRET_TARGETS`] below info.
///
/// ```
/// use tracing_subscriber::layer::SubscriberExt as _;
/// let subscriber = tracing_subscriber::registry()
///     .with(tau_mcp::auth::secrets_filter());
///     // .with(your fmt layer, with its own filter)
/// # let _ = subscriber;
/// ```
pub fn secrets_filter() -> tracing_subscriber::filter::Targets {
    tracing_subscriber::filter::Targets::new()
        .with_default(tracing_subscriber::filter::LevelFilter::TRACE)
        .with_targets(SECRET_TARGETS.map(|target| {
            (target, tracing_subscriber::filter::LevelFilter::INFO)
        }))
}
