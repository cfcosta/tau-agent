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
        Some(exp) if exp + LEEWAY_SECS > expected.now => {}
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

    #[test]
    fn each_bad_claim_is_named() {
        let with = |name: &str, value: Value| {
            let mut claims = claims();
            claims[name] = value;
            check_claims(&claims, &expected())
        };
        assert!(matches!(
            with("iss", json!("https://evil")),
            Err(IdTokenError::Issuer(_))
        ));
        assert_eq!(with("aud", json!("other")), Err(IdTokenError::Audience));
        assert_eq!(
            with("aud", json!(["oaiapp_1", "x"])),
            Err(IdTokenError::Audience),
            "several audiences need azp"
        );
        assert!(with("aud", json!(["oaiapp_1"])).is_ok());
        assert_eq!(with("exp", json!(900)), Err(IdTokenError::Expired));
        assert!(with("exp", json!(1000)).is_ok(), "within the leeway");
        assert_eq!(with("nonce", json!("n2")), Err(IdTokenError::Nonce));
        assert_eq!(with("sub", json!("")), Err(IdTokenError::Subject));
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
