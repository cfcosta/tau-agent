//! A device sign-in against a fake GitHub, through the host: the token
//! GitHub answers is kept as the `github` secret and is in nothing the
//! plugin keeps; the account is who it belongs to; and the forge's
//! requests carry it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use serde_json::json;
use tau_luau_plugins_host::runtime::Hook;
use tau_testing::block_on_io;

#[test]
fn a_device_sign_in_keeps_the_token_past_the_plugin() {
    let (base, seen) = super::fake::fake(|line| match line {
        "POST /login/device/code" => (
            200,
            json!({
                "device_code": "dev-1", "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900, "interval": 5,
            }),
        ),
        "POST /login/oauth/access_token" => (
            200,
            json!({ "access_token": "gho_secret", "expires_in": 28800 }),
        ),
        "GET /user" => (200, json!({ "login": "octo" })),
        "GET /user/installations?per_page=100&page=1" => {
            (200, json!({ "installations": [] }))
        }
        _ => (404, json!({})),
    });
    let luau = super::Luau::at(&base);
    luau.call(Hook::Action("sign_in".into()), json!({}))
        .unwrap();
    luau.call(Hook::Action("check".into()), json!({})).unwrap();
    luau.call(Hook::Job("sign_in".into()), json!(null)).unwrap();
    assert_eq!(luau.kept().as_deref(), Some("gho_secret"));
    // In nothing the plugin keeps.
    let kept = block_on_io(luau.store.0.everything()).unwrap();
    assert!(!format!("{kept:?}").contains("gho_secret"), "{kept:?}");
    let app = luau.call(Hook::App, json!(null)).unwrap();
    assert_eq!(
        app["sign_in"],
        json!({ "state": "signed_in", "user": "octo" })
    );
    assert_eq!(
        luau.call(Hook::Forge("account".into()), json!({})).unwrap(),
        json!("octo")
    );
    luau.call(Hook::Forge("repos".into()), json!({})).unwrap();
    let requests = seen.lock().unwrap().clone();
    let user = requests
        .iter()
        .find(|request| request.starts_with("GET /user "))
        .unwrap();
    assert!(
        user.to_lowercase()
            .contains("authorization: bearer gho_secret"),
        "{user}"
    );
    // Signing out forgets it.
    luau.call(Hook::Action("sign_out".into()), json!({}))
        .unwrap();
    assert_eq!(luau.kept(), None);
}
