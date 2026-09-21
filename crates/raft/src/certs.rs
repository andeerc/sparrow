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

/// Mint a join token embedding everything a new node needs to join:
/// node id, CA cert PEM, and CA key PEM (base64, dot-separated).
/// Format: `SPARJOIN.<node-id>.<ca-cert-b64url>.<ca-key-b64url>`.
/// The leader prints it via `cluster join-token`; the joiner parses it,
/// mints its own node cert locally, and registers — no manual ca.pem copy.
/// SECURITY: the CA key signs arbitrary node certs — transmit over a secure
/// channel and rotate the CA if a token leaks.
pub fn mint_join_token(node_id: u64, ca_pem: &str, ca_key_pem: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    format!(
        "SPARJOIN.{node_id}.{}.{}",
        URL_SAFE_NO_PAD.encode(ca_pem.as_bytes()),
        URL_SAFE_NO_PAD.encode(ca_key_pem.as_bytes())
    )
}

/// Parse a token from []. Errors name the broken segment —
/// never silently mint against the wrong CA.
pub fn parse_join_token(token: &str) -> anyhow::Result<(u64, String, String)> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let rest = token
        .strip_prefix("SPARJOIN.")
        .ok_or_else(|| anyhow::anyhow!("join token must start with SPARJOIN."))?;
    let mut parts = rest.splitn(3, '.');
    let (id_s, ca_b64, key_b64) = match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => anyhow::bail!("join token needs 3 dot-separated parts: SPARJOIN.<node-id>.<ca>.<key>"),
    };
    let node_id: u64 = id_s
        .parse()
        .map_err(|_| anyhow::anyhow!("join token node id '{id_s}' is not a number"))?;
    if node_id == 0 {
        anyhow::bail!("join token node id must be non-zero (1 is the founder)");
    }
    let ca_pem = String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(ca_b64)
            .map_err(|e| anyhow::anyhow!("join token CA cert is not valid base64: {e}"))?,
    )
    .map_err(|_| anyhow::anyhow!("join token CA cert is not valid UTF-8"))?;
    let ca_key_pem = String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(key_b64)
            .map_err(|e| anyhow::anyhow!("join token CA key is not valid base64: {e}"))?,
    )
    .map_err(|_| anyhow::anyhow!("join token CA key is not valid UTF-8"))?;
    if !ca_pem.contains("BEGIN CERTIFICATE") {
        anyhow::bail!("join token CA cert is not a PEM certificate");
    }
    Ok((node_id, ca_pem, ca_key_pem))
}

/// Mint a node cert from serialized CA PEMs (cert + key), without needing the
/// original rcgen objects. Used by joiners holding a SPARJOIN token: reloads
/// the CA signing key via `KeyPair::from_pem` and re-creates an equivalent CA
/// `Certificate` object by self-signing the SAME key with the SAME subject
/// params parsed from the CA PEM (CN/Organization). The issued node cert is
/// then signed by that key — rustls verifies the chain by signature bytes
/// against the CA cert, so it validates against the real leader CA.
/// Returns the bundle (ca_pem echoed back for storage on the joiner).
pub fn mint_node_cert_from_ca_pem(
    ca_pem: &str,
    ca_key_pem: &str,
    node_name: &str,
    addr: &str,
) -> anyhow::Result<(NodeCertBundle, usize)> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    // Reload the CA signing key.
    let ca_key = KeyPair::from_pem(ca_key_pem)
        .map_err(|e| anyhow::anyhow!("join token CA key invalid: {e}"))?;
    // Parse the CA PEM to recover subject params (CN/Org) for the shadow CA.
    let ca_der =
        pem::parse(ca_pem).map_err(|e| anyhow::anyhow!("join token CA cert invalid PEM: {e}"))?;
    let (_rem, x509) = {
        use x509_parser::prelude::FromDer;
        x509_parser::certificate::X509Certificate::from_der(ca_der.contents())
            .map_err(|e| anyhow::anyhow!("join token CA cert invalid DER: {e}"))?
    };
    let subject_attrs: Vec<(String, String)> = x509
        .subject()
        .iter_common_name()
        .map(|a| ("CN".to_string(), a.as_str().unwrap_or("").to_string()))
        .chain(
            x509.subject()
                .iter_organization()
                .map(|a| ("O".to_string(), a.as_str().unwrap_or("").to_string())),
        )
        .collect();
    // Rebuild equivalent CA params and self-sign with the SAME key to get a
    // signer object whose signature verifies against the real CA cert.
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = DistinguishedName::new();
    for (kind, val) in &subject_attrs {
        if kind == "CN" && !val.is_empty() {
            ca_params
                .distinguished_name
                .push(DnType::CommonName, val.clone());
        } else if kind == "O" && !val.is_empty() {
            ca_params
                .distinguished_name
                .push(DnType::OrganizationName, val.clone());
        }
    }
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let shadow_ca = ca_params
        .self_signed(&ca_key)
        .map_err(|e| anyhow::anyhow!("failed to reload CA signer: {e}"))?;
    // Now mint the node cert exactly like generate_node_cert, but against the shadow CA.
    let bundle = generate_node_cert(&shadow_ca, &ca_key, node_name, addr, ca_pem)?;
    let fp_len = B64.encode(shadow_ca.der()).len();
    Ok((bundle, fp_len))
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

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn roundtrip_token() {
        let (ca_pem, ca_key, _, _) = generate_ca("test").unwrap();
        let tok = mint_join_token(3, &ca_pem, &ca_key);
        assert!(tok.starts_with("SPARJOIN.3."));
        let (id, ca2, key2) = parse_join_token(&tok).unwrap();
        assert_eq!(id, 3);
        assert_eq!(ca2, ca_pem);
        assert_eq!(key2, ca_key);
    }

    #[test]
    fn reject_bad_tokens() {
        assert!(parse_join_token("garbage").is_err());
        assert!(parse_join_token("SPARJOIN.abc.eG9.eyJ9").is_err());
        assert!(parse_join_token("SPARJOIN.1.eG9.eyJ9").is_err());
        assert!(parse_join_token("SPARJOIN.0.eG9.eyJ9").is_err());
    }
}
