//! Sign in with ChatGPT end to end: the real client against
//! `FakeChatGpt` in a turmoil simulation, with credentials in a temporary
//! directory (`tau_ai::chatgpt`).

use std::{future::Future, path::PathBuf, time::Duration};

use serde_json::json;
use tau_ai::chatgpt::{
    AccountId,
    AccountStatus,
    ChatGpt,
    ChatGptError,
    IdTokenError,
    PlanUsage,
    Recovery,
    RedirectUri,
    Revocation,
    SignedIn,
    Store,
};
use tau_testing::fake_chatgpt::{
    Consent,
    FakeChatGpt,
    IdTokenFault,
    SimDialer,
};

type Client = ChatGpt<SimDialer>;

/// Runs `test` as a turmoil client beside `fake`, with a fresh store.
fn simulate<F, Fut>(fake: FakeChatGpt, test: F)
where
    F: FnOnce(FakeChatGpt, Client, PathBuf) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .build();
    fake.install(&mut sim);
    sim.client("tau", async move {
        let dir = tempfile::tempdir().unwrap();
        let chatgpt = client(&fake, dir.path());
        // The directory lives until the test is done.
        test(fake, chatgpt, dir.path().to_owned()).await;
        drop(dir);
        Ok(())
    });
    sim.run().unwrap();
}

fn client(fake: &FakeChatGpt, dir: &std::path::Path) -> Client {
    ChatGpt::with_dialer(Store::open(dir).unwrap(), SimDialer, fake.config())
}

const REDIRECT: RedirectUri = RedirectUri { port: 1455 };

/// The whole browser round trip, through a pasted redirect URL.
async fn sign_in(
    chatgpt: &Client,
    fake: &FakeChatGpt,
    account: Option<&AccountId>,
) -> Result<SignedIn, ChatGptError> {
    let sign_in = chatgpt.start_sign_in(account, REDIRECT, false)?;
    let returned = fake.approve(sign_in.url());
    let callback = sign_in.callback(&returned)?;
    chatgpt.finish_sign_in(&sign_in, &callback).await
}

fn param(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn a_first_sign_in_registers_a_client_and_saves_it() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, dir| async move {
        let signed_in = sign_in(&chatgpt, &fake, None).await.unwrap();
        assert_eq!(signed_in.plan_usage, PlanUsage::Enabled);
        assert_eq!(signed_in.email.as_deref(), Some("user@example.com"));
        assert_eq!(fake.clients(), vec!["oaiapp_test1".to_owned()]);
        assert_eq!(fake.violations(), Vec::<String>::new());

        let store = chatgpt.store();
        let host = store.host_id().unwrap();
        let asked = &fake.authorizations()[0];
        assert_eq!(
            param(asked, "client_id").as_deref(),
            Some("dynamic_agent_client")
        );
        assert_eq!(param(asked, "agent_name_hint").as_deref(), Some("tau"));
        assert_eq!(
            param(asked, "ext_agent_host_id").as_deref(),
            Some(host.as_str())
        );
        assert_eq!(
            param(asked, "redirect_uri").as_deref(),
            Some("http://127.0.0.1:1455/auth/callback")
        );
        assert_eq!(param(asked, "id_token_hint"), None);

        assert_eq!(store.active().unwrap(), Some(signed_in.account.clone()));
        let saved = store.load(&signed_in.account).unwrap();
        assert_eq!(saved.client_id, "oaiapp_test1");
        assert_eq!(saved.subject, "user-sub-1");
        assert_eq!(saved.issuer, "https://auth.openai.com");
        assert_eq!(saved.ext_agent_host_id, host);
        assert_eq!(saved.expires_at, Some(fake.now() + 3600));
        assert!(
            saved
                .scopes
                .contains(&"chatgpt.tokens.use.direct".to_owned())
        );
        assert!(saved.id_token.is_some() && saved.refresh_token.is_some());
        assert_eq!(saved.status(), AccountStatus::SignedIn(PlanUsage::Enabled));
        assert!(!format!("{saved:?}").contains("at-"), "no tokens in Debug");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir
                .join("accounts")
                .join(format!("{}.json", signed_in.account));
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(dir.join("host.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let token = chatgpt.inference_token(&signed_in.account).await.unwrap();
        assert!(token.starts_with("at-"));
    });
}

#[test]
fn signing_in_again_reuses_the_client_host_and_hints() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let first = sign_in(&chatgpt, &fake, None).await.unwrap();
        let saved = chatgpt.store().load(&first.account).unwrap();
        let again = sign_in(&chatgpt, &fake, Some(&first.account))
            .await
            .unwrap();
        assert_eq!(again.account, first.account);
        assert_eq!(fake.clients().len(), 1, "no second registration");
        assert_eq!(fake.violations(), Vec::<String>::new());

        let [first_ask, second_ask] = &fake.authorizations()[..] else {
            panic!("two authorizations");
        };
        assert_eq!(
            param(second_ask, "client_id").as_deref(),
            Some("oaiapp_test1")
        );
        assert_eq!(param(second_ask, "agent_name_hint"), None);
        assert_eq!(param(second_ask, "id_token_hint"), saved.id_token);
        assert_eq!(
            param(second_ask, "login_hint").as_deref(),
            Some("user@example.com")
        );
        assert_eq!(
            param(second_ask, "ext_agent_host_id"),
            param(first_ask, "ext_agent_host_id")
        );
        assert_ne!(param(second_ask, "state"), param(first_ask, "state"));
        assert_ne!(param(second_ask, "nonce"), param(first_ask, "nonce"));
        let resaved = chatgpt.store().load(&first.account).unwrap();
        assert_ne!(resaved.access_token, saved.access_token);
        assert_eq!(resaved.label, saved.label);
    });
}

