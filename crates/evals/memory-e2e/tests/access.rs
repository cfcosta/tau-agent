//! Which access the evaluation runs on: a sign-in named on the command
//! line, then the environment's key, then what tau saved, and none
//! without any, so nothing runs.

use tau_memory_e2e::access::{Access, resolve};

#[test]
fn access_is_the_first_found_in_order() {
    let config = tempfile::tempdir().unwrap();
    let dir = Some(config.path());
    assert_eq!(resolve(None, None, dir), None);
    assert_eq!(resolve(None, Some("  ".into()), dir), None);
    assert_eq!(resolve(None, None, None), None);
    std::fs::write(config.path().join("openai-key"), "saved\n").unwrap();
    assert_eq!(
        resolve(None, None, dir),
        Some(Access::ApiKey("saved".into()))
    );
    std::fs::write(config.path().join("codex.json"), "{}").unwrap();
    let signed_in = Access::Codex(config.path().join("codex.json"));
    assert_eq!(resolve(None, None, dir), Some(signed_in.clone()));
    assert_eq!(
        resolve(None, Some("env".into()), dir),
        Some(Access::ApiKey("env".into()))
    );
    let named = Access::Codex("named.json".into());
    assert_eq!(
        resolve(Some("named.json".into()), Some("env".into()), dir),
        Some(named)
    );
}

#[test]
fn each_access_defaults_to_tau_uis_model() {
    assert_eq!(Access::Codex("c".into()).default_model(), "gpt-6-sol");
    assert_eq!(Access::ApiKey("k".into()).default_model(), "gpt-5.5");
}
