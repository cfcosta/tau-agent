# tau-remote

What crosses between tau on a phone and the tau running on a computer. The
phone runs no agent: it pairs with the computer, shows its runs and steers
them. This crate holds the pairing code, the TLS identity the phone trusts
by fingerprint, the paired devices, the wire frames, and both sides of the
connection. It has no GPUI in it, so a host without a window could serve it.

## What it provides

| Module    | What it holds                                                                                                   |
| --------- | --------------------------------------------------------------------------------------------------------------- |
| `pairing` | `PairingCode`, the `tau-pair://` URI the computer shows as a QR code; `Address`, `Fingerprint`, `PairingSecret` |
| `tls`     | `Identity`, the computer's self-signed certificate, and `fingerprint_of`                                        |
| `devices` | `Device`, a paired phone; `phones.json` keeps each one's token as a SHA-256, never the token itself             |
| `wire`    | The JSON frames: `Hello`, `Answer`, `Refusal`, `Down` (computer to phone) and `Up` (phone to computer)          |
| `feed`    | `Feed`, the numbered messages, kept so a phone that comes back gets what it missed                              |
| `outbox`  | `Outbox`, the phone's requests, sent again until the computer takes them, and taken once                        |
| `server`  | The computer's side: `Server::start`, `ServerHandle`, `ServerEvent`                                             |
| `client`  | The phone's side: `probe`, `pair`, `resume`, `Connection`, `Sender`, `Credentials`                              |

The pairing code looks like this:

```text
tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp=<64 hex digits>&code=K7QM-2XPA
```

It holds the address the computer listens on, the computer's name, the
SHA-256 fingerprint of its certificate (the only thing the phone trusts it
by), and a one-time secret the phone trades for a device token. Only one
secret is open at a time, and it expires after five minutes
(`devices::SECRET_LIFETIME`).

The bodies inside `Down` and `Up` frames are the app's own JSON. This crate
does not look into them.

## How it fits

It builds on `tau-ai` (for files only the user can read, and timestamps),
tokio, rustls and tungstenite. `tau-ui` runs the server
(`tau_ui::phone_server`); `tau-ui-remote` runs the client on the phone
(`tau_ui_remote::remote`); `tau-phone` reads pairing codes with it.

## Usage

The computer starts a server and shows a code; the phone pairs with it:

```rust
use serde_json::json;
use tau_remote::{Address, Server, ServerConfig, ServerEvent, client};

let (handle, mut events) = Server::start(ServerConfig {
    listen: "0.0.0.0:7443".parse()?,
    dir: config_dir.clone(),
    host: "desk".into(),
})
.await?;
let address = Address {
    host: "100.84.12.7".into(),
    port: Address::DEFAULT_PORT,
};
let code = handle.pairing_code(address.clone());
println!("{code}"); // the tau-pair:// URI, for a QR code

// On the phone, once it has read `code`:
let (mut connection, credentials) =
    client::pair(&code.address, code.fingerprint, &code.secret, "Pixel")
        .await?;

// The computer answers a new phone with everything it shows.
if let Some(ServerEvent::NeedSnapshot { conn, .. }) = events.recv().await {
    handle.snapshot(conn, vec![json!({"runs": []})]);
}
handle.broadcast(json!({"run": 1}));
let down = connection.recv().await?;
```

The phone keeps `credentials` and connects again later with
`client::resume(&credentials, last_seq)`, which brings it the messages it
missed while the computer still has them (`feed::REPLAY`), or a fresh
snapshot otherwise.

## Testing

```sh
cargo nextest run --release -p tau-remote
```

`tests/remote.rs` runs a host and phones in one process, over real TLS and
WebSockets on the loopback. `tests/feed.rs` checks the feed with Hegel,
in process. Unit tests in `src/` cover pairing codes, TLS, devices and
frames.

## Further reading

- [docs/decisions/0013-phones-connect-to-a-running-tau.md](../../docs/decisions/0013-phones-connect-to-a-running-tau.md)
