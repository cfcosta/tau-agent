//! The saved sign-in: `github.json` in tau's config directory, readable
//! only by the user.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A saved sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub token: String,
    /// The login it belongs to.
    pub user: String,
    /// When it stops working, in Unix milliseconds; `None` for tokens
    /// that do not expire.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl Token {
    /// Where the sign-in is kept in tau's config directory `dir`.
    pub fn file(dir: &Path) -> PathBuf {
        dir.join("github.json")
    }

    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at.is_some_and(|at| now_ms >= at)
    }

    /// The sign-in saved in `dir`, unless it expired.
    pub async fn load(dir: &Path) -> Option<Self> {
        let text = tokio::fs::read_to_string(Self::file(dir)).await.ok()?;
        let token: Self = serde_json::from_str(&text).ok()?;
        (!token.is_expired(now_ms())).then_some(token)
    }

    /// Saves the sign-in in `dir`.
    pub async fn save(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_vec_pretty(self)?;
        tau_ai::files::write_private_async(&Self::file(dir), &text).await
    }

    /// Forgets the sign-in saved in `dir`, if there is one.
    pub async fn forget(dir: &Path) -> std::io::Result<()> {
        match tokio::fs::remove_file(Self::file(dir)).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error)
            }
            _ => Ok(()),
        }
    }
}

/// Now, in Unix milliseconds.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sign-in with an expiry is expired from that millisecond on, not
    /// a millisecond before; one without never expires. Times are drawn
    /// at the expiry, either side of it, and anywhere.
    #[hegel::test(test_cases = 300)]
    fn a_sign_in_expires_at_its_time(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let expires_at: Option<u64> = tc.draw(gs::optional(
            gs::integers::<u64>().min_value(1).max_value(u64::MAX - 1),
        ));
        let around = expires_at.unwrap_or(1_700_000_000_000);
        let now: u64 = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![around - 1, around, around + 1]),
            gs::integers::<u64>(),
        ));
        let token = Token {
            token: "ghu_x".into(),
            user: "octocat".into(),
            expires_at,
        };
        let expired = match expires_at {
            Some(at) => now >= at,
            None => false,
        };
        assert_eq!(token.is_expired(now), expired, "{expires_at:?} at {now}");
    }

    #[test]
    fn expired_sign_ins_are_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let token = Token {
            token: "ghu_x".into(),
            user: "octocat".into(),
            expires_at: Some(now_ms() + 60_000),
        };
        tau_testing::block_on_io(token.save(dir)).unwrap();
        assert_eq!(
            tau_testing::block_on_io(Token::load(dir)),
            Some(token.clone())
        );
        let expired = Token {
            expires_at: Some(1),
            ..token
        };
        tau_testing::block_on_io(expired.save(dir)).unwrap();
        assert_eq!(tau_testing::block_on_io(Token::load(dir)), None);
        tau_testing::block_on_io(Token::forget(dir)).unwrap();
        tau_testing::block_on_io(Token::forget(dir)).unwrap();
        assert!(!Token::file(dir).exists());
    }
}
