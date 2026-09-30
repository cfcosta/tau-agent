//! Where sign-ins live: one protected record per registration (issued
//! client id and account), the active account, and this host's id.
//!
//! ```text
//! $XDG_CONFIG_HOME/tau/chatgpt/          0700
//!   host.json                            {"ext_agent_host_id": "urn:uuid:…"}
//!   active                               the active account's id
//!   accounts/<account id>.json           one credential record, 0600
//!   accounts/<account id>.lock           held while refreshing or signing out
//! ```
//!
//! Every file is written whole to a temporary file and renamed into
//! place, so a reader never sees half of one. Signing out clears a
//! record's tokens but keeps the record: its client id is reused on the
//! next sign-in. The host id is never replaced.

use std::{
    fmt,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ChatGptError, PLAN_USAGE_SCOPE, random_bytes};

/// This host's `ext_agent_host_id`: opaque, made once, never
/// user-identifying, and not a credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(String);

impl HostId {
    /// A fresh `urn:uuid:` id from a random UUIDv4.
    pub fn new_uuid() -> Self {
        let mut bytes: [u8; 16] = random_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Self(format!(
            "urn:uuid:{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        ))
    }

    /// An id in one of the forms OpenAI accepts: `urn:uuid:`, a JWK
    /// thumbprint URI, or `did:key:`.
    pub fn parse(text: &str) -> Option<Self> {
        let accepted = [
            "urn:uuid:",
            "urn:ietf:params:oauth:jwk-thumbprint:",
            "did:key:",
        ];
        accepted
            .iter()
            .any(|prefix| text.len() > prefix.len() && text.starts_with(prefix))
            .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A saved registration's stable id: its file name. Made from the issued
/// client id and the validated subject, so two registrations with one
/// email stay apart.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(client_id: &str, subject: &str) -> Self {
        let client: String = client_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .take(64)
            .collect();
        let digest = Sha256::digest(subject.as_bytes());
        let subject: String =
            digest[..6].iter().map(|b| format!("{b:02x}")).collect();
        Self(format!("{client}-{subject}"))
    }

    /// An id as the user or a file gave it; `None` if it could not name a
    /// file in the store.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a sign-in may use the ChatGPT plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanUsage {
    /// `chatgpt.tokens.use.direct` was granted.
    Enabled,
    /// Signed in without it: declined, or not allowed. Inference must not
    /// start; offer to enable it or another way to pay.
    Disabled,
}

/// What an account picker shows for a saved registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    /// Tokens are saved.
    SignedIn(PlanUsage),
    /// Signed out, or the refresh token died: the registration is kept
    /// for the next sign-in.
    SignedOut,
}

/// One registration's record, in the fields OpenAI's sign-in guide
/// shows, plus what tau needs to resume: when the access token expires
/// and a label for the picker.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Distinct among saved accounts; shown in the picker.
    pub label: String,
    pub email: Option<String>,
    pub issuer: String,
    /// The validated ID token's `sub`.
    pub subject: String,
    /// The issued client id (`oaiapp_…`), never `dynamic_agent_client`.
    pub client_id: String,
    /// The host id this record was issued on.
    pub ext_agent_host_id: HostId,
    /// Kept for `id_token_hint`; cleared on sign-out.
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_in: Option<u64>,
    /// Unix seconds when the access token expires.
    pub expires_at: Option<u64>,
    /// The token response's `earliest_refresh_at`, as it came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_refresh_at: Option<serde_json::Value>,
    /// Granted scopes, sorted.
    pub scopes: Vec<String>,
    /// RFC 3339 UTC time the last token response arrived.
    pub saved_at: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print tokens.
        f.debug_struct("Credentials")
            .field("label", &self.label)
            .field("email", &self.email)
            .field("subject", &self.subject)
            .field("client_id", &self.client_id)
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("signed_in", &self.has_tokens())
            .finish_non_exhaustive()
    }
}

/// Refresh this long before the access token expires.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

impl Credentials {
    pub fn id(&self) -> AccountId {
        AccountId::new(&self.client_id, &self.subject)
    }

    pub fn plan_usage(&self) -> PlanUsage {
        if self.scopes.iter().any(|scope| scope == PLAN_USAGE_SCOPE) {
            PlanUsage::Enabled
        } else {
            PlanUsage::Disabled
        }
    }

    pub fn has_tokens(&self) -> bool {
        self.access_token.is_some() || self.refresh_token.is_some()
    }

    pub fn status(&self) -> AccountStatus {
        if self.has_tokens() {
            AccountStatus::SignedIn(self.plan_usage())
        } else {
            AccountStatus::SignedOut
        }
    }

    /// Whether to refresh at `now` (Unix seconds): the access token is
    /// missing or expired, or it expires within [`REFRESH_MARGIN`] and
    /// the token response's `earliest_refresh_at` has passed. OpenAI asks
    /// not to refresh before that time unless the token has expired.
    pub fn needs_refresh(&self, now: u64) -> bool {
        let Some(at) = self.expires_at.filter(|_| self.access_token.is_some())
        else {
            return true;
        };
        if now >= at {
            return true;
        }
        now + REFRESH_MARGIN.as_secs() >= at
            && self
                .earliest_refresh()
                .is_none_or(|earliest| now >= earliest)
    }

