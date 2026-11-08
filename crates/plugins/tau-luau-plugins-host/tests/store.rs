//! A Luau plugin's store (ADR 0048's first seam): what a session reads
//! and lists is what a plain map holds after the same writes; a session
//! goes in whole or not at all, a crash anywhere in its commit included;
//! and hooks that read and write it take turns, so none loses another's
//! write.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, sync::Arc};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_luau_plugins_host::{
    runtime::{Context, Files, Hook, NoReach, StoreRef, load},
    store::{Fuse, Kv, Stores},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

/// One write of a session.
#[derive(Debug, Clone)]
enum Write {
    Put(u8, i64),
    Delete(u8),
}

/// Sessions as drawn: each its writes (put or delete, a key, a value)
/// and whether it commits.
type Sessions = Vec<(Vec<(bool, u8, i64)>, bool)>;

fn key(k: u8) -> String {
    format!("k{}/{}", k % 2, k)
}

/// Sessions of writes, each committed or dropped, read back after each:
/// the store holds what a map given the committed ones holds, and a
/// session reads its own writes before it commits.
#[hegel::test(test_cases = 100)]
fn a_store_holds_what_its_committed_sessions_wrote(tc: TestCase) {
    let sessions: Sessions = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::vecs(hegel::tuples!(
                gs::booleans(),
                gs::integers::<u8>().max_value(5),
                gs::integers::<i64>().min_value(-3).max_value(3),
            ))
            .max_size(5),
            gs::booleans(),
        ))
        .max_size(6),
    );
    block_on_io(async {
        let kv = Arc::new(Kv::memory(Fuse::never()).await.unwrap());
        let mut model: BTreeMap<String, Value> = BTreeMap::new();
        for (writes, commits) in sessions {
            let mut session = kv.session().await;
            let mut seen = model.clone();
            for (put, k, n) in writes {
                let write = if put {
                    Write::Put(k, n)
                } else {
                    Write::Delete(k)
                };
                match write {
                    Write::Put(k, n) => {
                        session.put(&key(k), json!(n));
                        seen.insert(key(k), json!(n));
                    }
                    Write::Delete(k) => {
                        session.delete(&key(k));
                        seen.remove(&key(k));
                    }
                }
                for k in 0..6 {
                    assert_eq!(
                        session.get(&key(k)).await.unwrap(),
                        seen.get(&key(k)).cloned()
                    );
                }
                let listed: Vec<(String, Value)> =
                    session.list("k1/").await.unwrap();
                let want: Vec<(String, Value)> = seen
                    .iter()
                    .filter(|(k, _)| k.starts_with("k1/"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                assert_eq!(listed, want);
            }
            if commits {
                session.commit().await.unwrap();
                model = seen;
            }
            assert_eq!(kv.everything().await.unwrap(), model);
        }
    });
}

/// tau closes in the middle of a session's commit, after a drawn number
/// of writes: once the store opens again from its file, it holds the
/// sessions committed before, whole, and nothing of the one cut off.
#[hegel::test(test_cases = 60)]
fn a_crash_in_a_commit_leaves_the_sessions_before_it_whole(tc: TestCase) {
    let sessions: Vec<Vec<(u8, i64)>> = tc.draw(
        gs::vecs(
            gs::vecs(hegel::tuples!(
                gs::integers::<u8>().max_value(4),
                gs::integers::<i64>().max_value(9),
            ))
            .min_size(1)
            .max_size(4),
        )
        .min_size(1)
        .max_size(4),
    );
    let writes_through: i64 = tc.draw(gs::integers::<i64>().max_value(20));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    block_on_io(async {
        let fuse = Fuse::after(writes_through);
        let kv = Arc::new(Kv::open(&path, fuse.clone()).await.unwrap());
        let mut model: BTreeMap<String, Value> = BTreeMap::new();
        for writes in &sessions {
            let mut session = kv.session().await;
            let mut next = model.clone();
            for (k, n) in writes {
                session.put(&key(*k), json!(n));
                next.insert(key(*k), json!(n));
            }
            match session.commit().await {
                Ok(()) => model = next,
                Err(_) => {
                    assert!(fuse.blown());
                    break;
                }
            }
        }
        drop(kv);
        let reopened = Kv::open(&path, Fuse::never()).await.unwrap();
        assert_eq!(reopened.everything().await.unwrap(), model);
    });
}

const COUNTER: &str = r#"
local tau = require("tau")
return tau.plugin { uses = { store = true },
  actions = {
    bump = function(args, ctx)
      local n = ctx.store.get("n") or 0
      ctx.store.put("n", n + 1)
      if args.fail then error("it fails after writing") end
    end,
  },
}
"#;

/// Hooks that read and then write the store take turns: as many bumps at
/// once as there are count them all. A hook that fails writes nothing.
#[test]
fn hooks_take_turns_with_the_store_and_a_failing_one_writes_nothing() {
    block_on_io(async {
        let files = Files {
            source: COUNTER.into(),
            ..Files::named("counter")
        };
        let loaded = load("counter", files).await.unwrap();
        assert!(loaded.declaration.uses.store);
        let kv = Arc::new(Kv::memory(Fuse::never()).await.unwrap());
        let context = Context {
            store: Some(StoreRef(kv.clone())),
            ..Context::default()
        };
        let bumps = (0..20).map(|_| {
            let (loaded, context) = (loaded.clone(), context.clone());
            tokio::spawn(async move {
                loaded
                    .call(
                        &Hook::Action("bump".into()),
                        json!({}),
                        &context,
                        Arc::new(NoReach),
                        CancellationToken::new(),
                    )
                    .await
            })
        });
        for bump in futures_util::future::join_all(bumps).await {
            assert_eq!(bump.unwrap().error, None);
        }
        assert_eq!(kv.everything().await.unwrap()["n"], json!(20));
        let failed = loaded
            .call(
                &Hook::Action("bump".into()),
                json!({ "fail": true }),
                &context,
                Arc::new(NoReach),
                CancellationToken::new(),
            )
            .await;
        assert!(failed.error.is_some());
        assert_eq!(kv.everything().await.unwrap()["n"], json!(20));
    });
}

/// The repositories a plugin keeps a store in are read back from their
/// files' names, as tau starts again, odd names and all.
#[test]
fn a_plugins_repositories_are_read_back_from_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["home", "owner/repo", "a b%c"];
    {
        let stores = Stores::at(dir.path().to_owned());
        for name in names {
            block_on_io(stores.of("todo", name)).unwrap();
        }
    }
    let again = Stores::at(dir.path().to_owned());
    let mut want: Vec<&str> = names.to_vec();
    want.sort();
    assert_eq!(block_on_io(again.repos_of("todo")), want);
    assert!(block_on_io(again.repos_of("other")).is_empty());
}
