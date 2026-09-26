//! The I/O-free core of the WebSocket transport: lane and pool state
//! machines that take events and return actions.

pub mod continuation;
pub mod lane;