    /// `earliest_refresh_at` as Unix seconds, if it came as a number (or
    /// a string of one).
    pub fn earliest_refresh(&self) -> Option<u64> {
        match self.earliest_refresh_at.as_ref()? {
            serde_json::Value::Number(number) => number.as_u64(),
            serde_json::Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// Forgets the access and refresh tokens, which no longer work. The
    /// ID token stays, as the hint for the next sign-in.
    pub fn clear_session(&mut self) {
        self.access_token = None;
        self.refresh_token = None;
        self.expires_at = None;
    }

    /// Forgets every token, for sign-out.
    pub fn clear_tokens(&mut self) {
        self.clear_session();
        self.id_token = None;
    }
}

/// The saved accounts, the active one and this host's id.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct HostFile {
    ext_agent_host_id: HostId,
}

impl Store {
    /// `$XDG_CONFIG_HOME/tau/chatgpt`, falling back to
    /// `~/.config/tau/chatgpt`.
    pub fn default_dir() -> Result<PathBuf, ChatGptError> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .ok_or(ChatGptError::NoConfigDir)?;
        Ok(config.join("tau").join("chatgpt"))
    }

    /// The store in `dir`, created (owner only) if missing.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, ChatGptError> {
        let dir = dir.into();
        create_private_dir(&dir.join("accounts"))
            .and_then(|()| create_private_dir(&dir))
            .map_err(|error| storage(&dir, error))?;
        Ok(Self { dir })
    }

    /// The store at [`Self::default_dir`].
    pub fn open_default() -> Result<Self, ChatGptError> {
        Self::open(Self::default_dir()?)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This host's id, made and saved the first time it is asked for.
    /// Racing processes agree on one: the first to link its file wins.
    pub fn host_id(&self) -> Result<HostId, ChatGptError> {
        let path = self.dir.join("host.json");
        match self.read_host(&path) {
            Ok(id) => return Ok(id),
            Err(ChatGptError::Storage { error, .. })
                if error.kind() == io::ErrorKind::NotFound => {}
            Err(other) => return Err(other),
        }
        let text = serde_json::to_string_pretty(&HostFile {
            ext_agent_host_id: HostId::new_uuid(),
        })
        .expect("a host file serializes");
        let temp = write_temp(&path, text.as_bytes())
            .map_err(|error| storage(&path, error))?;
        // `hard_link` fails if the file exists: never replace a host id.
        let linked = fs::hard_link(&temp, &path);
        let _ = fs::remove_file(&temp);
        match linked {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(storage(&path, error)),
        }
        self.read_host(&path)
    }

    fn read_host(&self, path: &Path) -> Result<HostId, ChatGptError> {
        let text =
            fs::read_to_string(path).map_err(|error| storage(path, error))?;
        let file: HostFile = serde_json::from_str(&text)
            .map_err(|error| corrupt(path, error.to_string()))?;
        HostId::parse(file.ext_agent_host_id.as_str())
            .ok_or_else(|| corrupt(path, "not an accepted host id".into()))
    }

    fn account_path(&self, id: &AccountId) -> PathBuf {
        self.dir.join("accounts").join(format!("{id}.json"))
    }

    pub fn load(&self, id: &AccountId) -> Result<Credentials, ChatGptError> {
        let path = self.account_path(id);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ChatGptError::UnknownAccount(id.to_string()));
            }
            Err(error) => return Err(storage(&path, error)),
        };
        serde_json::from_str(&text)
            .map_err(|error| corrupt(&path, error.to_string()))
    }

    /// Saves `credentials` under their id, replacing the old record whole.
    pub fn save(&self, credentials: &Credentials) -> Result<(), ChatGptError> {
        let path = self.account_path(&credentials.id());
        let text = serde_json::to_string_pretty(credentials)
            .expect("credentials serialize");
        write_atomic(&path, text.as_bytes())
            .map_err(|error| storage(&path, error))
    }

    /// Every saved registration, by label.
    pub fn accounts(&self) -> Result<Vec<Credentials>, ChatGptError> {
        let dir = self.dir.join("accounts");
        let entries =
            fs::read_dir(&dir).map_err(|error| storage(&dir, error))?;
        let mut accounts = Vec::new();
        for entry in entries {
            let path = entry.map_err(|error| storage(&dir, error))?.path();
            let id = path
                .extension()
                .is_some_and(|ext| ext == "json")
                .then(|| path.file_stem()?.to_str().and_then(AccountId::parse))
                .flatten();
            if let Some(id) = id {
                accounts.push(self.load(&id)?);
            }
        }
        accounts.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(accounts)
    }

    pub fn active(&self) -> Result<Option<AccountId>, ChatGptError> {
        let path = self.dir.join("active");
        match fs::read_to_string(&path) {
            Ok(text) => Ok(AccountId::parse(&text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage(&path, error)),
        }
    }

    pub fn set_active(&self, id: &AccountId) -> Result<(), ChatGptError> {
        self.load(id)?;
        let path = self.dir.join("active");
        write_atomic(&path, id.as_str().as_bytes())
            .map_err(|error| storage(&path, error))
    }

    /// A label for a new record: its email, made distinct from the other
    /// accounts' labels with the client id's tail.
    pub(crate) fn label_for(
        &self,
        email: Option<&str>,
        client_id: &str,
        subject: &str,
    ) -> Result<String, ChatGptError> {
        let id = AccountId::new(client_id, subject);
        if let Ok(existing) = self.load(&id) {
            return Ok(existing.label);
        }
        let base = email.unwrap_or(subject).to_owned();
        let taken = self.accounts()?;
        if !taken.iter().any(|account| account.label == base) {
            return Ok(base);
        }
        let tail: String = client_id
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(format!("{base} ({tail})"))
    }

    /// Holds the account's lock file until the guard drops. Threads and
    /// processes that refresh or sign out the same account take turns.
    pub(crate) async fn lock(
        &self,
        id: &AccountId,
    ) -> Result<AccountLock, ChatGptError> {
        let path = self.dir.join("accounts").join(format!("{id}.lock"));
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| storage(&path, error))?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(AccountLock { _file: file }),
                Err(fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(fs::TryLockError::Error(error)) => {
                    return Err(storage(&path, error));
                }
            }
        }
    }
}

