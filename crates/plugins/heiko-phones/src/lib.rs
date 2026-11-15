//! Phones, as a plugin (decision 0013, ADR 0045): once "Allow phones" is
//! on, heiko listens for them on this computer, pairs them by a code they
//! scan, and carries the window's mirror both ways. What crosses and
//! what a phone may ask stay the kernel's ([`heiko_ui_plugin::app`]); this
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
use heiko_ui_kit::{assets::Icon, theme::Tone as Ink};
use heiko_ui_plugin::{
    Declaration,
    Link,
    Manifest,
    NavEntry,
    Page,
    UiPlugin,
    ViewCx,
    points::{self, AtApp},
    services::names,
    tree::{Kind, MarkState, Node, Tone},
};

pub const NAME: &str = "heiko-phones";
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
    /// Where heiko listens, once it does: `100.84.12.7:7443`.
    pub listening: Option<String>,
    pub code: Option<ShownCode>,
    pub paired: Vec<Paired>,
    /// Why heiko could not listen, or what last went wrong.
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

/// heiko-phones with its UI.
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
        size: Default::default(),
    }
}

/// `rows` with a hairline between each two.
fn ruled(rows: impl IntoIterator<Item = Node>) -> Vec<Node> {
    let mut ruled = Vec::new();
    for (at, row) in rows.into_iter().enumerate() {
        if at > 0 {
            ruled.push(Node::Rule);
        }
        ruled.push(row);
    }
    ruled
}

/// A section of the page: its title over its pieces.
fn section(id: &str, title: impl Into<String>, children: Vec<Node>) -> Node {
    Node::Section {
        id: format!("phones-{id}"),
        title: title.into(),
        open: true,
        children,
    }
}

/// Small print in mono.
fn small(text: impl Into<String>) -> Node {
    Node::Toned {
        text: text.into(),
        tone: Tone::Neutral,
        mono: true,
    }
}

/// The Phones page: "Allow phones", where heiko listens, the phones
/// paired, and what pairs one.
fn page(view: &mut ViewCx<'_, PhonesUi>) -> Node {
    let phones = &view.data;
    let mut allow = vec![Node::Switch {
        label: "Allow phones".into(),
        detail: Some("Only while this app is open.".into()),
        on: phones.allowed,
        action: ACT.into(),
        args: json!(Act::Allow {
            allow: !phones.allowed
        }),
    }];
    if let Some(error) = &phones.error {
        allow.push(Node::Toned {
            text: error.clone(),
            tone: Tone::Bad,
            mono: false,
        });
    }
    let mut page = vec![section("allow", "Phones", allow)];
    if phones.allowed {
        page.push(listen_on(phones));
    }
    page.push(paired(phones));
    if phones.allowed {
        page.push(pair(phones));
    }
    Node::stack(page)
}

/// Where heiko listens, and a ruled row for each address it could: the
/// one it listens on ticked, the others a click away.
fn listen_on(phones: &PhonesData) -> Node {
    let chosen = phones
        .listening
        .as_deref()
        .and_then(|address| address.rsplit_once(':'))
        .map(|(host, _)| host);
    let addresses = phones.addresses.iter().map(|address| {
        let on = chosen == Some(address.ip.as_str());
        Node::row([
            Node::Mark {
                state: if on { MarkState::Met } else { MarkState::Idle },
                count: None,
            },
            if on {
                Node::Rich {
                    text: format!("**{}**", address.label),
                }
            } else {
                button(
                    &address.label,
                    &Act::ListenOn {
                        ip: address.ip.clone(),
                    },
                    Kind::Secondary,
                )
            },
            Node::Mono {
                text: address.ip.clone(),
            },
        ])
    });
    let mut children = vec![small(match &phones.listening {
        Some(address) => format!("listening on {address}"),
        None => "not listening".into(),
    })];
    children.extend(ruled(addresses));
    section("listen", "Listen on", children)
}

/// `2026-09-30T12:03:11Z` as `2026-09-30 12:03`.
fn when(stamp: &str) -> String {
    stamp
        .get(..16)
        .map_or_else(|| stamp.to_owned(), |minute| minute.replace('T', " "))
}

/// The phones paired, a ruled row each: its name, when it paired and
/// was last seen, and the way to revoke it.
fn paired(phones: &PhonesData) -> Node {
    let rows = phones.paired.iter().map(|phone| {
        let seen = phone.last_seen.as_deref().map_or_else(
            || "never connected".to_owned(),
            |seen| format!("last seen {}", when(seen)),
        );
        Node::row([
            Node::Rich {
                text: format!("**{}**", phone.name),
            },
            small(format!("paired {} · {seen}", when(&phone.paired_at))),
            Node::Spacer,
            button(
                "revoke",
                &Act::Revoke {
                    id: phone.id.clone(),
                },
                Kind::Danger,
            ),
        ])
    });
    let mut children = ruled(rows);
    if children.is_empty() {
        children.push(Node::Empty {
            text: "No phones yet.".into(),
        });
    }
    let title = match phones.paired.len() {
        0 => "Paired phones".to_owned(),
        count => format!("Paired phones · {count}"),
    };
    section("paired", title, children)
}

/// What pairs a phone: the way to show a code, or the code shown: the
/// QR code beside the code in big print, then where it reaches the
/// computer and the certificate, as ruled rows.
fn pair(phones: &PhonesData) -> Node {
    let Some(code) = &phones.code else {
        let mut children = Vec::new();
        if phones.listening.is_some() {
            children.push(button(
                "Show pairing code",
                &Act::ShowCode,
                Kind::Primary,
            ));
        }
        return section("pair", "Pair a phone", children);
    };
    let detail = |key: &str, value: &str| {
        Node::row([small(key), Node::Mono { text: value.into() }])
    };
    let children = vec![
        Node::row([
            small("expires"),
            Node::Due {
                id: "code-expires".into(),
                at: code.expires,
            },
            Node::Spacer,
            button("hide code", &Act::HideCode, Kind::Secondary),
        ]),
        Node::row([
            Node::Qr {
                text: code.text.clone(),
                size: 200.,
            },
            Node::stack([
                small("code"),
                Node::Figure {
                    text: code.secret.clone(),
                },
                Node::text("In heiko on your phone, tap Scan pairing code."),
            ]),
        ]),
        Node::Rule,
        detail("address", &code.address),
        Node::Rule,
        detail("certificate", &code.certificate),
        Node::Rule,
        // Typing the address instead: the phone shows the whole
        // certificate, to compare with this.
        Node::stack(code.lines.iter().map(|line| small(line.clone()))),
    ];
    section("pair", "Pair a phone", children)
}
