use anyhow::{bail, Context, Result};
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, Error, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::Arc};

#[derive(Serialize, Deserialize)]
pub struct Identity {
    pub cert: Vec<u8>,
    key: Vec<u8>,
}

pub fn device_id(cert: &[u8]) -> Result<String> {
    let (remaining, cert) = x509_parser::parse_x509_certificate(cert)
        .map_err(|_| anyhow::anyhow!("Invalid device certificate"))?;
    if !remaining.is_empty()
        || cert.public_key().algorithm.algorithm.to_id_string() != "1.3.101.112"
    {
        bail!("An Ed25519 device certificate is required");
    }
    Ok(
        blake3::hash(cert.public_key().subject_public_key.data.as_ref())
            .to_hex()
            .to_string(),
    )
}

impl Identity {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("identity.json");
        if path.exists() {
            let identity: Self = serde_json::from_slice(&fs::read(path)?)
                .context("Cannot read identity; keep a backup before resetting")?;
            device_id(&identity.cert)?;
            return Ok(identity);
        }
        let key = KeyPair::generate_for(&PKCS_ED25519)?;
        let cert = CertificateParams::new(vec!["localmesh.local".into()])?.self_signed(&key)?;
        let identity = Self {
            cert: cert.der().to_vec(),
            key: key.serialize_der(),
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(path)?;
        file.write_all(&serde_json::to_vec(&identity)?)?;
        file.sync_all()?;
        Ok(identity)
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.key.clone()).into()
    }

    pub fn configs(&self) -> Result<(quinn::ServerConfig, quinn::ClientConfig)> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // Certificates prove key possession. Trust is decided at the application
        // boundary after the TLS-exporter pairing code is confirmed on BOTH devices.
        let verifier = Arc::new(DeviceVerifier);
        let mut server = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier.clone())
            .with_single_cert(vec![CertificateDer::from(self.cert.clone())], self.key())?;
        server.alpn_protocols = vec![b"localmesh/1".to_vec()];
        let mut client = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(vec![CertificateDer::from(self.cert.clone())], self.key())?;
        client.alpn_protocols = vec![b"localmesh/1".to_vec()];
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server)?,
        ));
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(16u32.into());
        transport.max_concurrent_uni_streams(0u32.into());
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
        transport.max_idle_timeout(Some(std::time::Duration::from_secs(30).try_into()?));
        let transport = Arc::new(transport);
        server.transport_config(transport.clone());
        let mut client = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client)?,
        ));
        client.transport_config(transport);
        Ok((server, client))
    }
}

#[derive(Debug)]
struct DeviceVerifier;

fn check_cert(cert: &CertificateDer<'_>) -> std::result::Result<(), Error> {
    device_id(cert.as_ref())
        .map(|_| ())
        .map_err(|_| Error::InvalidCertificate(rustls::CertificateError::BadEncoding))
}

fn signature(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> std::result::Result<HandshakeSignatureValid, Error> {
    rustls::crypto::verify_tls13_signature(
        message,
        cert,
        dss,
        &rustls::crypto::ring::default_provider().signature_verification_algorithms,
    )
}

impl ServerCertVerifier for DeviceVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        check_cert(cert)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        signature(msg, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

impl ClientCertVerifier for DeviceVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        check_cert(cert)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        signature(msg, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}
