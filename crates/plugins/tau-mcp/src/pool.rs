//! A set of connections kept across changes to the servers
//! (`docs/reference/mcp.md`, "On the host").
//!
//! [`Pool::update`] takes the servers as they are now and keeps every
//! connection whose server did not change: a new or changed server gets
//! a new connection, and a removed, changed or disabled one is let go.
//! A connection let go closes once nothing holds it, so a run going on
//! keeps the connections it started with ([`Connection`]'s drop).
//!
//! The pool does not connect: whoever uses a connection starts it
//! ([`Connection::start`]), so a server no scope uses never runs.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use futures_util::future::join_all;

use crate::{
    config::{Origin, ServerConfig},
    connection::{Connection, Environment},
};

/// What [`Pool::update`] does, by server name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    /// Enabled before and now, with the same entry: the connection stays.
    pub kept: BTreeSet<String>,
    /// Enabled now, and new, changed or disabled before: a new
    /// connection.
    pub connected: BTreeSet<String>,
    /// Enabled before, and removed, changed or disabled now: its
    /// connection is let go.
    pub closed: BTreeSet<String>,
}

/// Whether `old` and `new`, the same server's, are the same entry from
/// the same place. A [`Transport::Stream`](crate::config::Transport)
/// server is the same only with the same dial.
fn same(old: &(Origin, ServerConfig), new: &(Origin, ServerConfig)) -> bool {
    old == new
}

/// What changes from servers `old` to servers `new`, matched by name.
pub fn diff(
    old: &[(Origin, ServerConfig)],
    new: &[(Origin, ServerConfig)],
) -> Diff {
    let find = |servers: &[(Origin, ServerConfig)], name: &str| {
        servers
            .iter()
            .find(|(_, server)| server.name == name)
            .cloned()
    };
    let mut result = Diff::default();
    for entry in new.iter().filter(|(_, server)| server.enabled) {
        let name = entry.1.name.clone();
        match find(old, &name) {
            Some(before) if before.1.enabled && same(&before, entry) => {
                result.kept.insert(name);
            }
            _ => {
                result.connected.insert(name);
            }
        }
    }
    for before in old.iter().filter(|(_, server)| server.enabled) {
        let name = &before.1.name;
        let stays = find(new, name)
            .is_some_and(|now| now.1.enabled && same(before, &now));
        if !stays {
            result.closed.insert(name.clone());
        }
    }
    result
}

/// Connections, one per server, that outlive changes to the others.
pub struct Pool {
    environment: Environment,
    connections: Mutex<Vec<Arc<Connection>>>,
}

impl Pool {
    /// An empty pool whose servers resolve in `environment`: its
    /// repository is their root and where relative `cwd`s start.
    pub fn new(environment: Environment) -> Self {
        Self {
            environment,
            connections: Mutex::default(),
        }
    }

    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// The connections, one per server, in the order last given.
    pub fn connections(&self) -> Vec<Arc<Connection>> {
        self.connections.lock().expect("pool lock").clone()
    }

    /// The connection to the server `name`.
    pub fn get(&self, name: &str) -> Option<Arc<Connection>> {
        self.connections
            .lock()
            .expect("pool lock")
            .iter()
            .find(|connection| connection.name() == name)
            .cloned()
    }

    /// Makes the pool's servers `servers`, keeping the connection of
    /// every server whose entry did not change, and returns what
    /// changed. New connections are not started. A connection let go
    /// closes when the last holder drops it; drop it on a runtime.
    pub fn update(&self, servers: Vec<(Origin, ServerConfig)>) -> Diff {
        let mut connections = self.connections.lock().expect("pool lock");
        let old: Vec<(Origin, ServerConfig)> = connections
            .iter()
            .map(|connection| {
                (connection.origin(), connection.config().clone())
            })
            .collect();
        let changes = diff(&old, &servers);
        let next: Vec<Arc<Connection>> = servers
            .into_iter()
            .map(|(origin, server)| {
                let entry = (origin, server);
                connections
                    .iter()
                    .find(|connection| {
                        connection.name() == entry.1.name
                            && same(
                                &(
                                    connection.origin(),
                                    connection.config().clone(),
                                ),
                                &entry,
                            )
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        Connection::new(
                            entry.1,
                            entry.0,
                            self.environment.clone(),
                        )
                    })
            })
            .collect();
        let old = std::mem::replace(&mut *connections, next);
        drop(connections);
        drop(old);
        changes
    }

    /// Closes every connection, runs going on included.
    pub async fn shutdown(&self) {
        let connections = self.connections();
        join_all(connections.iter().map(|c| c.shutdown())).await;
    }
}
