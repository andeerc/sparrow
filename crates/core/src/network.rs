use std::fs;
use std::path::Path;
use tokio::process::Command;

/// Wireguard Config generator and setup helpers.
pub struct WireguardManager;

impl WireguardManager {
    /// Generate public and private wireguard keys using `wg genkey` and `wg pubkey` commands.
    pub async fn generate_keys() -> anyhow::Result<(String, String)> {
        let private_out = Command::new("wg").arg("genkey").output().await?;
        if !private_out.status.success() {
            anyhow::bail!(
                "Failed to generate private key: {}",
                String::from_utf8_lossy(&private_out.stderr)
            );
        }
        let private_key = String::from_utf8(private_out.stdout)?.trim().to_string();

        let mut child = Command::new("wg")
            .arg("pubkey")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        use tokio::io::AsyncWriteExt;
        stdin.write_all(private_key.as_bytes()).await?;
        drop(stdin);

        let public_out = child.wait_with_output().await?;
        if !public_out.status.success() {
            anyhow::bail!(
                "Failed to generate public key: {}",
                String::from_utf8_lossy(&public_out.stderr)
            );
        }
        let public_key = String::from_utf8(public_out.stdout)?.trim().to_string();

        Ok((private_key, public_key))
    }

    /// Write wireguard wg0 config file.
    pub fn write_config(
        config_path: &Path,
        private_key: &str,
        address: &str,
        listen_port: u16,
        peers: &[(String, String, String)], // (public_key, endpoint_addr, allowed_ips)
    ) -> anyhow::Result<()> {
        let mut content = format!(
            "[Interface]\nPrivateKey = {}\nAddress = {}\nListenPort = {}\n\n",
            private_key, address, listen_port
        );

        for (pub_key, endpoint, allowed_ips) in peers {
            content.push_str(&format!(
                "[Peer]\nPublicKey = {}\nEndpoint = {}\nAllowedIPs = {}\nPersistentKeepalive = 25\n\n",
                pub_key, endpoint, allowed_ips
            ));
        }

        fs::write(config_path, content)?;
        Ok(())
    }

    /// Up the interface using wg-quick (needs root access or appropriate privileges).
    pub async fn up(config_path: &Path) -> anyhow::Result<()> {
        let out = Command::new("wg-quick")
            .args(["up", &config_path.to_string_lossy()])
            .output()
            .await?;
        if !out.status.success() {
            anyhow::bail!(
                "wg-quick up failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// Down the interface.
    pub async fn down(config_path: &Path) -> anyhow::Result<()> {
        let out = Command::new("wg-quick")
            .args(["down", &config_path.to_string_lossy()])
            .output()
            .await?;
        if !out.status.success() {
            anyhow::bail!(
                "wg-quick down failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }
}
