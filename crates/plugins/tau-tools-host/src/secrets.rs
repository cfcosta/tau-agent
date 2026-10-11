//! tau's secret store: keys an agent's HTTP requests name and never
//! see (ADR 0037). Each agent has its own file,
//! `~/.config/tau/secrets/<agent>.json` ([`SecretStore::of_agent`]),
//! holding `{ name: value }`, owner-only, replaced whole in one rename:
//! an agent's requests reach its own keys and no other agent's. The
//! person fills it from the agent's page; [`crate::http`] puts the values in as a request leaves,
//! and [`Scrub`] takes them out of everything that comes back.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use serde_json::Value;

/// The store's file in tau's configuration directory.
pub const SECRETS_FILE: &str = "secrets.json";

/// The directory, in tau's configuration directory, of each agent's
/// own store.
pub const AGENTS_DIR: &str = "secrets";

/// The shortest value [`Scrub`] takes out of text: shorter ones would
/// take out ordinary words.
pub const SCRUBBED_FROM: usize = 4;

/// One read-modify-write at a time in this process.
static WRITING: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Whether `name` can name a secret: letters, digits, `_`, `-` and `.`,
/// 1 to 64 of them.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The secrets file. Every call reads or writes it, so a value saved
/// from the page reaches the next request at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretStore {
    path: PathBuf,
}

impl SecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<dir>/secrets.json`.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join(SECRETS_FILE))
    }

    /// The agent `agent`'s own store in tau's configuration directory
    /// `dir`: `<dir>/secrets/<agent>.json`. An agent's name is a
    /// directory's name already, so it makes a file's.
    pub fn of_agent(dir: &Path, agent: &str) -> Self {
        Self::new(dir.join(AGENTS_DIR).join(format!("{agent}.json")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every secret, by name. No file is none.
    pub async fn read(&self) -> io::Result<BTreeMap<String, String>> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(error),
        };
        // The parser's message could quote the file: it never leaves.
        serde_json::from_str(&text).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a secrets file", self.path.display()),
            )
        })
    }

    /// The names it holds, in order.
    pub async fn names(&self) -> io::Result<Vec<String>> {
        Ok(self.read().await?.into_keys().collect())
    }

    /// Saves `value` as `name`, in place of any before.
    pub async fn set(&self, name: &str, value: &str) -> io::Result<()> {
        if !valid_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{name}` cannot name a secret"),
            ));
        }
        let _writing = WRITING.lock().await;
        let mut secrets = self.read().await?;
        secrets.insert(name.to_owned(), value.to_owned());
        self.write(&secrets).await
    }

    /// Forgets `name`. Whether it was there.
    pub async fn remove(&self, name: &str) -> io::Result<bool> {
        let _writing = WRITING.lock().await;
        let mut secrets = self.read().await?;
        let was = secrets.remove(name).is_some();
        if was {
            self.write(&secrets).await?;
        }
        Ok(was)
    }

    async fn write(
        &self,
        secrets: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let text = serde_json::to_string_pretty(secrets)
            .expect("names and values always print");
        tau_ai::files::write_private_async(&self.path, text.as_bytes()).await
    }
}

/// Takes every secret's value out of text, each as `[secret:<name>]`,
/// longest first so one value inside another goes whole.
#[derive(Debug, Clone, Default)]
pub struct Scrub {
    values: Vec<(String, String)>,
}

impl Scrub {
    /// Scrubs `secrets`' values of at least [`SCRUBBED_FROM`] bytes.
    pub fn new(secrets: &BTreeMap<String, String>) -> Self {
        let mut values: Vec<(String, String)> = secrets
            .iter()
            .filter(|(_, value)| value.len() >= SCRUBBED_FROM)
            .map(|(name, value)| (value.clone(), name.clone()))
            .collect();
        values.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.cmp(b)));
        Self { values }
    }

    /// `text` with no secret's value in it.
    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for (value, name) in &self.values {
            if text.contains(value.as_str()) {
                text =
                    text.replace(value.as_str(), &format!("[secret:{name}]"));
            }
        }
        text
    }

    /// `bytes` with no secret's value in them, text or not.
    pub fn bytes(&self, bytes: &[u8]) -> Vec<u8> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return self.text(text).into_bytes();
        }
        let mut bytes = bytes.to_vec();
        for (value, name) in &self.values {
            let value = value.as_bytes();
            let mark = format!("[secret:{name}]").into_bytes();
            let mut scrubbed = Vec::with_capacity(bytes.len());
            let mut at = 0;
            while at < bytes.len() {
                if bytes[at..].starts_with(value) {
                    scrubbed.extend_from_slice(&mark);
                    at += value.len();
                } else {
                    scrubbed.push(bytes[at]);
                    at += 1;
                }
            }
            bytes = scrubbed;
        }
        bytes
    }

    /// `value` with every string in it, keys included, scrubbed.
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(items) => Value::Array(
                items.iter().map(|item| self.value(item)).collect(),
            ),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (self.text(key), self.value(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}
