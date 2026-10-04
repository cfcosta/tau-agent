//! A host and phones in one process, over real TLS and WebSockets on
//! the loopback.

use std::time::Duration;

use serde_json::{Value, json};
use tau_remote::{
    Address,
    ClientError,
    Down,
    Fingerprint,
    Refusal,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    client,
    outbox::Outbox,
    server::REPLAY,
};
use tokio::sync::mpsc::UnboundedReceiver;

struct Host {
    handle: ServerHandle,
    events: UnboundedReceiver<ServerEvent>,
    address: Address,
    _dir: tempfile::TempDir,
}

async fn host() -> Host {
    let dir = tempfile::tempdir().unwrap();
    let (handle, events) = Server::start(ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        dir: dir.path().to_owned(),
        host: "desk".into(),
    })
    .await
    .unwrap();
    let address = Address {
        host: "127.0.0.1".into(),
        port: handle.local_addr().port(),
    };
    Host {
        handle,
        events,
        address,
        _dir: dir,
    }
}

impl Host {
    /// The next event that is not a change to the list of phones.
    async fn event(&mut self) -> ServerEvent {
        loop {
            let event = tokio::time::timeout(
                Duration::from_secs(5),
                self.events.recv(),
            )
            .await
            .expect("an event in time")
            .expect("the server runs");
            if !matches!(event, ServerEvent::DevicesChanged(_)) {
                return event;
            }
        }
    }

    async fn pair(&mut self) -> (client::Connection, client::Credentials) {
        let code = self.handle.pairing_code(self.address.clone());
        let paired = client::pair(
            &self.address,
            code.fingerprint,
            &code.secret,
            "Pixel",
        )
        .await
        .unwrap();
        // A new phone needs everything.
        let ServerEvent::NeedSnapshot { conn, device } = self.event().await
        else {
            panic!("no snapshot asked for");
        };
        assert_eq!(device.name, "Pixel");
        self.handle.snapshot(conn, vec![json!("everything")]);
        paired
    }
}

async fn recv(connection: &mut client::Connection) -> Down {
    connection.recv().await.unwrap().expect("a message")
}

