use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

const NONCE_SIZE: usize = 12;
const SALT_SIZE: usize = 16;
/// Argon2id cost: m=19 MiB, t=2, p=1 (OWASP minimum for password storage).
const ARGON_M_COST: u32 = 19_456;
const ARGON_T_COST: u32 = 2;

/// v2 envelope: `v2:<base64 salt>:<base64 nonce||ciphertext>`.
/// Bare base64 without prefix is the legacy v1 format (SHA-256 KDF),
/// still accepted by [`decrypt`] for secrets written before v0.9.7.
fn derive_key_argon2(seed: &str, salt: &[u8]) -> Key<Aes256Gcm> {
    let params = Params::new(ARGON_M_COST, ARGON_T_COST, 1, Some(32))
        .expect("hardcoded Argon2 params are valid");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(seed.as_bytes(), salt, &mut out)
        .expect("Argon2 with fixed output length cannot fail");
    *Key::<Aes256Gcm>::from_slice(&out)
}

/// Legacy v1 KDF: raw SHA-256 of the seed. No salt, no work factor —
/// kept read-only for secrets written before the Argon2 migration.
fn derive_key_legacy(seed: &str) -> Key<Aes256Gcm> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    *Key::<Aes256Gcm>::from_slice(&hasher.finalize())
}

fn encrypt_with_key(plaintext: &str, key: &Key<Aes256Gcm>) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key);
    let nonce_vec: Vec<u8> = (0..NONCE_SIZE).map(|_| rand::random::<u8>()).collect();
    let nonce = Nonce::from_slice(&nonce_vec);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .unwrap_or_default();
    let mut combined = nonce_vec;
    combined.extend_from_slice(&ciphertext);
    combined
}

fn decrypt_with_key(payload: &[u8], key: &Key<Aes256Gcm>) -> Option<String> {
    if payload.len() < NONCE_SIZE {
        return None;
    }
    let (nonce_bytes, ciphertext) = payload.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);
    Aes256Gcm::new(key)
        .decrypt(nonce, ciphertext)
        .ok()
        .map(|v| String::from_utf8_lossy(&v).to_string())
}

/// Encrypt plaintext using AES-256-GCM with an Argon2id-derived key.
/// Returns the v2 envelope `v2:<base64 salt>:<base64 nonce||ciphertext>`.
pub fn encrypt(plaintext: &str, seed: &str) -> String {
    let salt: Vec<u8> = (0..SALT_SIZE).map(|_| rand::random::<u8>()).collect();
    let key = derive_key_argon2(seed, &salt);
    let combined = encrypt_with_key(plaintext, &key);
    format!("v2:{}:{}", BASE64.encode(&salt), BASE64.encode(&combined))
}

/// Decrypt a v2 envelope or a legacy v1 payload using AES-256-GCM.
pub fn decrypt(encoded: &str, seed: &str) -> Option<String> {
    if let Some(rest) = encoded.strip_prefix("v2:") {
        let (salt_b64, payload_b64) = rest.split_once(':')?;
        let salt = BASE64.decode(salt_b64.as_bytes()).ok()?;
        if salt.len() != SALT_SIZE {
            return None;
        }
        let payload = BASE64.decode(payload_b64.as_bytes()).ok()?;
        let key = derive_key_argon2(seed, &salt);
        decrypt_with_key(&payload, &key)
    } else {
        let key = derive_key_legacy(seed);
        let combined = BASE64.decode(encoded.as_bytes()).ok()?;
        decrypt_with_key(&combined, &key)
    }
}

/// Encrypt with the legacy v1 format (SHA-256 KDF, bare base64).
/// Test-only helper to prove [`decrypt`] still reads pre-migration secrets.
#[cfg(test)]
fn encrypt_legacy(plaintext: &str, seed: &str) -> String {
    let key = derive_key_legacy(seed);
    BASE64.encode(encrypt_with_key(plaintext, &key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = "my-secret-token-123";
        let seed = "sparrow-cluster-key";
        let encrypted = encrypt(original, seed);
        assert!(encrypted.starts_with("v2:"), "new secrets use v2 format");
        let decrypted = decrypt(&encrypted, seed).unwrap();
        assert_eq!(original, decrypted);
    }

    #[test]
    fn test_wrong_seed_fails() {
        let original = "secret-data";
        let encrypted = encrypt(original, "seed-a");
        let result = decrypt(&encrypted, "seed-b");
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_string() {
        let encrypted = encrypt("", "key");
        let decrypted = decrypt(&encrypted, "key").unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_legacy_v1_still_decrypts() {
        // Secret written by the pre-Argon2 code path must remain readable.
        let legacy = encrypt_legacy("old-secret", "seed");
        assert!(!legacy.contains(':'));
        assert_eq!(decrypt(&legacy, "seed").as_deref(), Some("old-secret"));
        assert!(decrypt(&legacy, "other-seed").is_none());
    }

    #[test]
    fn test_salts_are_unique() {
        // Same seed + plaintext must not produce the same envelope twice.
        let a = encrypt("same", "seed");
        let b = encrypt("same", "seed");
        assert_ne!(a, b);
        assert_eq!(decrypt(&a, "seed").as_deref(), Some("same"));
        assert_eq!(decrypt(&b, "seed").as_deref(), Some("same"));
    }

    #[test]
    fn test_malformed_v2_rejected() {
        assert!(decrypt("v2:!!!:$$$", "seed").is_none());
        assert!(decrypt("v2:only-one-part", "seed").is_none());
        // Valid base64 but wrong salt length.
        let bad = format!("v2:{}:{}", BASE64.encode(b"short"), BASE64.encode(b"x"));
        assert!(decrypt(&bad, "seed").is_none());
        assert!(decrypt("not-valid-base64!!!", "seed").is_none());
    }
}