/// An account's lock file, locked; dropping it unlocks.
#[derive(Debug)]
pub(crate) struct AccountLock {
    _file: fs::File,
}

fn storage(path: &Path, error: io::Error) -> ChatGptError {
    ChatGptError::Storage {
        path: path.to_owned(),
        error,
    }
}

fn corrupt(path: &Path, message: String) -> ChatGptError {
    ChatGptError::Corrupt {
        path: path.to_owned(),
        message,
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Writes `bytes` to a new owner-only file beside `path`.
fn write_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let suffix: String = random_bytes::<6>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let temp = path.with_file_name(format!(".{name}.{suffix}.tmp"));
    let mut file =
        private_options().write(true).create_new(true).open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(temp)
}

/// Replaces `path` with `bytes` in one rename, owner-only.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = write_temp(path, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// `seconds` since the epoch as RFC 3339 UTC, e.g. `2026-09-29T12:00:00Z`.
pub fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_ids_are_uuid_v4_urns() {
        let id = HostId::new_uuid();
        let text = id.as_str();
        assert!(text.starts_with("urn:uuid:"));
        let uuid = &text["urn:uuid:".len()..];
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4", "version 4");
        assert!("89ab".contains(&uuid[19..20]), "RFC 4122 variant");
        assert_eq!(HostId::parse(text), Some(id.clone()));
        assert_ne!(HostId::new_uuid(), id);
        assert_eq!(HostId::parse("me@example.com"), None);
        assert_eq!(HostId::parse("urn:uuid:"), None);
    }

    #[test]
    fn timestamps_are_rfc_3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_790_032_532), "2026-09-21T23:15:32Z");
    }

    fn credentials(
        expires_at: u64,
        earliest: Option<serde_json::Value>,
    ) -> Credentials {
        Credentials {
            label: "a".into(),
            email: None,
            issuer: "i".into(),
            subject: "s".into(),
            client_id: "c".into(),
            ext_agent_host_id: HostId::new_uuid(),
            id_token: None,
            access_token: Some("at".into()),
            refresh_token: Some("rt".into()),
            token_type: None,
            expires_in: None,
            expires_at: Some(expires_at),
            earliest_refresh_at: earliest,
            scopes: Vec::new(),
            saved_at: String::new(),
        }
    }

    #[test]
    fn refreshes_wait_for_the_earliest_refresh_time() {
        let margin = REFRESH_MARGIN.as_secs();
        let plain = credentials(1000, None);
        assert!(!plain.needs_refresh(1000 - margin - 1));
        assert!(plain.needs_refresh(1000 - margin));
        // Inside the margin, but OpenAI asked to wait until 990.
        let held = credentials(1000, Some(serde_json::json!(990)));
        assert!(!held.needs_refresh(1000 - margin));
        assert!(held.needs_refresh(990));
        // An expired token refreshes whatever the earliest time says.
        let late = credentials(1000, Some(serde_json::json!("5000")));
        assert_eq!(late.earliest_refresh(), Some(5000));
        assert!(!late.needs_refresh(999));
        assert!(late.needs_refresh(1000));
        let mut none = credentials(1000, None);
        none.access_token = None;
        assert!(none.needs_refresh(0));
    }

    #[test]
    fn account_ids_name_files() {
        let id = AccountId::new("oaiapp_abc", "google-oauth2|123");
        assert!(id.as_str().starts_with("oaiapp_abc-"));
        assert_eq!(AccountId::parse(id.as_str()), Some(id.clone()));
        assert_ne!(id, AccountId::new("oaiapp_abc", "google-oauth2|124"));
        assert_ne!(id, AccountId::new("oaiapp_abd", "google-oauth2|123"));
        assert_eq!(AccountId::parse("../etc"), None);
    }
}
