//! What crosses the socket: JSON text frames, each an object tagged by
//! `type`.
//!
//! A phone opens with [`Hello`]; the host answers with an [`Answer`].
//! From then on the host sends [`Down`] frames and the phone [`Up`]
//! ones. The bodies are the app's own JSON; this crate does not look
//! into them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol's version. A host refuses a phone that speaks another.
pub const VERSION: u32 = 2;

/// A phone's first frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Hello {
    /// Trade a one-time pairing secret for a device token.
    Pair {
        version: u32,
        secret: String,
        name: String,
    },
    /// Connect with a device token; `last_seq` is the last message the
    /// phone had, to be sent what it missed.
    Resume {
        version: u32,
        token: String,
        last_seq: Option<u64>,
    },
}

impl Hello {
    pub fn version(&self) -> u32 {
        match self {
            Self::Pair { version, .. } | Self::Resume { version, .. } => {
                *version
            }
        }
    }
}

/// The host's answer to a [`Hello`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Welcome {
        version: u32,
        /// The computer's name.
        host: String,
        /// The phone's device id.
        device: String,
        /// The device token, only in answer to a pairing.
        token: Option<String>,
        /// Whether the messages the phone missed follow, rather than a
        /// snapshot.
        resumed: bool,
    },
    Refused {
        reason: Refusal,
    },
}

/// Why a host turned a phone away.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refusal {
    #[error("tau on the computer speaks protocol version {speaks}")]
    Version { speaks: u32 },
    #[error("the pairing code is wrong, used or expired")]
    BadSecret,
    #[error("this phone is not paired, or was revoked")]
    UnknownToken,
}

/// Host to phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Down {
    /// Everything the phone needs, as of message `seq`.
    Snapshot {
        seq: u64,
        bodies: Vec<Value>,
    },
    Message {
        seq: u64,
        body: Value,
    },
    /// The host took every request of this phone's up to `up`
    /// ([`crate::outbox`]). Not numbered: it is for this phone alone.
    Ack {
        up: u64,
    },
}

impl Down {
    /// The number of the last message this brings the phone to; none
    /// for an answer to the phone's own request.
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Snapshot { seq, .. } | Self::Message { seq, .. } => {
                Some(*seq)
            }
            Self::Ack { .. } => None,
        }
    }
}

/// Phone to host: a request, numbered by the phone's
/// [`Outbox`](crate::outbox::Outbox).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    Up { id: u64, body: Value },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn frames_are_tagged_objects() {
        let hello = Hello::Resume {
            version: VERSION,
            token: "t".into(),
            last_seq: Some(3),
        };
        assert_eq!(
            serde_json::to_value(&hello).unwrap(),
            json!({"type": "resume", "version": 2, "token": "t", "last_seq": 3})
        );
        let refused = Answer::Refused {
            reason: Refusal::Version { speaks: 1 },
        };
        let text = serde_json::to_string(&refused).unwrap();
        assert_eq!(serde_json::from_str::<Answer>(&text).unwrap(), refused);
        let down = Down::Message {
            seq: 9,
            body: json!({"a": 1}),
        };
        assert_eq!(down.seq(), Some(9));
        // An answer to the phone's request has no number in the feed.
        assert_eq!(Down::Ack { up: 4 }.seq(), None);
        let up = Up::Up {
            id: 4,
            body: json!("go"),
        };
        assert_eq!(
            serde_json::to_value(&up).unwrap(),
            json!({"type": "up", "id": 4, "body": "go"})
        );
    }
}