#[test]
fn a_returning_sign_in_as_someone_else_is_refused() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let first = sign_in(&chatgpt, &fake, None).await.unwrap();
        let saved = chatgpt.store().load(&first.account).unwrap();
        fake.set_user("user-sub-2", "other@example.com");
        let error = sign_in(&chatgpt, &fake, Some(&first.account))
            .await
            .unwrap_err();
        assert!(matches!(error, ChatGptError::IdentityMismatch), "{error}");
        assert_eq!(chatgpt.store().load(&first.account).unwrap(), saved);
    });
}

#[test]
fn a_declined_consent_stops_the_sign_in() {
    let fake = FakeChatGpt::new();
    fake.consent(Consent::Deny);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let error = sign_in(&chatgpt, &fake, None).await.unwrap_err();
        assert!(matches!(error, ChatGptError::ConsentDeclined), "{error}");
        assert!(chatgpt.store().accounts().unwrap().is_empty());
        assert_eq!(chatgpt.store().active().unwrap(), None);
    });
}

#[test]
fn a_registration_without_a_client_id_is_incomplete() {
    let fake = FakeChatGpt::new();
    fake.consent(Consent::OmitClientId);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let error = sign_in(&chatgpt, &fake, None).await.unwrap_err();
        assert!(
            matches!(error, ChatGptError::RegistrationIncomplete),
            "{error}"
        );
        assert_eq!(error.recovery(), Recovery::SignInAgain);
        assert!(chatgpt.store().accounts().unwrap().is_empty());
    });
}

#[test]
fn a_reauthorization_for_another_client_is_refused() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let first = sign_in(&chatgpt, &fake, None).await.unwrap();
        let saved = chatgpt.store().load(&first.account).unwrap();
        fake.consent(Consent::OtherClientId);
        let error = sign_in(&chatgpt, &fake, Some(&first.account))
            .await
            .unwrap_err();
        match error {
            ChatGptError::ClientMismatch { expected, got } => {
                assert_eq!(expected, "oaiapp_test1");
                assert_eq!(got, "oaiapp_someone_else");
            }
            other => panic!("{other}"),
        }
        assert_eq!(chatgpt.store().load(&first.account).unwrap(), saved);
    });
}

