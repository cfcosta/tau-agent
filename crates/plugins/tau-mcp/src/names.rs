//! Tool names and namespaces (`docs/reference/mcp.md`, "Names").
//!
//! A tool is `mcp__<server>__<tool>`, every character outside
//! `[A-Za-z0-9_]` replaced by `_`. A name over 64 characters, or one
//! that two tools share, is cut and gets `_` and the first 8 hex digits
//! of `sha256(server \0 tool)`. Every tool in a collision gets one, so
//! the names do not depend on the order tools are listed.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::config::hex;

/// The longest name a tool gets.
pub const MAX_LEN: usize = 64;

/// `_` and 8 hex digits.
const SUFFIX_LEN: usize = 9;

/// Replaces every character outside `[A-Za-z0-9_]` with `_`.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A server's namespace: `mcp__<server with - as _>`. It is also what
/// [`ServerConfig::namespace`](crate::config::ServerConfig::namespace)
/// gives.
pub fn namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

/// The name of `server`'s tool `tool`, before collisions are known.
pub fn base_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize(server), sanitize(tool))
}

/// `base` cut to fit, with `_` and the first 8 hex digits of
/// `sha256(server \0 tool)`.
pub fn hashed_name(server: &str, tool: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(tool.as_bytes());
    let digest = hex(&hasher.finalize()[..4]);
    let mut base = base_name(server, tool);
    // The base is ASCII, so any byte is a char boundary.
    base.truncate(MAX_LEN - SUFFIX_LEN);
    format!("{base}_{digest}")
}

/// The names of every `(server, tool)` pair, in the same order. Equal
/// pairs get the same name; distinct pairs get distinct names (barring
/// a collision of SHA-256 prefixes), and a pair's name depends on the
/// set of pairs, not on their order.
pub fn tool_names(pairs: &[(&str, &str)]) -> Vec<String> {
    let mut hashed = vec![false; pairs.len()];
    for (index, (server, tool)) in pairs.iter().enumerate() {
        hashed[index] = base_name(server, tool).len() > MAX_LEN;
    }
    // Hashing a name can make it collide with another tool's plain name;
    // repeat until no two distinct pairs share a name. Each round hashes
    // at least one more pair, so this ends.
    loop {
        let names: Vec<String> = pairs
            .iter()
            .zip(&hashed)
            .map(|((server, tool), hashed)| {
                if *hashed {
                    hashed_name(server, tool)
                } else {
                    base_name(server, tool)
                }
            })
            .collect();
        let mut owners: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, name) in names.iter().enumerate() {
            owners.entry(name).or_default().push(index);
        }
        let mut changed = false;
        for indices in owners.values() {
            let first = pairs[indices[0]];
            if indices.iter().all(|&index| pairs[index] == first) {
                continue;
            }
            for &index in indices {
                if !hashed[index] {
                    hashed[index] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return names;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names() {
        assert_eq!(
            tool_names(&[("git-hub", "list.issues")]),
            ["mcp__git_hub__list_issues"]
        );
        assert_eq!(namespace("git-hub"), "mcp__git_hub");
    }

    #[test]
    fn collisions_hash_every_member() {
        let names = tool_names(&[("a", "b__c"), ("a__b", "c"), ("x", "y")]);
        assert_eq!(names[2], "mcp__x__y");
        assert_ne!(names[0], names[1]);
        assert!(names[0].starts_with("mcp__a__b__c_"));
        assert_eq!(names[0].len(), "mcp__a__b__c_".len() + 8);
    }

    #[test]
    fn long_names_are_cut() {
        let tool = "t".repeat(100);
        let name = &tool_names(&[("s", &tool)])[0];
        assert_eq!(name.len(), MAX_LEN);
    }
}
