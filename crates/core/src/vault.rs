use std::path::Path;
use std::fs;
use rand::Rng;

use crate::crypto;

/// Vault manages encryption keys and secret operations.
/// Key sourced from: SPARROW_VAULT_KEY env var > vault.key file > init()
pub struct Vault {
    seed: String,
}

impl Vault {
    /// Get the vault seed (for sharing with API state).
    pub fn seed(&self) -> &str {
        &self.seed
    }
    /// Open vault — reads key from env var or config file.
    /// Returns error if no key found.
    pub fn open(config_dir: &Path) -> Result<Self, anyhow::Error> {
        if let Ok(key) = std::env::var("SPARROW_VAULT_KEY") {
            if !key.is_empty() {
                return Ok(Vault { seed: key });
            }
        }
        let key_path = config_dir.join("vault.key");
        if key_path.exists() {
            let key = fs::read_to_string(&key_path)?.trim().to_string();
            if !key.is_empty() {
                return Ok(Vault { seed: key });
            }
        }
        anyhow::bail!("No vault key found. Run `sparrow secret init` or set SPARROW_VAULT_KEY")
    }

    /// Init vault — generates a random key and writes to vault.key.
    pub fn init(config_dir: &Path) -> Result<Self, anyhow::Error> {
        let seed: String = (0..32).map(|_| format!("{:02x}", rand::thread_rng().gen::<u8>())).collect();
        fs::create_dir_all(config_dir)?;
        let key_path = config_dir.join("vault.key");
        fs::write(&key_path, &seed)?;
        // Restrict permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }
        println!("🔐 Vault initialized: {key_path:?}");
        Ok(Vault { seed })
    }

    /// Encrypt plaintext using AES-256-GCM.
    pub fn encrypt(&self, plaintext: &str) -> String {
        crypto::encrypt(plaintext, &self.seed)
    }

    /// Decrypt ciphertext using AES-256-GCM.
    pub fn decrypt(&self, ciphertext: &str) -> Option<String> {
        crypto::decrypt(ciphertext, &self.seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_vault_init_and_roundtrip() {
        let dir = TempDir::new().unwrap();
        let vault = Vault::init(dir.path()).unwrap();
        let enc = vault.encrypt("mysecret");
        let dec = vault.decrypt(&enc).unwrap();
        assert_eq!(dec, "mysecret");
    }

    #[test]
    fn test_vault_open_from_file() {
        let dir = TempDir::new().unwrap();
        let vault = Vault::init(dir.path()).unwrap();
        let enc = vault.encrypt("data");
        // Open a new vault instance from the same key file
        let vault2 = Vault::open(dir.path()).unwrap();
        let dec = vault2.decrypt(&enc).unwrap();
        assert_eq!(dec, "data");
    }

    #[test]
    fn test_vault_open_missing_key() {
        let dir = TempDir::new().unwrap();
        assert!(Vault::open(dir.path()).is_err());
    }
}
