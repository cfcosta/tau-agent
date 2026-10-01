//! The host's certificate, and how a phone trusts it: by its SHA-256
//! fingerprint alone, never by a certificate authority.

use std::{
    fs,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};
use tokio_rustls::rustls::{
    self,
    ClientConfig,
    DigitallySignedStruct,
    ServerConfig,
    SignatureScheme,
    client::danger::{
        HandshakeSignatureValid,
        ServerCertVerified,
        ServerCertVerifier,
    },
    crypto::{CryptoProvider, ring},
    pki_types::{
        CertificateDer,
        PrivateKeyDer,
        PrivatePkcs8KeyDer,
        ServerName,
        UnixTime,
    },
};

use crate::pairing::Fingerprint;

const CERT: &str = "cert.der";
const KEY: &str = "key.der";

/// The name a phone asks for. The certificate carries it, but only the
/// fingerprint is checked.
pub(crate) const SERVER_NAME: &str = "tau";

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("could not read or write the certificate: {0}")]
    Io(#[from] io::Error),
    #[error("could not make a certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("the certificate does not work for TLS: {0}")]
    Rustls(#[from] rustls::Error),
}

/// The host's self-signed certificate and its key.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Identity {
    /// Reads the certificate kept in `dir`, or makes one, once, named
    /// after `host`.
    pub fn load_or_create(dir: &Path, host: &str) -> Result<Self, TlsError> {
        let (cert, key) = (dir.join(CERT), dir.join(KEY));
        if cert.exists() && key.exists() {
            return Ok(Self {
                cert: CertificateDer::from(fs::read(cert)?),
                key: PrivatePkcs8KeyDer::from(fs::read(key)?),
            });
        }
        fs::create_dir_all(dir)?;
        let pair = rcgen::KeyPair::generate()?;
        let mut params =
            rcgen::CertificateParams::new(vec![SERVER_NAME.into()])?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, host);
        let made = params.self_signed(&pair)?;
        let identity = Self {
            cert: made.der().clone(),
            key: PrivatePkcs8KeyDer::from(pair.serialize_der()),
        };
        tau_ai::files::write_private(&key, identity.key.secret_pkcs8_der())?;
        fs::write(cert, identity.cert.as_ref())?;
        Ok(identity)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint_of(&self.cert)
    }

    pub fn server_config(&self) -> Result<Arc<ServerConfig>, TlsError> {
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![self.cert.clone()],
                PrivateKeyDer::Pkcs8(self.key.clone_key()),
            )?;
        Ok(Arc::new(config))
    }
}

pub fn fingerprint_of(cert: &[u8]) -> Fingerprint {
    Fingerprint(Sha256::digest(cert).into())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// Trusts a server by its certificate's fingerprint: exactly the one
/// expected, or, when probing, whatever it is, to show a person.
#[derive(Debug)]
pub(crate) struct Pinned {
    expect: Option<Fingerprint>,
    /// The fingerprint the server showed, once it has.
    seen: Mutex<Option<Fingerprint>>,
    provider: Arc<CryptoProvider>,
}

impl Pinned {
    pub(crate) fn expecting(fingerprint: Fingerprint) -> Arc<Self> {
        Self::new(Some(fingerprint))
    }

    /// Accepts any certificate; for reading its fingerprint only.
    pub(crate) fn recording() -> Arc<Self> {
        Self::new(None)
    }

    fn new(expect: Option<Fingerprint>) -> Arc<Self> {
        Arc::new(Self {
            expect,
            seen: Mutex::new(None),
            provider: provider(),
        })
    }

    pub(crate) fn expected(&self) -> Option<Fingerprint> {
        self.expect
    }

    pub(crate) fn seen(&self) -> Option<Fingerprint> {
        *self.seen.lock().expect("not poisoned")
    }

    pub(crate) fn client_config(self: &Arc<Self>) -> Arc<ClientConfig> {
        let config = ClientConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(self.clone())
            .with_no_client_auth();
        Arc::new(config)
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let found = fingerprint_of(end_entity);
        *self.seen.lock().expect("not poisoned") = Some(found);
        match self.expect {
            Some(expected) if expected != found => {
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let made = Identity::load_or_create(dir.path(), "desk").unwrap();
        let read = Identity::load_or_create(dir.path(), "other").unwrap();
        assert_eq!(made.fingerprint(), read.fingerprint());
        assert!(made.server_config().is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(dir.path().join(KEY))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
