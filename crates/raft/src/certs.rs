use std::net::IpAddr;
use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};

/// Generated certificate bundle for a cluster node.
pub struct NodeCertBundle {
    pub ca_pem: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// Generate a self-signed CA certificate and key, returning both the PEM strings
/// and the rcgen objects for reuse in signing node certs.
pub fn generate_ca(org: &str) -> anyhow::Result<(String, String, rcgen::Certificate, KeyPair)> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::OrganizationName, org);
    params
        .distinguished_name
        .push(DnType::CommonName, format!("{org} CA"));
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let cert = params.self_signed(&key_pair)?;

    Ok((cert.pem(), key_pair.serialize_pem(), cert, key_pair))
}

/// Generate a node certificate signed by the CA.
pub fn generate_node_cert(
    ca_cert: &rcgen::Certificate,
    ca_key: &KeyPair,
    node_name: &str,
    addr: &str,
    ca_pem: &str,
) -> anyhow::Result<NodeCertBundle> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, node_name);
    params
        .distinguished_name
        .push(DnType::OrganizationName, "sparrow");

    let ip: IpAddr = addr.parse()?;
    params
        .subject_alt_names
        .push(rcgen::SanType::DnsName(node_name.try_into()?));
    params.subject_alt_names.push(rcgen::SanType::IpAddress(ip));

    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let node_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let cert = params.signed_by(&node_key, ca_cert, ca_key)?;

    Ok(NodeCertBundle {
        ca_pem: ca_pem.to_string(),
        cert_pem: cert.pem(),
        key_pem: node_key.serialize_pem(),
    })
}

/// Build a `rustls::ServerConfig` for mTLS (requires client certs signed by CA).
pub fn server_config_from_pem(
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    use rustls::server::WebPkiClientVerifier;
    use rustls::RootCertStore;

    let mut root_store = RootCertStore::empty();
    let ca_der = load_pem_cert(ca_pem)?;
    root_store.add(ca_der)?;

    let certs = vec![load_pem_cert(cert_pem)?];
    let key = load_pem_private_key(key_pem)?;

    let verifier = WebPkiClientVerifier::builder(Arc::new(root_store)).build()?;

    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?;

    Ok(Arc::new(config))
}

/// Build a `reqwest::Client` that uses mTLS for outbound Raft connections.
pub fn mtls_client_from_pem(
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
) -> anyhow::Result<reqwest::Client> {
    let ca = reqwest::tls::Certificate::from_pem(ca_pem.as_bytes())?;
    let identity = reqwest::tls::Identity::from_pem(format!("{cert_pem}\n{key_pem}").as_bytes())?;

    Ok(reqwest::Client::builder()
        .add_root_certificate(ca)
        .identity(identity)
        .build()?)
}

pub fn load_pem_cert(pem: &str) -> anyhow::Result<rustls_pki_types::CertificateDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let mut iter = rustls_pemfile::certs(&mut reader);
    match iter.next() {
        Some(Ok(cert)) => Ok(cert),
        Some(Err(e)) => anyhow::bail!("Failed to parse PEM cert: {e}"),
        None => anyhow::bail!("No certificate found in PEM"),
    }
}

pub fn load_pem_private_key(pem: &str) -> anyhow::Result<rustls_pki_types::PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    match rustls_pemfile::private_key(&mut reader)? {
        Some(key) => Ok(key),
        None => anyhow::bail!("No private key found in PEM"),
    }
}
