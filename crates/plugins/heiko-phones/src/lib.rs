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
    tree::{self, Kind, MarkState, Node, SquareLook, Tone},
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
        wide: false,
        detail: None,
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

/// A section's head, which does not fold: its title, small print at
/// its end, and what the person can do after that.
fn header(title: &str, meta: Option<String>, actions: Vec<Node>) -> Node {
    Node::Header {
        title: title.into(),
        meta,
        actions,
    }
}

/// An empty line: the room between two sections.
fn gap() -> Node {
    Node::text("\u{a0}")
}

/// Small print in mono.
fn small(text: impl Into<String>) -> Node {
    Node::Toned {
        text: text.into(),
        tone: Tone::Neutral,
        mono: true,
    }
}

/// One of the two segments of "Allow phones": outlined when it is the
/// one that holds, else a click away.
fn segment(label: &str, allow: bool, allowed: bool) -> Node {
    let chosen = allow == allowed;
    Node::Square {
        label: label.into(),
        style: if chosen {
            SquareLook::Outlined
        } else {
            SquareLook::Quiet
        },
        tone: if chosen {
            tree::Ink::Info
        } else {
            tree::Ink::Neutral
        },
        action: (!chosen).then(|| ACT.to_owned()),
        args: if chosen {
            Value::Null
        } else {
            json!(Act::Allow { allow })
        },
    }
}

/// The Phones page: "Allow phones", where heiko listens and the phones
/// paired, and beside them what pairs one; on a phone, under them.
fn page(view: &mut ViewCx<'_, PhonesUi>) -> Node {
    tree(view.data, view.compact)
}

/// The page's tree; `compact` for a narrow window, one column.
fn tree(phones: &PhonesData, compact: bool) -> Node {
    let mut main = vec![
        header(
            "Phones",
            Some("on your network, or through your VPN".into()),
            vec![],
        ),
        Node::row([
            Node::stack([
                Node::Rich {
                    text: "**Allow phones**".into(),
                },
                Node::Toned {
                    text: "Only while this app is open.".into(),
                    tone: Tone::Neutral,
                    mono: false,
                },
            ]),
            Node::Spacer,
            segment("off", false, phones.allowed),
            segment("on", true, phones.allowed),
        ]),
    ];
    if let Some(error) = &phones.error {
        main.push(Node::Toned {
            text: error.clone(),
            tone: Tone::Bad,
            mono: false,
        });
    }
    main.push(Node::Rule);
    if phones.allowed {
        main.push(gap());
        main.extend(listen_on(phones));
    }
    main.push(gap());
    main.extend(paired(phones));
    if !phones.allowed {
        return Node::stack(main);
    }
    let pair = pair(phones);
    if compact {
        main.push(gap());
        main.extend(pair);
        return Node::stack(main);
    }
    Node::Table {
        head: vec![],
        rows: vec![vec![Node::stack(main), Node::stack(pair)]],
        widths: vec![1.4, 1.],
    }
}

/// Where heiko listens, and a ruled row for each address it could: the
/// one it listens on ticked, the others a click away.
fn listen_on(phones: &PhonesData) -> Vec<Node> {
    let chosen = phones
        .listening
        .as_deref()
        .and_then(|address| address.rsplit_once(':'))
        .map(|(host, _)| host);
    let addresses = phones.addresses.iter().map(|address| {
        let on = chosen == Some(address.ip.as_str());
        vec![
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
            ]),
            Node::Mono {
                text: address.ip.clone(),
            },
        ]
    });
    vec![
        header(
            "Listen on",
            Some(match &phones.listening {
                Some(address) => format!("listening on {address}"),
                None => "not listening".into(),
            }),
            vec![],
        ),
        Node::Table {
            head: vec![],
            rows: addresses.collect(),
            widths: vec![1., 2.6],
        },
    ]
}

/// `2026-09-30T12:03:11Z` as `2026-09-30 12:03`.
fn when(stamp: &str) -> String {
    stamp
        .get(..16)
        .map_or_else(|| stamp.to_owned(), |minute| minute.replace('T', " "))
}

/// The phones paired, a ruled row each: its name, when it paired and
/// was last seen, and the way to revoke it.
fn paired(phones: &PhonesData) -> Vec<Node> {
    let rows = phones.paired.iter().map(|phone| {
        let seen = phone.last_seen.as_deref().map_or_else(
            || "never connected".to_owned(),
            |seen| format!("last seen {}", when(seen)),
        );
        Node::row([
            Node::Rich {
                text: format!("**{}**", phone.name),
            },
            Node::Spacer,
            small(format!("paired {} · {seen}", when(&phone.paired_at))),
            button(
                "revoke",
                &Act::Revoke {
                    id: phone.id.clone(),
                },
                Kind::Danger,
            ),
        ])
    });
    let mut children = vec![header(
        "Paired phones",
        (!phones.paired.is_empty()).then(|| phones.paired.len().to_string()),
        vec![],
    )];
    children.extend(ruled(rows));
    if phones.paired.is_empty() {
        children.push(Node::Empty {
            text: "No phones yet.".into(),
        });
    } else {
        children.push(Node::Rule);
    }
    children
}

/// How wide the pairing QR code is, in pixels: it fits its half of the
/// column beside the code.
const QR: f32 = 180.;

/// How many groups of the certificate's fingerprint stand on a line.
const CERTIFICATE_GROUPS: usize = 3;

/// What pairs a phone: the way to show a code, or the code shown: the
/// QR code beside the code in big print, then where it reaches the
/// computer and the certificate, as ruled rows.
fn pair(phones: &PhonesData) -> Vec<Node> {
    let Some(code) = &phones.code else {
        let mut children = vec![header("Pair a phone", None, vec![])];
        if phones.listening.is_some() {
            children.push(Node::row([button(
                "Show pairing code",
                &Act::ShowCode,
                Kind::Primary,
            )]));
        }
        return children;
    };
    vec![
        header(
            "Pair a phone",
            None,
            vec![
                small("expires"),
                Node::Due {
                    id: "code-expires".into(),
                    at: code.expires,
                },
                button("hide code", &Act::HideCode, Kind::Secondary),
            ],
        ),
        // Side by side, ruled under: the code and what to do with it
        // wrap in their half, beside the QR code.
        Node::Table {
            head: vec![],
            rows: vec![vec![
                Node::Qr {
                    text: code.text.clone(),
                    size: QR,
                    offset: true,
                },
                Node::stack([
                    small("code"),
                    Node::Figure {
                        text: code.secret.clone(),
                    },
                    Node::text(
                        "In heiko on your phone, tap Scan pairing code.",
                    ),
                ]),
            ]],
            widths: vec![1., 1.1],
        },
        Node::Table {
            head: vec![],
            rows: vec![
                vec![
                    small("address"),
                    Node::Mono {
                        text: code.address.clone(),
                    },
                ],
                vec![
                    small("certificate"),
                    Node::Mono {
                        text: code.certificate.clone(),
                    },
                ],
                // Typing the address instead: the phone shows the whole
                // certificate, to compare with this.
                vec![
                    Node::Stack { children: vec![] },
                    Node::stack(
                        code.lines
                            .chunks(CERTIFICATE_GROUPS)
                            .map(|groups| small(groups.join(" "))),
                    ),
                ],
            ],
            widths: vec![1., 2.7],
        },
    ]
}
