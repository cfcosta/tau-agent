//! What a loop ([`crate::runner::driver::Loop`]) is held to, for any
//! filler of `tau.loop` to check itself against (ADR 0045): the runner's
//! model, and the checks the tests that drive whole runs share.

pub mod loop_model;
pub mod support;
