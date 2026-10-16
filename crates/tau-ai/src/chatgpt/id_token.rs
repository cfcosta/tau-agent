//! Validating the ID token: its signature against OpenAI's JWKS, then its
//! issuer, audience, expiry and nonce. Only asymmetric algorithms are
//! accepted (RS256/384/512, ES256); `none` and HMAC never are.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde::Deserialize;
use serde_json::Value;

use super::IdTokenError;

/// A published signing key, as a JWKS lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default, rename = "use")]
    pub usage: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

/// A JSON Web Key Set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

/// A JWT split into its parts, not yet trusted.
#[derive(Debug, Clone)]
pub struct Jwt {
    alg: String,
    kid: Option<String>,
    signed: String,
    signature: Vec<u8>,
    pub claims: Value,
}

impl Jwt {
    pub fn parse(token: &str) -> Result<Self, IdTokenError> {
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(IdTokenError::Malformed);
        };
        let decode = |part: &str| {
            URL_SAFE_NO_PAD
                .decode(part)
                .map_err(|_| IdTokenError::Malformed)
        };
        let head: Header = serde_json::from_slice(&decode(header)?)
            .map_err(|_| IdTokenError::Malformed)?;
        let claims: Value = serde_json::from_slice(&decode(payload)?)
            .map_err(|_| IdTokenError::Malformed)?;
        if !claims.is_object() {
            return Err(IdTokenError::Malformed);
        }
        Ok(Self {
            alg: head.alg,
            kid: head.kid,
            signed: format!("{header}.{payload}"),
            signature: decode(signature)?,
            claims,
        })
    }

    /// The key `kid` names, or the only key usable for the algorithm.
    pub fn key<'a>(&self, jwks: &'a Jwks) -> Option<&'a Jwk> {
        let usable = |key: &&Jwk| {
            key.usage.as_deref().is_none_or(|usage| usage == "sig")
                && key.alg.as_deref().is_none_or(|alg| alg == self.alg)
        };
        match &self.kid {
            Some(kid) => jwks
                .keys
                .iter()
                .filter(usable)
                .find(|key| key.kid.as_deref() == Some(kid)),
            None => {
                let mut keys = jwks.keys.iter().filter(usable);
                let only = keys.next();
                keys.next().is_none().then_some(only).flatten()
            }
        }
    }

    pub fn has_kid(&self) -> bool {
        self.kid.is_some()
    }

    /// Checks the signature with `key`.
    pub fn verify(&self, key: &Jwk) -> Result<(), IdTokenError> {
        let decode = |field: &Option<String>| {
            field
                .as_deref()
                .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
                .ok_or(IdTokenError::UnknownKey)
        };
        let message = self.signed.as_bytes();
        let ok = match (self.alg.as_str(), key.kty.as_str()) {
            (alg @ ("RS256" | "RS384" | "RS512"), "RSA") => {
                let params = match alg {
                    "RS256" => &signature::RSA_PKCS1_2048_8192_SHA256,
                    "RS384" => &signature::RSA_PKCS1_2048_8192_SHA384,
                    _ => &signature::RSA_PKCS1_2048_8192_SHA512,
                };
                RsaPublicKeyComponents {
                    n: decode(&key.n)?,
                    e: decode(&key.e)?,
                }
                .verify(params, message, &self.signature)
                .is_ok()
            }
            ("ES256", "EC") if key.crv.as_deref() == Some("P-256") => {
                let mut point = vec![0x04];
                point.extend(decode(&key.x)?);
                point.extend(decode(&key.y)?);
                UnparsedPublicKey::new(
                    &signature::ECDSA_P256_SHA256_FIXED,
                    point,
                )
                .verify(message, &self.signature)
                .is_ok()
            }
            // A known algorithm, but not this key's type.
            ("RS256" | "RS384" | "RS512" | "ES256", _) => {
                return Err(IdTokenError::UnknownKey);
            }
            (alg, _) => return Err(IdTokenError::Algorithm(alg.to_owned())),
        };
        if ok {
            Ok(())
        } else {
            Err(IdTokenError::Signature)
        }
    }

    /// Whether tau accepts the algorithm at all.
    pub fn check_algorithm(&self) -> Result<(), IdTokenError> {
        match self.alg.as_str() {
            "RS256" | "RS384" | "RS512" | "ES256" => Ok(()),
            other => Err(IdTokenError::Algorithm(other.to_owned())),
        }
    }
}

/// What an ID token must say.
#[derive(Debug, Clone)]
pub struct Expected<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    /// `None` on a refresh, which carries no nonce.
    pub nonce: Option<&'a str>,
    /// Unix seconds.
    pub now: u64,
}

/// How far a clock may be off.
pub const LEEWAY_SECS: u64 = 60;

