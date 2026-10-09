//! The credential store against a model: whatever order accounts are
//! saved, replaced and chosen in, the store lists, loads and names the
//! active one exactly as a map and a pointer would, and its files stay
//! private to the owner.

use std::{collections::BTreeMap, os::unix::fs::PermissionsExt as _};

use hegel::{
    TestCase,
    generators::{self as gs, PrintableGenerator},
};
use tau_ai::chatgpt::{AccountId, ChatGptError, Credentials, HostId, Store};

/// Four registrations, so saves collide and replace each other.
const POOL: [(&str, &str); 4] = [
    ("oaiapp_one", "sub-a"),
    ("oaiapp_one", "sub-b"),
    ("oaiapp_two", "sub-a"),
    ("oaiapp_two/with odd:chars", "sub-é"),
];

fn record(
    tc: &TestCase,
    host: &HostId,
    (client_id, subject): (&str, &str),
) -> Credentials {
    let token =
        || gs::optional(gs::from_regex("tok_[a-z0-9]{6}").fullmatch(true));
    Credentials {
        label: tc.draw(gs::from_regex("[a-z]{1,6}").fullmatch(true)),
        email: tc.draw(gs::optional(gs::just("me@example.com".to_owned()))),
        issuer: "https://auth.openai.com".into(),
        subject: subject.into(),
        client_id: client_id.into(),
        ext_agent_host_id: host.clone(),
        id_token: tc.draw(token()),
        access_token: tc.draw(token()),
        refresh_token: tc.draw(token()),
        token_type: Some("Bearer".into()),
        expires_in: None,
        expires_at: tc
            .draw(gs::optional(gs::integers::<u64>().max_value(1 << 40))),
        earliest_refresh_at: None,
        scopes: vec![],
        saved_at: "2026-10-09T00:00:00Z".into(),
    }
}

fn which() -> impl PrintableGenerator<usize> {
    gs::integers::<usize>().max_value(POOL.len() - 1)
}

struct StoreModel {
    dir: tempfile::TempDir,
    store: Store,
    host: HostId,
    saved: BTreeMap<AccountId, Credentials>,
    active: Option<AccountId>,
}

#[hegel::state_machine]
impl StoreModel {
    #[rule]
    fn save(&mut self, tc: TestCase) {
        let credentials = record(&tc, &self.host, POOL[tc.draw(which())]);
        self.store.save(&credentials).unwrap();
        self.saved.insert(credentials.id(), credentials);
    }

    #[rule]
    fn choose(&mut self, tc: TestCase) {
        let (client, subject) = POOL[tc.draw(which())];
        let id = AccountId::new(client, subject);
        match self.store.set_active(&id) {
            Ok(()) => {
                assert!(self.saved.contains_key(&id));
                self.active = Some(id);
            }
            Err(ChatGptError::UnknownAccount(name)) => {
                assert!(!self.saved.contains_key(&id));
                assert_eq!(name, id.to_string());
            }
            Err(other) => panic!("{other}"),
        }
    }

    #[rule]
    fn sign_out(&mut self, tc: TestCase) {
        let (client, subject) = POOL[tc.draw(which())];
        let id = AccountId::new(client, subject);
        let Some(model) = self.saved.get_mut(&id) else {
            return;
        };
        let mut loaded = self.store.load(&id).unwrap();
        loaded.clear_tokens();
        self.store.save(&loaded).unwrap();
        model.clear_tokens();
    }

    #[invariant]
    fn the_store_is_the_model(&mut self, _tc: TestCase) {
        for (id, credentials) in &self.saved {
            assert_eq!(&self.store.load(id).unwrap(), credentials);
        }
        let mut listed = self.store.accounts().unwrap();
        listed.sort_by_key(Credentials::id);
        let wanted: Vec<_> = self.saved.values().cloned().collect();
        assert_eq!(listed, wanted);
        assert!(
            self.store
                .accounts()
                .unwrap()
                .windows(2)
                .all(|pair| pair[0].label <= pair[1].label),
            "listed by label"
        );
        assert_eq!(self.store.active().unwrap(), self.active);
        assert_eq!(self.store.host_id().unwrap(), self.host);
    }

    #[invariant]
    fn nothing_is_readable_by_anyone_else(&mut self, _tc: TestCase) {
        let mode = |path: &std::path::Path| {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        };
        let dir = &self.dir.path().join("chatgpt");
        assert_eq!(mode(dir) & 0o077, 0, "the store directory");
        assert_eq!(mode(&dir.join("accounts")) & 0o077, 0);
        for entry in std::fs::read_dir(dir.join("accounts")).unwrap() {
            let path = entry.unwrap().path();
            assert_eq!(mode(&path) & 0o077, 0, "{}", path.display());
        }
    }
}

#[hegel::test(test_cases = 100)]
fn the_store_matches_a_map_and_a_pointer(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("chatgpt")).unwrap();
    let host = store.host_id().unwrap();
    let model = StoreModel {
        dir,
        store,
        host,
        saved: BTreeMap::new(),
        active: None,
    };
    hegel::stateful::machine(model).steps(25).run(tc);
}
