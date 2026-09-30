//! Which access the evaluation runs on: an account named on the command
//! line, then tau's active sign-in, and none without either, so nothing
//! runs. A saved API key or `OPENAI_API_KEY` is never used.

use tau_ai::chatgpt::AccountId;
use tau_memory_e2e::access::{Access, resolve};

#[test]
fn access_is_a_saved_chatgpt_sign_in() {
    let config = tempfile::tempdir().unwrap();
    let dir = Some(config.path());
    assert_eq!(resolve(None, dir), None);
    assert_eq!(resolve(None, None), None);
    // A key tau used to read is ignored.
    std::fs::write(config.path().join("openai-key"), "saved\n").unwrap();
    assert_eq!(resolve(None, dir), None);
    // A store with no active account adds nothing.
    std::fs::create_dir_all(config.path().join("chatgpt/accounts")).unwrap();
    assert_eq!(resolve(None, dir), None);
    let account = AccountId::parse("oaiapp_x-0123").unwrap();
    assert_eq!(resolve(Some(account.clone()), None), None);
    assert_eq!(
        resolve(Some(account.clone()), dir),
        Some(Access {
            store: config.path().join("chatgpt"),
            account,
        })
    );
}
