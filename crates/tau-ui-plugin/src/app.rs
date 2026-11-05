//! The computer's window as phones mirror it (ADR 0046): what it
//! applies goes out, a phone that needs everything is sent what it
//! shows, and what a phone sends the window acts on as on its own. The
//! kernel keeps what crosses and what a phone may ask; a plugin that
//! carries it to phones (tau-phones) reaches the window only through
//! this, the service [`crate::services::names::APP`].

use std::sync::Arc;

use serde_json::Value;

/// One body for phones: what the window applied, or an answer to a
/// phone's request.
#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub body: Value,
    /// An answer, which a phone that missed it asks again: sent to the
    /// phones there now, unnumbered and not kept.
    pub transient: bool,
}

/// Where the window sends what phones are sent, on its own thread, in
/// the order it applies it.
pub type Sink = Arc<dyn Fn(Feed) + Send + Sync>;

/// Hands a phone that needs everything what the window shows, as of the
/// last body sent to the sink.
pub type Reply = Box<dyn FnOnce(Vec<Value>) + Send>;

/// What came of what a phone sent ([`App::up`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Up {
    /// The window takes it, or it was not something a phone may ask.
    Taken,
    /// The phone gave its name, which is the carrier's to keep.
    Renamed(String),
}

/// The computer's window, as a plugin that carries it to phones sees it.
pub trait App: Send + Sync {
    /// Sends what the window applies, and its answers to phones, to
    /// `sink` from now on; `None` stops, and forgets what phones asked.
    fn mirror(&self, sink: Option<Sink>);

    /// Hands `reply` what the window shows, on the window's thread, so
    /// that nothing sent to the sink falls between them.
    fn snapshot(&self, reply: Reply);

    /// What the phone `device` sent.
    fn up(&self, device: &str, body: Value) -> Up;

    /// The phone `device` left: nothing is built for it any more.
    fn left(&self, device: &str);
}
