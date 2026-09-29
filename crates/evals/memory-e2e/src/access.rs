//! How the evaluation reaches a model: an OpenAI API key or a ChatGPT
//! (Codex) sign-in, found the way tau-ui keeps them, and the model each
//! defaults to there.

use std::path::{Path, PathBuf};

/// How runs reach the model.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    /// A ChatGPT sign-in, from this credentials file.
    Codex(PathBuf),
    /// An OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex(path) => f.debug_tuple("Codex").field(path).finish(),
            Self::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

impl Access {
    /// The model tau-ui runs on this access when none is chosen.
    pub fn default_model(&self) -> &'static str {
        match self {
            Self::Codex(_) => "gpt-6-sol",
            Self::ApiKey(_) => "gpt-5.5",
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Codex(path) => {
                format!("ChatGPT sign-in ({})", path.display())
            }
            Self::ApiKey(_) => "OpenAI API key".into(),
        }
    }
}

/// The access to use, first found of:
///
/// 1. `codex`, a sign-in file named on the command line;
/// 2. `env_key`, the `OPENAI_API_KEY` environment variable, when set;
/// 3. tau's saved ChatGPT sign-in, `codex.json` in `config`
///    (`$XDG_CONFIG_HOME/tau`);
/// 4. tau's saved API key, `openai-key` in `config`.
pub fn resolve(
    codex: Option<PathBuf>,
    env_key: Option<String>,
    config: Option<&Path>,
) -> Option<Access> {
    if let Some(path) = codex {
        return Some(Access::Codex(path));
    }
    if let Some(key) = env_key.map(|key| key.trim().to_owned())
        && !key.is_empty()
    {
        return Some(Access::ApiKey(key));
    }
    let config = config?;
    let signed_in = config.join("codex.json");
    if signed_in.is_file() {
        return Some(Access::Codex(signed_in));
    }
    std::fs::read_to_string(config.join("openai-key"))
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .map(Access::ApiKey)
}

/// tau's config directory, where tau-ui keeps what the user signed in
/// with.
pub fn config_dir() -> Option<PathBuf> {
    tau_ai::codex::CodexCredentials::default_path()
        .and_then(|path| path.parent().map(Path::to_owned))
}