#[test]
fn a_callback_with_another_state_is_refused() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let sign_in = chatgpt.start_sign_in(None, REDIRECT, false).unwrap();
        let returned = fake.approve(sign_in.url());
        let mut url = url::Url::parse(&returned).unwrap();
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .into_owned()
            .map(|(k, v)| {
                if k == "state" {
                    (k, "forged".into())
                } else {
                    (k, v)
                }
            })
            .collect();
        url.query_pairs_mut().clear().extend_pairs(pairs);
        let error = sign_in.callback(url.as_str()).unwrap_err();
        assert!(matches!(error, ChatGptError::StateMismatch), "{error}");
    });
}

#[test]
fn a_used_code_is_rejected_for_a_fresh_sign_in() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let sign_in = chatgpt.start_sign_in(None, REDIRECT, false).unwrap();
        let callback = sign_in.callback(&fake.approve(sign_in.url())).unwrap();
        chatgpt.finish_sign_in(&sign_in, &callback).await.unwrap();
        let error = chatgpt
            .finish_sign_in(&sign_in, &callback)
            .await
            .unwrap_err();
        assert!(matches!(error, ChatGptError::CodeRejected), "{error}");
    });
}

#[test]
fn bad_id_tokens_are_refused_and_nothing_is_saved() {
    let cases = [
        (IdTokenFault::WrongNonce, IdTokenError::Nonce),
        (IdTokenFault::WrongAudience, IdTokenError::Audience),
        (
            IdTokenFault::WrongIssuer,
            IdTokenError::Issuer(Some("https://auth.example".into())),
        ),
        (IdTokenFault::Expired, IdTokenError::Expired),
        (IdTokenFault::WrongKey, IdTokenError::Signature),
    ];
    for (fault, expected) in cases {
        let fake = FakeChatGpt::new();
        fake.fault_id_token(fault);
        simulate(fake, move |fake, chatgpt, _dir| async move {
            let error = sign_in(&chatgpt, &fake, None).await.unwrap_err();
            match error {
                ChatGptError::IdToken(got) => assert_eq!(got, expected),
                other => panic!("{fault:?}: {other}"),
            }
            assert!(chatgpt.store().accounts().unwrap().is_empty());
        });
    }
}

#[test]
fn a_sign_in_without_plan_usage_is_kept_but_cannot_infer() {
    let fake = FakeChatGpt::new();
    fake.consent(Consent::GrantWithoutPlanUsage);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let signed_in = sign_in(&chatgpt, &fake, None).await.unwrap();
        assert_eq!(signed_in.plan_usage, PlanUsage::Disabled);
        let saved = chatgpt.store().load(&signed_in.account).unwrap();
        assert_eq!(
            saved.status(),
            AccountStatus::SignedIn(PlanUsage::Disabled)
        );
        let error = chatgpt
            .inference_token(&signed_in.account)
            .await
            .unwrap_err();
        assert!(matches!(error, ChatGptError::PlanUsageDisabled));
        assert_eq!(error.recovery(), Recovery::EnablePlanUsage);

        // Enabling it later: the saved client, asking for consent.
        let again = chatgpt
            .start_sign_in(Some(&signed_in.account), REDIRECT, true)
            .unwrap();
        let callback = again.callback(&fake.approve(again.url())).unwrap();
        let enabled = chatgpt.finish_sign_in(&again, &callback).await.unwrap();
        assert_eq!(enabled.plan_usage, PlanUsage::Enabled);
        let asked = fake.authorizations().pop().unwrap();
        assert_eq!(param(&asked, "prompt").as_deref(), Some("consent"));
        assert_eq!(param(&asked, "client_id").as_deref(), Some("oaiapp_test1"));
    });
}

