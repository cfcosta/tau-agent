//! Phones, as a plugin (decision 0013, ADR 0045): once "Allow phones" is
//! on, tau listens for them on this computer, pairs them by a code they
//! scan, and carries the window's mirror both ways. What crosses and
//! what a phone may ask stay the kernel's ([`tau_ui_plugin::app`]); this
//! plugin only carries it, so it reaches nothing a phone may not.
//!
//! The Phones page is the plugin's, listed in the sidebar's Everywhere.
//! It stays on the computer (`computer_only`): its data holds the
//! pairing code.

#[cfg(feature = "host")]
mod bridge;
#[cfg(feature = "host")]
mod half;

#[cfg(feature = "host")]
pub use bridge::Phones;
#[cfg(feature = "host")]
pub use half::PhonesHost;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_kit::{assets::Icon, theme::Tone as Ink};
use tau_ui_plugin::{
    Declaration,
    Link,
    Manifest,
    NavEntry,
    Page,
    UiPlugin,
    ViewCx,
    points::{self, AtApp},
    services::names,
    tree::{KeyValue, Kind, Node, Tone},
};

pub const NAME: &str = "tau-phones";
/// The Phones page.
pub const PAGE: &str = "phones";
/// The page's one action: its `args` are the [`Act`] to carry out.
pub const ACT: &str = "act";

/// What the Phones page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhonesData {
    /// "Allow phones": off until the person turns it on.
    pub allowed: bool,
    /// Where a phone could reach this computer.
    pub addresses: Vec<LocalAddress>,
    /// Where tau listens, once it does: `100.84.12.7:7443`.
    pub listening: Option<String>,
    pub code: Option<ShownCode>,
    pub paired: Vec<Paired>,
    /// Why tau could not listen, or what last went wrong.
    pub error: Option<String>,
}

/// An address of this computer a phone could reach it at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAddress {
    pub ip: String,
    /// Where it is, in words: `Tailscale`, `wlan0`.
    pub label: String,
}

/// A pairing code on screen, until it expires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownCode {
    /// The code as the QR code holds it.
    pub text: String,
    /// Where it reaches the computer.
    pub address: String,
    pub secret: String,
    /// The certificate's fingerprint, short.
    pub certificate: String,
    /// The whole fingerprint, in lines, for a phone that types the
    /// address to compare with.
    pub lines: Vec<String>,
    /// When it stops working, in milliseconds since 1970.
    pub expires: i64,
}

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    pub id: String,
    pub name: String,
    /// When it paired: `2026-09-30T12:03:11Z`.
    pub paired_at: String,
    pub last_seen: Option<String>,
}

/// What the person asked of the Phones page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "kebab-case")]
pub enum Act {
    Allow {
        allow: bool,
    },
    /// Listen on this IP.
    ListenOn {
        ip: String,
    },
    ShowCode,
    HideCode,
    /// Refuse this phone's token and close its connection.
    Revoke {
        id: String,
    },
}

/// tau-phones with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhonesUi;

impl UiPlugin for PhonesUi {
    type State = ();
    type Data = PhonesData;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new().services(&[names::APP]).computer_only()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new(PAGE, page)
                    .title(|_| "Phones".to_owned())
                    .framed(),
            )
            .contribute(points::EVERYWHERE, |_: &AtApp, view| {
                let paired = view.data.paired.len();
                Some(
                    NavEntry::new("Phones", Icon::Phone, Link::page(PAGE))
                        .badge(
                            (paired > 0).then(|| {
                                (format!("{paired} paired"), Ink::Good)
                            }),
                        ),
                )
            })
    }

    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == ACT
            && let Ok(act) = serde_json::from_value::<Act>(args.clone())
        {
            view.handle.act(act);
        }
    }
}

fn button(label: &str, act: &Act, kind: Kind) -> Node {
    Node::Button {
        label: label.into(),
        action: ACT.into(),
        args: json!(act),
        kind,
        icon: None,
    }
}

