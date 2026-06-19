use aes_gcm::{Aes256Gcm, Key, Nonce};
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

const NONCE_SIZE: usize = 12;

/// Derive a 256-bit key from a seed string using SHA-256
fn derive_key(seed: &str) -> Key<Aes256Gcm> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    *Key::<Aes256Gcm>::from_slice(&hasher.finalize())
}

/// Encrypt plaintext using AES-256-GCM. Returns base64(nonce || ciphertext).
pub fn encrypt(plaintext: &str, seed: &str) -> String {
    let key = derive_key(seed);
    let cipher = Aes256Gcm::new(&key);
    let nonce_vec: Vec<u8> = (0..NONCE_SIZE).map(|_| rand::random::<u8>()).collect();
    let nonce = Nonce::from_slice(&nonce_vec);
    let ciphertext = cipher.encrypt(nonce, plaintext.as_bytes()).unwrap_or_default();
    let mut combined = nonce_vec.clone();
    combined.extend_from_slice(&ciphertext);
    BASE64.encode(&combined)
}

/// Decrypt base64(nonce || ciphertext) using AES-256-GCM.
pub fn decrypt(encoded: &str, seed: &str) -> Option<String> {
    let key = derive_key(seed);
    let cipher = Aes256Gcm::new(&key);
    let combined = BASE64.decode(encoded.as_bytes()).ok()?;
    if combined.len() < NONCE_SIZE { return None; }
    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext).ok().map(|v| String::from_utf8_lossy(&v).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = "my-secret-token-123";
        let seed = "sparrow-cluster-key";
        let encrypted = encrypt(original, seed);
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
}
