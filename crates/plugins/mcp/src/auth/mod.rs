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