/// The Phones page: "Allow phones", where tau listens, the phones
/// paired, and the card that pairs one.
fn page(view: &mut ViewCx<'_, PhonesUi>) -> Node {
    let phones = &view.data;
    let mut left = vec![Node::Card {
        children: vec![Node::Switch {
            label: "Allow phones".into(),
            detail: Some(
                "Off by default. Phones reach tau only while this app is \
                 open."
                    .into(),
            ),
            on: phones.allowed,
            action: ACT.into(),
            args: json!(Act::Allow {
                allow: !phones.allowed
            }),
        }],
    }];
    if phones.allowed {
        left.push(listen_on(phones));
    }
    if let Some(error) = &phones.error {
        left.push(Node::Toned {
            text: error.clone(),
            tone: Tone::Bad,
            mono: false,
        });
    }
    left.push(paired(phones));
    let mut children = vec![
        Node::Title {
            text: "Phones".into(),
            subtitle: Some(
                "Reach tau from your phone while this app runs: on your \
                 network, or through your VPN when you are away."
                    .into(),
            ),
        },
        Node::stack(left),
    ];
    if phones.allowed {
        children.push(pair_card(phones));
    }
    Node::stack(children)
}

fn listen_on(phones: &PhonesData) -> Node {
    let chosen = phones
        .listening
        .as_deref()
        .and_then(|address| address.rsplit_once(':'))
        .map(|(host, _)| host);
    let addresses = phones.addresses.iter().map(|address| {
        let label = format!("{} · {}", address.label, address.ip);
        let kind = if chosen == Some(address.ip.as_str()) {
            Kind::Primary
        } else {
            Kind::Secondary
        };
        button(
            &label,
            &Act::ListenOn {
                ip: address.ip.clone(),
            },
            kind,
        )
    });
    Node::stack([
        Node::Heading {
            text: "Listen on".into(),
        },
        Node::row(addresses),
        Node::Toned {
            text: match &phones.listening {
                Some(address) => format!("Listening on {address}."),
                None => "Not listening.".into(),
            },
            tone: Tone::Neutral,
            mono: false,
        },
    ])
}

/// `2026-09-30T12:03:11Z` as `2026-09-30 12:03`.
fn when(stamp: &str) -> String {
    stamp
        .get(..16)
        .map_or_else(|| stamp.to_owned(), |minute| minute.replace('T', " "))
}

fn paired(phones: &PhonesData) -> Node {
    let mut children = vec![Node::Heading {
        text: "Paired phones".into(),
    }];
    if phones.paired.is_empty() {
        children.push(Node::Empty {
            text: "No phones yet.".into(),
        });
    }
    for phone in &phones.paired {
        let seen = phone.last_seen.as_deref().map_or_else(
            || "never connected".to_owned(),
            |seen| format!("last seen {}", when(seen)),
        );
        children.push(Node::row([
            Node::Icon {
                name: "phone".into(),
                tone: Tone::Neutral,
            },
            Node::Status {
                text: phone.name.clone(),
                detail: Some(format!(
                    "paired {} · {seen}",
                    when(&phone.paired_at)
                )),
                tone: Tone::Neutral,
            },
            Node::Spacer,
            button(
                "Revoke",
                &Act::Revoke {
                    id: phone.id.clone(),
                },
                Kind::Danger,
            ),
        ]));
    }
    children.push(Node::Toned {
        text: "Revoking a phone refuses its key at once and closes its \
               connection."
            .into(),
        tone: Tone::Neutral,
        mono: false,
    });
    Node::Card { children }
}

fn pair_card(phones: &PhonesData) -> Node {
    let mut children = vec![Node::Heading {
        text: "Pair a phone".into(),
    }];
    let Some(code) = &phones.code else {
        children.push(Node::text(
            "Show a code, then scan it from tau on your phone. It works \
             once, for a few minutes.",
        ));
        if phones.listening.is_some() {
            children.push(button(
                "Show pairing code",
                &Act::ShowCode,
                Kind::Primary,
            ));
        }
        return Node::Card { children };
    };
    let row = |key: &str, value: &str| KeyValue {
        key: key.into(),
        value: value.into(),
    };
    children.extend([
        Node::Qr {
            text: code.text.clone(),
            size: 200.,
        },
        Node::text("In tau on your phone, tap Scan pairing code."),
        Node::Rows {
            rows: vec![
                row("Address", &code.address),
                row("Code", &code.secret),
                row("Certificate", &code.certificate),
            ],
        },
        Node::row([
            Node::text("Expires"),
            Node::Due {
                id: "code-expires".into(),
                at: code.expires,
            },
        ]),
        // Typing the address instead: the phone shows the whole
        // certificate, to compare with this.
        Node::row(
            code.lines
                .iter()
                .map(|line| Node::Mono { text: line.clone() }),
        ),
        button("Hide code", &Act::HideCode, Kind::Secondary),
    ]);
    Node::Card { children }
}
