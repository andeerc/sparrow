use std::collections::HashMap;

use sparrow_proto::*;

/// Abstraction over container runtime (Podman CLI).
/// In Fase 1 this will have full Podman command execution.
pub struct PodmanRuntime {
    rootless: bool,
    socket_path: Option<String>,
}

impl PodmanRuntime {
    pub fn new(rootless: bool) -> Self {
        Self {
            rootless,
            socket_path: None,
        }
    }

    pub fn with_socket(mut self, path: &str) -> Self {
        self.socket_path = Some(path.to_string());
        self
    }

    /// Check if Podman is available
    pub async fn check_available(&self) -> anyhow::Result<bool> {
        let output = tokio::process::Command::new("podman")
            .arg("--version")
            .output()
            .await?;

        Ok(output.status.success())
    }

    /// Run a container
    pub async fn run_container(
        &self,
        name: &str,
        image: &str,
        ports: &[PortMapping],
        env: &[(String, String)],
        labels: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        let image = ensure_registry(image);
        let mut cmd = tokio::process::Command::new("podman");

        if self.rootless {
            // Rootless is default behavior
        }

        cmd.arg("run")
            .arg("-d")
            .arg("--name")
            .arg(name)
            .arg("--label")
            .arg(format!("sparrow.service={}", name.split('-').next().unwrap_or(name)));

        for port in ports {
            cmd.arg("-p").arg(format!("{}:{}/{}", port.published, port.target, match port.protocol {
                Protocol::Tcp => "tcp",
                Protocol::Udp => "udp",
            }));
        }

        for (k, v) in env {
            cmd.arg("-e").arg(format!("{}={}", k, v));
        }

        for (k, v) in labels {
            cmd.arg("--label").arg(format!("{}.{}={}", "sparrow", k, v));
        }

        cmd.arg("--restart").arg(match name.contains("test") {
            true => "no",
            false => "always",
        });

        cmd.arg(image);

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Podman run failed: {}", stderr));
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(container_id)
    }