#[test]
fn an_expiring_token_is_refreshed_and_rotated() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let before = chatgpt.store().load(&account).unwrap();

        fake.advance(60);
        let token = chatgpt.access_token(&account).await.unwrap();
        assert_eq!(Some(token), before.access_token, "not due yet");
        assert_eq!(fake.refresh_grants(), 0);

        fake.advance(3600 - 60 - 4 * 60);
        let token = chatgpt.access_token(&account).await.unwrap();
        let after = chatgpt.store().load(&account).unwrap();
        assert_eq!(fake.refresh_grants(), 1);
        assert_eq!(Some(token), after.access_token);
        assert_ne!(after.access_token, before.access_token);
        assert_ne!(after.refresh_token, before.refresh_token);
        assert!(
            !fake.refresh_token_is_live(
                before.refresh_token.as_deref().unwrap()
            )
        );
        assert_eq!(after.expires_at, Some(fake.now() + 3600));
        assert_eq!(after.scopes, before.scopes);
        assert_eq!(after.id_token, before.id_token);
        assert_eq!(fake.violations(), Vec::<String>::new());
    });
}

#[test]
fn concurrent_refreshes_take_turns() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        fake.advance(3600);
        // A clone shares the in-process turn; a second client on the same
        // store, as another process would be, only shares the lock file.
        let clone = chatgpt.clone();
        let other = client(&fake, &dir);
        let (a, b, c) = tokio::join!(
            chatgpt.access_token(&account),
            clone.access_token(&account),
            other.access_token(&account),
        );
        let (a, b, c) = (a.unwrap(), b.unwrap(), c.unwrap());
        assert_eq!(fake.refresh_grants(), 1, "one refresh for three callers");
        assert_eq!(a, b);
        assert_eq!(b, c);
    });
}

#[test]
fn a_dead_refresh_token_asks_for_a_new_sign_in() {
    for code in [
        "invalid_grant",
        "invalid_refresh_token",
        "token_expired",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "refresh_token_reused",
    ] {
        let fake = FakeChatGpt::new();
        fake.fail_refresh(400, json!({"error": code}));
        simulate(fake, move |fake, chatgpt, _dir| async move {
            let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
            let error = chatgpt.refresh(&account).await.unwrap_err();
            assert!(
                matches!(&error, ChatGptError::SignInRequired { reason: Some(r) } if r == code),
                "{code}: {error}"
            );
            assert_eq!(error.recovery(), Recovery::SignInAgain);
            let saved = chatgpt.store().load(&account).unwrap();
            assert_eq!(saved.status(), AccountStatus::SignedOut);
            assert_eq!(
                saved.client_id, "oaiapp_test1",
                "the registration stays"
            );
            assert!(saved.id_token.is_some(), "the hint stays");
            // The next sign-in reuses the client.
            sign_in(&chatgpt, &fake, Some(&account)).await.unwrap();
            assert_eq!(fake.clients().len(), 1);
        });
    }
}

#[test]
fn an_invalid_client_or_outage_keeps_the_tokens() {
    let fake = FakeChatGpt::new();
    fake.fail_refresh(401, json!({"error": "invalid_client"}));
    fake.fail_refresh(503, json!({"error": "temporarily_unavailable"}));
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let error = chatgpt.refresh(&account).await.unwrap_err();
        assert!(matches!(error, ChatGptError::InvalidClient(_)), "{error}");
        assert_eq!(error.recovery(), Recovery::FixClient);
        let error = chatgpt.refresh(&account).await.unwrap_err();
        assert_eq!(error.recovery(), Recovery::RetryLater, "{error}");
        let saved = chatgpt.store().load(&account).unwrap();
        assert_eq!(saved.status(), AccountStatus::SignedIn(PlanUsage::Enabled));
        chatgpt.refresh(&account).await.unwrap();
    });
}

