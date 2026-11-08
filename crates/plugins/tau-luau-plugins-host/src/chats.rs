//! What a Luau plugin asks of tau's chats (ADR 0048, the third seam):
//! to start one in a repository, or to steer one that exists, and from
//! a run's tool, the person's leave first.
//!
//! A hook never waits on a chat: it answers what it asks
//! (`tau.start_chat`, `tau.steer_chat`, `tau.allow`), and the host
//! carries it out once the hook ended and its store is written, then
//! calls the action it names next with what came of it. So the store a
//! hook holds is never held while a chat starts, whose own hooks may
//! want it.

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_plugin::Conversations;
use tokio::sync::oneshot;

/// The most times in a row an action may ask the chats again.
pub const MAX_FOLLOWS: usize = 4;

/// What a hook's answer asks of the chats.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Request {
    #[serde(flatten)]
    chat: Chat,
    /// The action called next, with `args` and what came of it.
    #[serde(default, rename = "then")]
    next: Option<String>,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Chat {
    /// A chat in `repo`, the hook's own when it names none, on the
    /// person's `text`.
    Start {
        #[serde(default)]
        repo: Option<String>,
        text: String,
    },
    /// The person's `text` said to the chat `run`.
    Steer { run: String, text: String },
}

/// What `value`, a hook's answer, asks of the chats: its `tau_chat`.
fn request(value: &Value) -> Option<Result<Request, String>> {
    let asked = value.get("tau_chat")?;
    Some(
        serde_json::from_value(asked.clone())
            .map_err(|error| format!("`tau_chat` is not a chat: {error}")),
    )
}

/// Carries out what `value`, the answer of a hook of the repository
/// `repo`, asks of `chats`, then calls the action it names next through
/// `then`, with its `args` and what came of it: `run` (and `repo` for a
/// start), or `error`. Answers the last answer that asks nothing more,
/// or what came of the last chat when no action is named next.
pub async fn follow<F, Fut>(
    mut value: Value,
    chats: Option<&dyn Conversations>,
    repo: Option<&str>,
    mut then: F,
) -> Result<Value, String>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    for _ in 0..MAX_FOLLOWS {
        let Some(asked) = request(&value) else {
            return Ok(value);
        };
        let asked = asked?;
        let chats = chats.ok_or("this host starts and steers no chats")?;
        let came = match asked.chat {
            Chat::Start { repo: named, text } => {
                let repo = named
                    .or_else(|| repo.map(str::to_owned))
                    .ok_or("`tau.start_chat` needs the repository")?;
                match chats.start(&repo, text).await {
                    Ok(run) => json!({ "run": run, "repo": repo }),
                    Err(error) => {
                        json!({ "repo": repo, "error": format!("{error:#}") })
                    }
                }
            }
            Chat::Steer { run, text } => match chats.steer(&run, text).await {
                Ok(()) => json!({ "run": run }),
                Err(error) => {
                    json!({ "run": run, "error": format!("{error:#}") })
                }
            },
        };
        let Some(next) = asked.next else {
            return match came.get("error").and_then(Value::as_str) {
                Some(error) => Err(error.to_owned()),
                None => Ok(came),
            };
        };
        let mut args = match asked.args {
            Value::Object(args) => args,
            // An empty Luau table may come as either.
            Value::Null => serde_json::Map::new(),
            Value::Array(args) if args.is_empty() => serde_json::Map::new(),
            other => return Err(format!("`args` is not a table: {other}")),
        };
        if let Value::Object(came) = came {
            args.extend(came);
        }
        value = then(next, Value::Object(args)).await?;
    }
    Err(format!(
        "it asked the chats more than {MAX_FOLLOWS} times in a row"
    ))
}