    /// Stop and remove a container
    pub async fn remove_container(&self, name: &str) -> anyhow::Result<()> {
        let output = tokio::process::Command::new("podman")
            .args(["rm", "-f", name])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no such container") {
                return Err(anyhow::anyhow!("Podman rm failed: {}", stderr));
            }
        }

        Ok(())
    }

    /// Get container logs (non-following, returns all lines)
    pub async fn logs(&self, name: &str, tail: u32) -> anyhow::Result<Vec<String>> {
        let output = tokio::process::Command::new("podman")
            .args(["logs", "--tail", &tail.to_string(), name])
            .output()
            .await?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let logs = String::from_utf8_lossy(&output.stdout);
        Ok(logs.lines().map(|l| l.to_string()).collect())
    }

    /// Stream container logs in real-time (follow mode).
    /// Spawns podman logs --follow and sends each line through the sender.
    /// Caller should cancel via the child handle when done.
    pub async fn logs_follow(
        &self,
        name: &str,
        tail: u32,
        tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<tokio::process::Child> {
        let mut child = tokio::process::Command::new("podman")
            .args(["logs", "--follow", "--tail", &tail.to_string(), name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Read stdout
        let tx1 = tx.clone();
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stdout);
            use tokio::io::AsyncBufReadExt;
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx1.send(line).is_err() {
                    break;
                }
            }
        });

        // Read stderr
        tokio::spawn(async move {
            let reader = tokio::io::BufReader::new(stderr);
            use tokio::io::AsyncBufReadExt;
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(format!("[stderr] {line}")).is_err() {
                    break;
                }
            }
        });

        Ok(child)
    }

    /// Inspect container status
    pub async fn inspect_container(&self, name: &str) -> anyhow::Result<ContainerStatus> {
        let output = tokio::process::Command::new("podman")
            .args(["inspect", name])
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Container '{}' not found", name));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let inspect: serde_json::Value = serde_json::from_str(&stdout)?;

        let state = inspect[0]["State"]["Status"]
            .as_str()
            .unwrap_or("unknown");

        let container_state = match state {
            "running" => ContainerState::Running,
            "exited" => ContainerState::Exited,
            "paused" => ContainerState::Paused,
            "created" => ContainerState::Created,
            _ => ContainerState::Unknown,
        };

        Ok(ContainerStatus {
            id: inspect[0]["Id"].as_str().unwrap_or("").to_string(),
            service_id: String::new(),
            node_id: String::new(),
            name: name.to_string(),
            image: inspect[0]["Config"]["Image"].as_str().unwrap_or("").to_string(),
            state: container_state,
            exit_code: inspect[0]["State"]["ExitCode"].as_i64().map(|c| c as i32),
            cpu_percent: None,
            mem_bytes: None,
            started_at: None,
            ip_address: inspect[0]["NetworkSettings"]["IPAddress"]
                .as_str()
                .map(|s| s.to_string()),
        })
    }

    /// List containers for a service
    pub async fn list_containers(&self, service_name: &str) -> anyhow::Result<Vec<ContainerStatus>> {
        let output = tokio::process::Command::new("podman")
            .args([
                "ps", "-a",
                "--filter", &format!("label=sparrow.service={}", service_name),
                "--format", "{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut containers = vec![];

        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let state = if parts[1].starts_with("Up") {
                    ContainerState::Running
                } else if parts[1].starts_with("Exited") {
                    ContainerState::Exited
                } else {
                    ContainerState::Unknown
                };

                containers.push(ContainerStatus {
                    id: parts[0].to_string(),
                    service_id: service_name.to_string(),
                    node_id: String::new(),
                    name: parts[0].to_string(),
                    image: parts.get(2).unwrap_or(&"").to_string(),
                    state,
                    exit_code: None,
                    cpu_percent: None,
                    mem_bytes: None,
                    started_at: None,
                    ip_address: None,
                });
            }
        }

        Ok(containers)
    }

    /// Get stats (CPU, memory) for a container
    pub async fn stats(&self, name: &str) -> anyhow::Result<(f64, u64)> {
        let output = tokio::process::Command::new("podman")
            .args([
                "stats",
                "--no-stream",
                "--format",
                "{{.CPUPerc}}\t{{.MemUsage}}",
                name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Ok((0.0, 0));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split('\t').collect();

        let cpu = parts
            .first()
            .and_then(|s| s.trim_end_matches('%').parse::<f64>().ok())
            .unwrap_or(0.0);

        let mem_str = parts.get(1).unwrap_or(&"0B");
        let mem = parse_memory(mem_str).unwrap_or(0);

        Ok((cpu, mem))
    }
}

fn parse_memory(s: &str) -> Option<u64> {
    // Format: "123.4MB / 1.0GB"
    let part = s.split('/').next()?.trim();
    let (num, unit) = parse_number_unit(part)?;

    let bytes = match unit.as_str() {
        "B" => num as u64,
        "KiB" | "KB" | "K" => (num * 1024.0) as u64,
        "MiB" | "MB" | "M" => (num * 1024.0 * 1024.0) as u64,
        "GiB" | "GB" | "G" => (num * 1024.0 * 1024.0 * 1024.0) as u64,
        _ => return None,
    };

    Some(bytes)
}

fn parse_number_unit(s: &str) -> Option<(f64, String)> {
    let s = s.trim();
    let num_end = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    let num: f64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim().to_string();
    Some((num, unit))
}

/// Podman (unlike Docker) doesn't auto-resolve Docker Hub short names.
/// Prepend `docker.io/library/` when no registry and no slash (plain name like "nginx").
/// Prepend `docker.io/` when no registry but has a slash (user/image like "myuser/myapp").
fn ensure_registry(image: &str) -> String {
    // Has scheme -> keep as-is (e.g. docker://, https://)
    if image.contains("://") {
        return image.to_string();
    }
    // Localhost -> keep as-is
    if image.starts_with("localhost") {
        return image.to_string();
    }
    // Split at first slash to check if registry is present
    match image.split_once('/') {
        None => {
            // Plain name: "nginx:latest" -> docker.io/library/nginx:latest
            format!("docker.io/library/{image}")
        }
        Some((first, _rest)) => {
            if first.contains('.') || first.contains(':') {
                // Has registry: "docker.io/...", "ghcr.io/...", "localhost:5000/..."
                image.to_string()
            } else {
                // User/image: "myuser/myapp" -> docker.io/myuser/myapp
                format!("docker.io/{image}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_registry_adds_docker_io() {
        assert_eq!(ensure_registry("nginx:alpine"), "docker.io/library/nginx:alpine");
        assert_eq!(ensure_registry("nginx"), "docker.io/library/nginx");
    }

    #[test]
    fn test_ensure_registry_preserves_full_path() {
        assert_eq!(ensure_registry("docker.io/nginx:latest"), "docker.io/nginx:latest");
        assert_eq!(ensure_registry("ghcr.io/org/image:v1"), "ghcr.io/org/image:v1");
    }

    #[test]
    fn test_ensure_registry_user_image() {
        assert_eq!(ensure_registry("myuser/myimage:tag"), "docker.io/myuser/myimage:tag");
    }
}