#[test]
fn signing_out_revokes_then_clears_the_tokens() {
    simulate(FakeChatGpt::new(), |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let host = chatgpt.store().host_id().unwrap();
        let refresh = chatgpt
            .store()
            .load(&account)
            .unwrap()
            .refresh_token
            .unwrap();
        assert_eq!(
            chatgpt.sign_out(&account).await.unwrap(),
            Revocation::Confirmed
        );
        let form = &fake.revocations()[0];
        assert_eq!(param(form, "token"), Some(refresh.clone()));
        assert_eq!(
            param(form, "token_type_hint").as_deref(),
            Some("refresh_token")
        );
        assert_eq!(param(form, "client_id").as_deref(), Some("oaiapp_test1"));
        assert!(!fake.refresh_token_is_live(&refresh));

        let saved = chatgpt.store().load(&account).unwrap();
        assert_eq!(saved.status(), AccountStatus::SignedOut);
        assert_eq!(saved.id_token, None, "no id_token_hint after sign-out");
        assert_eq!(saved.client_id, "oaiapp_test1");
        assert_eq!(chatgpt.store().host_id().unwrap(), host);
        let error = chatgpt.access_token(&account).await.unwrap_err();
        assert!(matches!(error, ChatGptError::SignInRequired { .. }));
        assert_eq!(
            chatgpt.sign_out(&account).await.unwrap(),
            Revocation::NothingToRevoke
        );

        sign_in(&chatgpt, &fake, Some(&account)).await.unwrap();
        let asked = fake.authorizations().pop().unwrap();
        assert_eq!(param(&asked, "client_id").as_deref(), Some("oaiapp_test1"));
        assert_eq!(param(&asked, "id_token_hint"), None);
        assert_eq!(
            param(&asked, "login_hint").as_deref(),
            Some("user@example.com")
        );
    });
}

#[test]
fn revocation_retries_outages_and_reports_what_it_could_not_confirm() {
    let fake = FakeChatGpt::new();
    fake.revoke_statuses(&[503, 502, 200]);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        assert_eq!(
            chatgpt.sign_out(&account).await.unwrap(),
            Revocation::Confirmed
        );
        assert_eq!(fake.revocations().len(), 3);
    });

    let fake = FakeChatGpt::new();
    fake.revoke_statuses(&[503; 4]);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let revocation = chatgpt.sign_out(&account).await.unwrap();
        assert!(matches!(revocation, Revocation::Unconfirmed { .. }));
        assert_eq!(fake.revocations().len(), 4);
        let saved = chatgpt.store().load(&account).unwrap();
        assert_eq!(saved.status(), AccountStatus::SignedOut, "cleared anyway");
    });

    let fake = FakeChatGpt::new();
    fake.revoke_statuses(&[400]);
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let revocation = chatgpt.sign_out(&account).await.unwrap();
        assert!(matches!(revocation, Revocation::Unconfirmed { .. }));
        assert_eq!(fake.revocations().len(), 1, "a 4xx is not retried");
    });
}

#[test]
fn the_model_list_keeps_listed_models_in_order() {
    let fake = FakeChatGpt::new();
    fake.set_models(json!([
        {"slug": "gpt-6.1-sol", "display_name": "GPT-6.1 Sol", "visibility": "list"},
        {"slug": "internal", "display_name": "Internal", "visibility": "hide"},
        {"slug": "gpt-5.5", "display_name": "GPT-5.5", "visibility": "list"},
    ]));
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let models = chatgpt.models(&account).await.unwrap();
        let slugs: Vec<&str> = models.iter().map(|m| m.slug.as_str()).collect();
        assert_eq!(slugs, ["gpt-6.1-sol", "gpt-5.5"]);
        assert_eq!(models[0].display_name, "GPT-6.1 Sol");
    });
}

#[test]
fn api_errors_keep_status_body_and_request_id() {
    let fake = FakeChatGpt::new();
    fake.fail_models(403, json!({"detail": "Region not supported"}));
    simulate(fake, |fake, chatgpt, _dir| async move {
        let account = sign_in(&chatgpt, &fake, None).await.unwrap().account;
        let error = chatgpt.models(&account).await.unwrap_err();
        let ChatGptError::Api(api) = &error else {
            panic!("{error}");
        };
        assert_eq!(api.status, 403);
        assert_eq!(api.body, r#"{"detail":"Region not supported"}"#);
        assert!(
            api.request_id
                .as_deref()
                .is_some_and(|id| id.starts_with("req_fake_"))
        );
        assert_eq!(error.recovery(), Recovery::Restricted);
    });
}

#[test]
fn the_host_id_is_made_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let id = store.host_id().unwrap();
    assert!(id.as_str().starts_with("urn:uuid:"));
    assert_eq!(store.host_id().unwrap(), id);
    assert_eq!(Store::open(dir.path()).unwrap().host_id().unwrap(), id);
}
