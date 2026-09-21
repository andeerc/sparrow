use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;
use tracing::warn;

const REPO_OWNER: &str = "andeerc";
const REPO_NAME: &str = "sparrow";
const API_BASE: &str = "https://api.github.com";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub struct UpdateInfo {
    pub current_tag: String,
    pub latest_tag: String,
    pub download_url: String,
    pub download_size: u64,
    pub html_url: String,
}

fn platform_asset_name(tag: &str) -> String {
    let arch = std::env::consts::ARCH;
    format!("sparrow-{tag}-{arch}-linux")
}

pub async fn check() -> Result<Option<UpdateInfo>> {
    let current = env!("CARGO_PKG_VERSION");
    let current_tag = format!("v{current}");

    let url = format!("{API_BASE}/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "sparrow-update-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Failed to fetch latest release from GitHub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("GitHub API returned {status}: {body}"));
    }

    let release: Release = resp.json().await.context("Failed to parse release JSON")?;

    if release.tag_name <= current_tag {
        return Ok(None);
    }

    let expected = platform_asset_name(&release.tag_name);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == expected)
        .ok_or_else(|| {
            anyhow!(
                "No asset for platform ({expected}) in release {}. Available: {}",
                release.tag_name,
                release
                    .assets
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    Ok(Some(UpdateInfo {
        current_tag,
        latest_tag: release.tag_name,
        download_url: asset.browser_download_url.clone(),
        download_size: asset.size,
        html_url: release.html_url,
    }))
}

pub async fn install(download_url: &str) -> Result<()> {
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;

    println!("📥 Downloading {download_url} ...");
    let response = reqwest::Client::new()
        .get(download_url)
        .header("User-Agent", "sparrow-update-check")
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .context("Failed to download update")?;

    if !response.status().is_success() {
        return Err(anyhow!("Download failed with HTTP {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read download stream")?;
    println!("  ✅ Downloaded {} bytes", bytes.len());

    // Write to /tmp (always writable by user)
    let temp_path = Path::new("/tmp").join(format!(".sparrow-update-{}", std::process::id()));

    std::fs::write(&temp_path, &bytes).context("Failed to write update to temp file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755))
            .context("Failed to make update executable")?;
    }

    // Back up the running binary BEFORE replacing it, so `update rollback`
    // can restore it. Best-effort: a failed backup aborts, a failed restore
    // of the backup is reported but never deletes the backup.
    let backup_path = current_exe.with_extension("pre-update-backup");
    std::fs::copy(&current_exe, &backup_path).context(format!(
        "Failed to back up current binary to {}",
        backup_path.display()
    ))?;
    println!("  💾 Backup saved to {}", backup_path.display());

    // Try atomic rename (same filesystem). If fails, try copy.
    // If copy fails with PermissionDenied, tell user to use sudo.
    let replace = || -> std::io::Result<()> {
        // Try rename first
        if std::fs::rename(&temp_path, &current_exe).is_ok() {
            return Ok(());
        }
        // Fallback: copy + remove temp
        std::fs::copy(&temp_path, &current_exe)?;
        let _ = std::fs::remove_file(&temp_path);
        Ok(())
    };

    replace().map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            anyhow!("Permission denied. Run: sudo sparrow update install")
        } else {
            anyhow!("Failed to replace current binary: {e}")
        }
    })?;

    println!("✅ Update installed! Restarting Sparrow service...");

    let user = std::env::var("SUDO_USER").unwrap_or_else(|_| whoami());
    restart_systemd(&user);

    Ok(())
}

/// Restore the binary saved by the last [`install`].
/// Refuses when no backup exists instead of guessing.
pub async fn rollback() -> Result<()> {
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
    let backup_path = current_exe.with_extension("pre-update-backup");
    if !backup_path.exists() {
        anyhow::bail!(
            "No backup found at {} — nothing to roll back to",
            backup_path.display()
        );
    }
    std::fs::copy(&backup_path, &current_exe).context("Failed to restore backup binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755))
            .context("Failed to make restored binary executable")?;
    }
    println!("↩️ Rolled back to {}", backup_path.display());
    let user = std::env::var("SUDO_USER").unwrap_or_else(|_| whoami());
    restart_systemd(&user);
    Ok(())
}
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".into())
}

fn restart_systemd(user: &str) {
    let service = format!("sparrow@{user}");
    match std::process::Command::new("systemctl")
        .args(["restart", &service])
        .output()
    {
        Ok(out) if out.status.success() => {
            println!("  ✅ Service {service} restarted");
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            warn!("Failed to restart {service}: {stderr}");
            eprintln!("  Run manually: sudo systemctl restart {service}");
        }
        Err(_) => {
            warn!("systemctl not available. Restart Sparrow manually.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_asset_name() {
        let name = platform_asset_name("v0.2.0");
        let arch = std::env::consts::ARCH;
        assert_eq!(name, format!("sparrow-v0.2.0-{arch}-linux"));
    }

    #[test]
    fn test_platform_asset_name_includes_tag() {
        let name = platform_asset_name("v1.0.0");
        assert!(name.contains("v1.0.0"));
        assert!(name.contains("linux"));
    }

    #[test]
    fn test_update_info_struct() {
        let info = UpdateInfo {
            current_tag: "v0.1.9".into(),
            latest_tag: "v0.2.0".into(),
            download_url: "https://example.com/sparrow".into(),
            download_size: 12345,
            html_url: "https://example.com/release".into(),
        };
        assert_eq!(info.current_tag, "v0.1.9");
        assert_eq!(info.latest_tag, "v0.2.0");
        assert_eq!(info.download_size, 12345);
    }

    #[test]
    fn test_github_release_json_shape() {
        // api.github.com/repos/{owner}/{repo}/releases/latest shape:
        // must deserialize into Release (tag_name, assets[], html_url).
        let body = serde_json::json!({
            "tag_name": "v0.9.7",
            "html_url": "https://github.com/andeerc/sparrow/releases/tag/v0.9.7",
            "assets": [{
                "name": "sparrow-v0.9.7-x86_64-linux",
                "browser_download_url": "https://github.com/andeerc/sparrow/releases/download/v0.9.7/sparrow-v0.9.7-x86_64-linux",
                "size": 42
            }]
        });
        let release: Release = serde_json::from_value(body).unwrap();
        assert_eq!(release.tag_name, "v0.9.7");
        assert_eq!(
            release.assets[0]
                .browser_download_url
                .split('/')
                .next_back()
                .unwrap(),
            "sparrow-v0.9.7-x86_64-linux"
        );
    }
}