fn body(down: Down) -> Value {
    match down {
        Down::Message { body, .. } => body,
        other => panic!("{other:?}, not a message"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_and_messages_go_both_ways() {
    let mut host = host().await;
    let (mut phone, credentials) = host.pair().await;
    assert_eq!(credentials.host, "desk");
    assert_eq!(credentials.fingerprint, host.handle.fingerprint());
    assert_eq!(host.handle.paired().len(), 1);
    let Down::Snapshot { seq, bodies } = recv(&mut phone).await else {
        panic!("no snapshot");
    };
    assert_eq!(bodies, [json!("everything")]);
    host.handle.broadcast(json!({"run": 1}));
    let message = recv(&mut phone).await;
    assert_eq!(message.seq(), Some(seq + 1));
    assert_eq!(body(message), json!({"run": 1}));
    let mut outbox = Outbox::new(0);
    phone
        .sender()
        .send(outbox.push(json!({"steer": "go"})))
        .unwrap();
    let ServerEvent::Up { device, body, .. } = host.event().await else {
        panic!("nothing came up");
    };
    assert_eq!(device.id, credentials.device);
    assert_eq!(body, json!({"steer": "go"}));
    assert_eq!(recv(&mut phone).await, Down::Ack { up: 1 });
}

/// A request the phone sends again, its answer lost with a connection,
/// is answered again and taken once; the
/// next is taken.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_sent_again_is_taken_once() {
    let mut host = host().await;
    let (mut phone, credentials) = host.pair().await;
    let last = recv(&mut phone).await.seq().unwrap();
    let mut outbox = Outbox::new(0);
    let first = outbox.push(json!("first"));
    phone.sender().send(first.clone()).unwrap();
    assert!(matches!(host.event().await, ServerEvent::Up { .. }));
    assert_eq!(recv(&mut phone).await, Down::Ack { up: 1 });
    // Its answer was lost: the phone sends it again on a new connection.
    drop(phone);
    let mut phone = client::resume(&credentials, Some(last)).await.unwrap();
    phone.sender().send(first).unwrap();
    assert_eq!(recv(&mut phone).await, Down::Ack { up: 1 });
    phone.sender().send(outbox.push(json!("second"))).unwrap();
    let ServerEvent::Up { body, .. } = host.event().await else {
        panic!("nothing came up");
    };
    assert_eq!(body, json!("second"), "the first was not taken again");
    assert_eq!(recv(&mut phone).await, Down::Ack { up: 2 });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_or_used_secret_is_refused() {
    let mut host = host().await;
    let code = host.handle.pairing_code(host.address.clone());
    let wrong = tau_remote::PairingSecret::typed("2222-2222").unwrap();
    let refused =
        client::pair(&host.address, code.fingerprint, &wrong, "Pixel").await;
    assert!(matches!(
        refused,
        Err(ClientError::Refused(Refusal::BadSecret))
    ));
    host.pair().await;
    let used =
        client::pair(&host.address, code.fingerprint, &code.secret, "Other")
            .await;
    assert!(matches!(
        used,
        Err(ClientError::Refused(Refusal::BadSecret))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_that_comes_back_gets_what_it_missed() {
    let mut host = host().await;
    let (mut phone, credentials) = host.pair().await;
    let last = recv(&mut phone).await.seq().unwrap();
    drop(phone);
    host.handle.broadcast(json!(1));
    host.handle.broadcast(json!(2));
    let mut phone = client::resume(&credentials, Some(last)).await.unwrap();
    assert!(phone.resumed());
    assert_eq!(body(recv(&mut phone).await), json!(1));
    let second = recv(&mut phone).await;
    assert_eq!(second.seq(), Some(last + 2));
    assert_eq!(body(second), json!(2));
    // New messages follow.
    host.handle.broadcast(json!(3));
    assert_eq!(body(recv(&mut phone).await), json!(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_away_too_long_gets_a_snapshot() {
    let mut host = host().await;
    let (mut phone, credentials) = host.pair().await;
    let last = recv(&mut phone).await.seq().unwrap();
    drop(phone);
    for n in 0..=REPLAY {
        host.handle.broadcast(json!(n));
    }
    let mut phone = client::resume(&credentials, Some(last)).await.unwrap();
    assert!(!phone.resumed());
    let ServerEvent::NeedSnapshot { conn, .. } = host.event().await else {
        panic!("no snapshot asked for");
    };
    host.handle.snapshot(conn, vec![json!("again")]);
    let Down::Snapshot { seq, bodies } = recv(&mut phone).await else {
        panic!("no snapshot");
    };
    assert_eq!(seq, last + REPLAY as u64 + 1);
    assert_eq!(bodies, [json!("again")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_phone_is_cut_off() {
    let mut host = host().await;
    let (mut phone, credentials) = host.pair().await;
    recv(&mut phone).await;
    assert!(host.handle.revoke(&credentials.device).unwrap());
    assert!(matches!(phone.recv().await, Ok(None) | Err(_)));
    let refused = client::resume(&credentials, None).await;
    assert!(matches!(
        refused,
        Err(ClientError::Refused(Refusal::UnknownToken))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn another_certificate_is_caught() {
    let host = host().await;
    assert_eq!(
        client::probe(&host.address).await.unwrap(),
        host.handle.fingerprint()
    );
    let code = host.handle.pairing_code(host.address.clone());
    let other = Fingerprint([7; 32]);
    let caught =
        client::pair(&host.address, other, &code.secret, "Pixel").await;
    let Err(ClientError::CertificateMismatch { found }) = caught else {
        panic!("not caught: {:?}", caught.err());
    };
    assert_eq!(found, host.handle.fingerprint());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_listening_is_unreachable() {
    let host = host().await;
    let address = host.address.clone();
    host.handle.stop();
    drop(host);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let error = client::probe(&address).await.unwrap_err();
    assert!(error.is_unreachable(), "{error}");
}