/// The validated identity in an ID token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
}

/// Checks the claims of a token whose signature already verified.
pub fn check_claims(
    claims: &Value,
    expected: &Expected<'_>,
) -> Result<Identity, IdTokenError> {
    let text = |name: &str| claims.get(name).and_then(Value::as_str);
    if text("iss") != Some(expected.issuer) {
        return Err(IdTokenError::Issuer(text("iss").map(str::to_owned)));
    }
    let audience_ok = match claims.get("aud") {
        Some(Value::String(aud)) => aud == expected.client_id,
        Some(Value::Array(auds)) => {
            auds.iter()
                .any(|aud| aud.as_str() == Some(expected.client_id))
                && (auds.len() == 1 || text("azp") == Some(expected.client_id))
        }
        _ => false,
    };
    if !audience_ok {
        return Err(IdTokenError::Audience);
    }
    match claims.get("exp").and_then(Value::as_u64) {
        // An expiry so far ahead that the leeway passes `u64::MAX` is
        // later than any clock.
        Some(exp)
            if exp
                .checked_add(LEEWAY_SECS)
                .is_none_or(|end| end > expected.now) => {}
        _ => return Err(IdTokenError::Expired),
    }
    if let Some(nonce) = expected.nonce
        && text("nonce") != Some(nonce)
    {
        return Err(IdTokenError::Nonce);
    }
    let subject = text("sub")
        .filter(|sub| !sub.is_empty())
        .ok_or(IdTokenError::Subject)?;
    Ok(Identity {
        subject: subject.to_owned(),
        email: text("email").map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expected() -> Expected<'static> {
        Expected {
            issuer: "https://auth.openai.com",
            client_id: "oaiapp_1",
            nonce: Some("n1"),
            now: 1000,
        }
    }

    fn claims() -> Value {
        json!({
            "iss": "https://auth.openai.com",
            "aud": "oaiapp_1",
            "exp": 2000,
            "nonce": "n1",
            "sub": "user-1",
            "email": "a@example.com",
        })
    }

    #[test]
    fn good_claims_give_the_identity() {
        assert_eq!(
            check_claims(&claims(), &expected()),
            Ok(Identity {
                subject: "user-1".into(),
                email: Some("a@example.com".into()),
            })
        );
    }

    /// How a drawn claim stands against what the token must say.
    #[derive(Debug, Clone)]
    enum Aud {
        /// The client id as a string.
        Ours,
        /// Another client's id.
        Theirs,
        /// A list holding only the client id.
        OnlyOurs,
        /// A list with the client id and another, and the `azp` drawn.
        Several(Option<&'static str>),
        /// A list without the client id.
        NotOurs,
        Missing,
    }

    /// The claims are checked in a fixed order, each against what the
    /// token must say, and the first that fails is named; a token that
    /// passes them all gives its subject and email. The written-out
    /// expectation reads `exp` in 128 bits, so a clock leeway past
    /// `u64::MAX` cannot wrap. Expiries are drawn on either side of the
    /// leeway's edge, and near `u64::MAX`, clocks near zero and near the
    /// top.
    #[hegel::test(test_cases = 500)]
    fn claims_are_checked_in_order_against_the_expected_token(
        tc: hegel::TestCase,
    ) {
        use hegel::generators::{self as gs, Generator as _};
        let now: u64 = tc.draw(hegel::one_of!(
            gs::integers::<u64>().max_value(100_000),
            gs::sampled_from(vec![0, 59, 60, 61, u64::MAX]),
            gs::integers::<u64>().min_value(u64::MAX - 120),
        ));
        let nonce = tc.draw(gs::optional(gs::just("n1")));
        let expected = Expected {
            issuer: "https://auth.openai.com",
            client_id: "oaiapp_1",
            nonce,
            now,
        };
        let mut claims = serde_json::Map::new();

        // Each claim is right most of the time, so the later checks are
        // reached; when it is wrong, it is wrong in any of a few ways.
        let right = || gs::weighted_booleans(0.8);
        let iss = if tc.draw(right()) {
            Some("https://auth.openai.com")
        } else {
            tc.draw(gs::optional(gs::sampled_from(vec![
                "https://auth.openai.com/",
                "https://evil",
            ])))
        };
        if let Some(iss) = iss {
            claims.insert("iss".into(), json!(iss));
        }
        let aud = if tc.draw(right()) {
            tc.draw(
                gs::sampled_from(vec![
                    Aud::Ours,
                    Aud::OnlyOurs,
                    Aud::Several(Some("oaiapp_1")),
                ])
                .print_as_debug(),
            )
        } else {
            tc.draw(
                gs::sampled_from(vec![
                    Aud::Theirs,
                    Aud::Several(None),
                    Aud::Several(Some("other")),
                    Aud::NotOurs,
                    Aud::Missing,
                ])
                .print_as_debug(),
            )
        };
        match &aud {
            Aud::Ours => claims.insert("aud".into(), json!("oaiapp_1")),
            Aud::Theirs => claims.insert("aud".into(), json!("other")),
            Aud::OnlyOurs => claims.insert("aud".into(), json!(["oaiapp_1"])),
            Aud::Several(azp) => {
                if let Some(azp) = azp {
                    claims.insert("azp".into(), json!(azp));
                }
                claims.insert("aud".into(), json!(["other", "oaiapp_1"]))
            }
            Aud::NotOurs => claims.insert("aud".into(), json!(["other"])),
            Aud::Missing => None,
        };
        // On either side of where the leeway runs out, far ahead, near the
        // top of the range, or not a whole number at all.
        let edge = now.saturating_sub(LEEWAY_SECS);
        let exp: Option<Value> = tc.draw(gs::optional(hegel::one_of!(
            gs::integers::<i64>()
                .min_value(-2)
                .max_value(2)
                .map(move |step| json!(edge.saturating_add_signed(step))),
            gs::integers::<u64>().map(|n| json!(n)),
            gs::integers::<u64>()
                .min_value(u64::MAX - 120)
                .map(|n| json!(n)),
            gs::just(json!(u64::MAX)),
            gs::just(json!(-1)),
            gs::just(json!(1.5)),
            gs::just(json!("2000")),
        )));
        if let Some(exp) = &exp {
            claims.insert("exp".into(), exp.clone());
        }
        let token_nonce = if tc.draw(right()) {
            Some("n1")
        } else {
            tc.draw(gs::optional(gs::just("n2")))
        };
        if let Some(token_nonce) = token_nonce {
            claims.insert("nonce".into(), json!(token_nonce));
        }
        let sub = if tc.draw(right()) {
            Some("user-1")
        } else {
            tc.draw(gs::optional(gs::just("")))
        };
        if let Some(sub) = sub {
            claims.insert("sub".into(), json!(sub));
        }
        let email = tc.draw(gs::optional(gs::just("a@example.com")));
        if let Some(email) = email {
            claims.insert("email".into(), json!(email));
        }

        let audience_ok = match aud {
            Aud::Ours | Aud::OnlyOurs => true,
            // Several audiences need `azp` to name this client.
            Aud::Several(azp) => azp == Some("oaiapp_1"),
            Aud::Theirs | Aud::NotOurs | Aud::Missing => false,
        };
        let unexpired =
            exp.as_ref().and_then(Value::as_u64).is_some_and(|exp| {
                u128::from(exp) + u128::from(LEEWAY_SECS) > u128::from(now)
            });
        let want = if iss != Some("https://auth.openai.com") {
            Err(IdTokenError::Issuer(iss.map(str::to_owned)))
        } else if !audience_ok {
            Err(IdTokenError::Audience)
        } else if !unexpired {
            Err(IdTokenError::Expired)
        } else if nonce.is_some() && token_nonce != nonce {
            Err(IdTokenError::Nonce)
        } else if sub.is_none_or(str::is_empty) {
            Err(IdTokenError::Subject)
        } else {
            Ok(Identity {
                subject: "user-1".into(),
                email: email.map(str::to_owned),
            })
        };
        assert_eq!(
            check_claims(&Value::Object(claims.clone()), &expected),
            want,
            "{claims:?} at {now}"
        );
    }

    /// Regression: `exp + LEEWAY_SECS` wrapped for an expiry within a
    /// minute of `u64::MAX`, and the token read as expired at any clock
    /// (shrunk by `claims_are_checked_in_order_against_the_expected_token`
    /// to `exp = u64::MAX - 59` at `now = 0`). Such an expiry is later than
    /// every clock, the last one included.
    #[test]
    fn an_expiry_at_the_end_of_time_is_later_than_any_clock() {
        for exp in [u64::MAX - 59, u64::MAX] {
            for now in [0, u64::MAX - 1, u64::MAX] {
                let mut claims = claims();
                claims["exp"] = json!(exp);
                let expected = Expected { now, ..expected() };
                assert!(
                    check_claims(&claims, &expected).is_ok(),
                    "{exp} at {now}"
                );
            }
        }
    }

    #[test]
    fn unsigned_tokens_are_refused() {
        let encode = |value: Value| URL_SAFE_NO_PAD.encode(value.to_string());
        let token =
            format!("{}.{}.", encode(json!({"alg": "none"})), encode(claims()));
        let jwt = Jwt::parse(&token).unwrap();
        assert_eq!(
            jwt.check_algorithm(),
            Err(IdTokenError::Algorithm("none".into()))
        );
        assert!(Jwt::parse("a.b").is_err());
    }
}
