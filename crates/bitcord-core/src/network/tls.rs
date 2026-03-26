//! TLS configuration for QUIC transport.
//!
//! The node derives a self-signed TLS certificate deterministically from its
//! Ed25519 signing key using `rcgen`. Clients pin the certificate's SHA-256
//! fingerprint (included in the invite link) so they know they are connecting
//! to the correct node without a CA.
//!
//! # Security model
//! - Transport security: QUIC + TLS 1.3 (provided by Quinn/rustls).
//! - Node identity binding: the TLS cert is derived from the node's Ed25519
//!   key, so the fingerprint is stable across restarts and tied to the node's
//!   cryptographic identity.
//! - Client verification: `FingerprintVerifier` — skips CA chain and hostname
//!   validation, only checks SHA-256(DER cert). Appropriate because the
//!   fingerprint is distributed out-of-band via the invite link.

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use pkcs8::EncodePrivateKey as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

/// ALPN protocol label for BitCord QUIC connections.
pub const ALPN_BITCORD: &[u8] = b"bitcord/1";

// ── Node TLS certificate ──────────────────────────────────────────────────────

/// A self-signed TLS certificate derived from the node's Ed25519 signing key.
///
/// The certificate is deterministic: the same `SigningKey` always produces the
/// same DER bytes and therefore the same fingerprint.
pub struct NodeTlsCert {
    /// DER-encoded X.509 certificate bytes.
    pub cert_der: Vec<u8>,
    /// DER-encoded PKCS#8 private key bytes.
    pub key_der: Vec<u8>,
    /// SHA-256 of `cert_der`. Embedded in invite links for client-side pinning.
    pub fingerprint: [u8; 32],
}

impl NodeTlsCert {
    /// Generate a self-signed TLS certificate from the node's Ed25519 signing key.
    ///
    /// The certificate carries a single DNS SAN `"bitcord-node"` and uses
    /// Ed25519 as the signature algorithm. Because the key is deterministic,
    /// the fingerprint is stable across node restarts.
    pub fn generate(signing_key: &SigningKey) -> Result<Self> {
        // Export the ed25519-dalek key as PKCS#8 DER so rcgen can import it.
        let pkcs8_doc = signing_key
            .to_pkcs8_der()
            .context("encode Ed25519 signing key as PKCS#8 DER")?;
        let key_der_bytes = pkcs8_doc.as_bytes().to_vec();

        // Import into rcgen and build a self-signed certificate.
        let key_pair = rcgen::KeyPair::try_from(key_der_bytes.as_slice())
            .context("create rcgen KeyPair from PKCS#8 DER")?;

        let cert_params = rcgen::CertificateParams::new(vec!["bitcord-node".to_string()])
            .context("create certificate parameters")?;

        let cert = cert_params
            .self_signed(&key_pair)
            .context("self-sign TLS certificate")?;

        let cert_der_bytes: Vec<u8> = cert.der().as_ref().to_vec();
        let fingerprint: [u8; 32] = Sha256::digest(&cert_der_bytes).into();

        Ok(Self {
            cert_der: cert_der_bytes,
            key_der: key_der_bytes,
            fingerprint,
        })
    }

    /// Build a `quinn::ServerConfig` backed by this certificate.
    pub fn server_config(&self) -> Result<quinn::ServerConfig> {
        let cert = CertificateDer::from(self.cert_der.clone());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()));

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("set TLS 1.3 version")?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .context("build rustls ServerConfig")?;

        tls.alpn_protocols = vec![ALPN_BITCORD.to_vec()];

        let quinn_cfg = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .context("build Quinn ServerConfig")?;

        let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(quinn_cfg));
        cfg.transport_config(Arc::new(safe_transport_config()));
        Ok(cfg)
    }
}

/// Build a `TransportConfig` with a safe initial MTU (1200 bytes, the QUIC
/// minimum) and MTU discovery disabled.  This avoids `sendmsg` errors on
/// Windows where the OS rejects datagrams larger than the path MTU.
fn safe_transport_config() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport.initial_mtu(1200);
    transport.mtu_discovery_config(None);
    transport
}

/// Client transport config: safe MTU settings + keepalive to prevent the
/// server from closing idle QUIC connections while the push reader is waiting.
fn client_transport_config() -> quinn::TransportConfig {
    let mut transport = safe_transport_config();
    // Send a QUIC PING every 20 s so the connection stays alive even when
    // there are no incoming push streams.  Without this the server's idle
    // timeout (~30 s) fires, accept_uni() returns "timed out", and the push
    // reader tears down the connection unnecessarily.
    transport.keep_alive_interval(Some(Duration::from_secs(20)));
    // Give the connection 5 minutes of true silence before we consider it dead.
    transport.max_idle_timeout(Some(
        Duration::from_secs(300)
            .try_into()
            .expect("valid idle timeout"),
    ));
    transport
}

