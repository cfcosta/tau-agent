//! The `tau.store` slot (ADR 0045): what opens the store every run is
//! kept in, as the host starts. It is filled before any plugin's host
//! state is made, since that is made with the store
//! ([`crate::HostHalf::fill_early`]).

use std::{future::Future, path::Path, pin::Pin};

use tau_store::Store;

use crate::slots::Slot;

/// The slot.
pub const STORE: Slot<dyn OpenStore> = Slot::new("tau.store");

/// What opening a store gives.
pub type Opened<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<Store>> + Send + 'a>>;

/// Opens stores.
pub trait OpenStore: Send + Sync {
    /// The store kept at `path`, made there when there is none.
    fn open<'a>(&'a self, path: &'a Path) -> Opened<'a>;
}
