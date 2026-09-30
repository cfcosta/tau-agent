//! Which access the evaluation runs on: an account named on the command
//! line, then the environment's key, then what tau saved, and none
//! without any, so nothing runs.

use tau_ai::chatgpt::AccountId;
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
    // A store with no active account adds nothing.
    std::fs::create_dir_all(config.path().join("chatgpt/accounts")).unwrap();
    assert_eq!(
        resolve(None, None, dir),
        Some(Access::ApiKey("saved".into()))
    );
    assert_eq!(
        resolve(None, Some("env".into()), dir),
        Some(Access::ApiKey("env".into()))
    );
    let account = AccountId::parse("oaiapp_x-0123").unwrap();
    assert_eq!(
        resolve(Some(account.clone()), Some("env".into()), dir),
        Some(Access::ChatGpt {
            store: config.path().join("chatgpt"),
            account,
        })
    );
}