// ── Fingerprint-pinning client verifier ───────────────────────────────────────

/// A TLS server-certificate verifier that only checks a SHA-256 fingerprint.
///
/// CA chain and hostname validation are intentionally bypassed: the node's
/// fingerprint, distributed via the invite link, is the sole trust anchor.
/// The TLS handshake still proves the server owns the private key matching
/// the certificate (the `verify_tls13_signature` call).
#[derive(Debug)]
pub struct FingerprintVerifier {
    expected: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl FingerprintVerifier {
    pub fn new(expected: [u8; 32]) -> Self {
        Self::new_with_provider(expected, Arc::new(rustls::crypto::ring::default_provider()))
    }

    pub fn new_with_provider(
        expected: [u8; 32],
        provider: Arc<rustls::crypto::CryptoProvider>,
    ) -> Self {
        Self { expected, provider }
    }
}

impl rustls::client::danger::ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // All-zeros fingerprint means TOFU mode — accept any certificate.
        // Used only for global bootstrap seeds and DHT/mailbox lookups where
        // the remote node's fingerprint is genuinely unknowable in advance.
        if self.expected == [0u8; 32] {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if got == self.expected {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "TLS certificate fingerprint mismatch".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // QUIC only uses TLS 1.3; this path is unreachable in practice.
        Err(rustls::Error::General("TLS 1.2 not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Build a `quinn::ClientConfig` that pins the given certificate fingerprint.
///
/// Hostname validation is disabled; the fingerprint is the sole trust anchor.
pub fn client_config_pinned(fingerprint: [u8; 32]) -> Result<quinn::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(FingerprintVerifier::new_with_provider(
        fingerprint,
        Arc::clone(&provider),
    ));

    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("set TLS 1.3 version")?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    tls.alpn_protocols = vec![ALPN_BITCORD.to_vec()];

    let quinn_cfg = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .context("build Quinn ClientConfig")?;

    let mut cfg = quinn::ClientConfig::new(Arc::new(quinn_cfg));
    cfg.transport_config(Arc::new(client_transport_config()));
    Ok(cfg)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use rustls::client::danger::ServerCertVerifier;

    #[test]
    fn cert_is_deterministic() {
        let sk = SigningKey::generate(&mut OsRng);
        let a = NodeTlsCert::generate(&sk).unwrap();
        let b = NodeTlsCert::generate(&sk).unwrap();
        assert_eq!(a.cert_der, b.cert_der);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn different_keys_give_different_fingerprints() {
        let sk_a = SigningKey::generate(&mut OsRng);
        let sk_b = SigningKey::generate(&mut OsRng);
        let a = NodeTlsCert::generate(&sk_a).unwrap();
        let b = NodeTlsCert::generate(&sk_b).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn fingerprint_matches_sha256_of_cert_der() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        let computed: [u8; 32] = Sha256::digest(&cert.cert_der).into();
        assert_eq!(cert.fingerprint, computed);
    }

    #[test]
    fn server_config_builds_successfully() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        assert!(cert.server_config().is_ok());
    }

    #[test]
    fn client_config_pinned_builds_successfully() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        assert!(client_config_pinned(cert.fingerprint).is_ok());
    }

    #[test]
    fn verifier_rejects_mismatched_fingerprint() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        let wrong_fp = [0xABu8; 32];
        let verifier = FingerprintVerifier::new(wrong_fp);
        let cert_der = CertificateDer::from(cert.cert_der);
        let result = verifier.verify_server_cert(
            &cert_der,
            &[],
            &rustls::pki_types::ServerName::try_from("bitcord-node").unwrap(),
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(result.is_err(), "should reject mismatched fingerprint");
    }

    #[test]
    fn verifier_accepts_correct_fingerprint() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        let verifier = FingerprintVerifier::new(cert.fingerprint);
        let cert_der = CertificateDer::from(cert.cert_der.clone());
        let result = verifier.verify_server_cert(
            &cert_der,
            &[],
            &rustls::pki_types::ServerName::try_from("bitcord-node").unwrap(),
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(result.is_ok(), "should accept correct fingerprint");
    }

    #[test]
    fn verifier_accepts_tofu_all_zeros() {
        let sk = SigningKey::generate(&mut OsRng);
        let cert = NodeTlsCert::generate(&sk).unwrap();
        let verifier = FingerprintVerifier::new([0u8; 32]);
        let cert_der = CertificateDer::from(cert.cert_der);
        let result = verifier.verify_server_cert(
            &cert_der,
            &[],
            &rustls::pki_types::ServerName::try_from("bitcord-node").unwrap(),
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(
            result.is_ok(),
            "all-zeros fingerprint (TOFU) should accept any certificate"
        );
    }
}