/// What a run's tool asks the person to allow: `tau.allow`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// What the card says it would do: "Start a chat on td-4e1b".
    pub title: String,
    /// More of it, in Markdown.
    #[serde(default)]
    pub text: String,
    /// The plugin's action carried out once allowed, with `args`, as the
    /// person's own.
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// What `value`, a tool's answer, asks the person to allow.
pub fn ask(value: &Value) -> Option<Result<Ask, String>> {
    let asked = value.get("tau_allow")?;
    Some(
        serde_json::from_value(asked.clone())
            .map_err(|error| format!("`tau_allow` is not an ask: {error}")),
    )
}

/// The person's answer to an ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub allowed: bool,
    /// Why not, when they said.
    #[serde(default)]
    pub reason: String,
}

/// The asks waiting for the person, by run and call. The first answer
/// takes one, from whichever device; the rest find it gone. Clones share
/// them.
pub type Allows = Waits<Answer>;

/// What a run's calls wait for the person on, by run and call, each
/// with `F`, what it asked; its answer is an `A`. The first answer takes
/// one, from whichever device; the rest find it gone. Clones share them.
pub struct Waits<A, F = ()>(Arc<Mutex<Pending<A, F>>>);

/// What each waiting call asked, and where its answer goes, by run and
/// call.
type Pending<A, F> = BTreeMap<(String, String), (F, oneshot::Sender<A>)>;

impl<A, F> Clone for Waits<A, F> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A, F> Default for Waits<A, F> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<A, F> std::fmt::Debug for Waits<A, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Waits").field(&self.lock().len()).finish()
    }
}

/// One call waiting: dropped, as when its run ends, it waits no more.
pub struct Waiting<A, F = ()> {
    waits: Waits<A, F>,
    key: (String, String),
    answer: Option<oneshot::Receiver<A>>,
}

impl<A, F> Waiting<A, F> {
    /// The person's answer; `None` once no one can answer, as tau closes.
    pub async fn answer(mut self) -> Option<A> {
        self.answer.take()?.await.ok()
    }
}

impl<A, F> Drop for Waiting<A, F> {
    fn drop(&mut self) {
        self.waits.lock().remove(&self.key);
    }
}

/// Why an answer found nothing to take.
const TAKEN: &str = "That was answered already, or its run ended before";

impl<A, F> Waits<A, F> {
    /// The call `call` of `run` waits for the person, having asked
    /// `asked`.
    pub fn wait(&self, run: &str, call: &str, asked: F) -> Waiting<A, F> {
        let (answer, receive) = oneshot::channel();
        let key = (run.to_owned(), call.to_owned());
        self.lock().insert(key.clone(), (asked, answer));
        Waiting {
            waits: self.clone(),
            key,
            answer: Some(receive),
        }
    }

    /// Gives the call `call` of `run` the person's answer, when it still
    /// waits for one and `fits` what it asked; one that does not fit
    /// leaves it waiting.
    pub fn answer_if(
        &self,
        run: &str,
        call: &str,
        answer: A,
        fits: impl FnOnce(&F) -> Result<(), String>,
    ) -> anyhow::Result<()> {
        let mut waiting = self.lock();
        let key = (run.to_owned(), call.to_owned());
        let (asked, _) =
            waiting.get(&key).ok_or_else(|| anyhow::anyhow!(TAKEN))?;
        fits(asked).map_err(anyhow::Error::msg)?;
        let (_, sender) = waiting.remove(&key).expect("found above");
        sender.send(answer).map_err(|_| anyhow::anyhow!(TAKEN))
    }

    /// Gives the call `call` of `run` the person's answer, when it still
    /// waits for one.
    pub fn answer(
        &self,
        run: &str,
        call: &str,
        answer: A,
    ) -> anyhow::Result<()> {
        self.answer_if(run, call, answer, |_| Ok(()))
    }

    /// The calls waiting, by run and call.
    pub fn waiting(&self) -> Vec<(String, String)> {
        self.lock().keys().cloned().collect()
    }

    /// Whether the call `call` of `run` waits.
    pub fn waits(&self, run: &str, call: &str) -> bool {
        self.lock().contains_key(&(run.to_owned(), call.to_owned()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending<A, F>> {
        self.0.lock().expect("not poisoned")
    }
}
