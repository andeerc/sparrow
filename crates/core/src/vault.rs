use rand::Rng;
use std::fs;
use std::path::Path;

use crate::crypto;
use crate::error::SparrowError;

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
        let seed: String = (0..32)
            .map(|_| format!("{:02x}", rand::thread_rng().gen::<u8>()))
            .collect();
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

/// Resolves environment variable secrets of the form `secret:name` by decrypting them using the vault.
///
/// Returns `Err` when a referenced secret is missing or undecryptable — callers
/// MUST propagate instead of booting containers with empty credentials.
/// Plain (non-`secret:`) vars resolve without touching the vault.
pub fn resolve_secrets(
    env_vars: &[sparrow_proto::EnvVar],
    config_dir: &Path,
    state: &crate::state::StateStore,
) -> Result<Vec<(String, String)>, SparrowError> {
    if !env_vars.iter().any(|e| e.value.starts_with("secret:")) {
        return Ok(env_vars
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect());
    }
    let vault = Vault::open(config_dir).map_err(|e| SparrowError::VaultError(e.to_string()))?;
    env_vars
        .iter()
        .map(|e| {
            if let Some(name) = e.value.strip_prefix("secret:") {
                let encrypted = state
                    .get_secret(name)
                    .map_err(|e| SparrowError::Internal(e.to_string()))?
                    .ok_or_else(|| SparrowError::SecretNotFound(name.to_string()))?;
                let decrypted = vault.decrypt(&encrypted).ok_or_else(|| {
                    SparrowError::VaultError(format!("Failed to decrypt secret '{name}'"))
                })?;
                Ok((e.key.clone(), decrypted))
            } else {
                Ok((e.key.clone(), e.value.clone()))
            }
        })
        .collect()
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

    #[test]
    fn test_resolve_secrets() {
        let dir = TempDir::new().unwrap();
        let vault = Vault::init(dir.path()).unwrap();
        let db = dir.path().join("test.db");
        let state = crate::state::StateStore::new(db.to_str().unwrap()).unwrap();

        // Save a mock secret
        let enc = vault.encrypt("superpassword");
        state.set_secret("db/password", &enc).unwrap();

        let env_vars = vec![
            sparrow_proto::EnvVar {
                key: "PLAIN_VAR".to_string(),
                value: "plainvalue".to_string(),
            },
            sparrow_proto::EnvVar {
                key: "SECRET_VAR".to_string(),
                value: "secret:db/password".to_string(),
            },
        ];

        let resolved = resolve_secrets(&env_vars, dir.path(), &state).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved[0],
            ("PLAIN_VAR".to_string(), "plainvalue".to_string())
        );
        assert_eq!(
            resolved[1],
            ("SECRET_VAR".to_string(), "superpassword".to_string())
        );
    }

    #[test]
    fn test_resolve_secrets_missing_fails() {
        let dir = TempDir::new().unwrap();
        Vault::init(dir.path()).unwrap();
        let db = dir.path().join("test.db");
        let state = crate::state::StateStore::new(db.to_str().unwrap()).unwrap();
        let env_vars = vec![sparrow_proto::EnvVar {
            key: "SECRET_VAR".to_string(),
            value: "secret:db/missing".to_string(),
        }];
        let err = resolve_secrets(&env_vars, dir.path(), &state).unwrap_err();
        assert!(matches!(err, crate::error::SparrowError::SecretNotFound(_)));
    }

    #[test]
    fn test_resolve_secrets_corrupt_fails() {
        let dir = TempDir::new().unwrap();
        Vault::init(dir.path()).unwrap();
        let db = dir.path().join("test.db");
        let state = crate::state::StateStore::new(db.to_str().unwrap()).unwrap();
        state.set_secret("db/bad", "not-valid-base64!!!").unwrap();
        let env_vars = vec![sparrow_proto::EnvVar {
            key: "SECRET_VAR".to_string(),
            value: "secret:db/bad".to_string(),
        }];
        let err = resolve_secrets(&env_vars, dir.path(), &state).unwrap_err();
        assert!(matches!(err, crate::error::SparrowError::VaultError(_)));
    }

    #[test]
    fn test_resolve_secrets_plain_without_vault() {
        // No vault.key in dir, but no secret: refs either — must succeed.
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("test.db");
        let state = crate::state::StateStore::new(db.to_str().unwrap()).unwrap();
        let env_vars = vec![sparrow_proto::EnvVar {
            key: "PLAIN_VAR".to_string(),
            value: "plainvalue".to_string(),
        }];
        let resolved = resolve_secrets(&env_vars, dir.path(), &state).unwrap();
        assert_eq!(
            resolved,
            vec![("PLAIN_VAR".to_string(), "plainvalue".to_string())]
        );
    }
}
