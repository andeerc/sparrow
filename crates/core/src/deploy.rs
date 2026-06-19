use serde::Deserialize;
use sparrow_proto::*;

/// YAML format for declarative service deployment.
///
/// ```yaml
/// apiVersion: sparrow/v1
/// kind: Service
/// metadata:
///   name: myapp
/// spec:
///   image: nginx:alpine
///   replicas: 3
///   ports:
///     - published: 80
///       target: 80
///   env:
///     - name: DOMAIN
///       value: example.com
/// ```
#[derive(Debug, Deserialize)]
pub struct DeployManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: DeployMetadata,
    pub spec: DeploySpec,
}

#[derive(Debug, Deserialize)]
pub struct DeployMetadata {
    pub name: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub annotations: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct DeploySpec {
    pub image: String,

    #[serde(default = "default_replicas")]
    pub replicas: u32,

    #[serde(default)]
    pub ports: Vec<DeployPort>,

    #[serde(default)]
    pub env: Vec<DeployEnv>,

    #[serde(default)]
    pub volumes: Vec<DeployVolume>,

    #[serde(default)]
    pub networks: Vec<String>,

    #[serde(default = "default_restart")]
    pub restart: String,

    #[serde(default)]
    pub domain: Option<String>,

    #[serde(default)]
    pub autoscale: Option<DeployAutoscale>,
}

#[derive(Debug, Deserialize)]
pub struct DeployPort {
    pub published: u16,
    pub target: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

#[derive(Debug, Deserialize)]
pub struct DeployEnv {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct DeployVolume {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeployAutoscale {
    #[serde(default = "default_autoscale_min")]
    pub min_replicas: u32,
    #[serde(default = "default_autoscale_max")]
    pub max_replicas: u32,
    #[serde(default)]
    pub cpu_target_percent: Option<f64>,
    #[serde(default)]
    pub memory_target_percent: Option<f64>,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u64,
}

fn default_replicas() -> u32 { 1 }
fn default_restart() -> String { "always".to_string() }
fn default_protocol() -> String { "tcp".to_string() }
fn default_autoscale_min() -> u32 { 1 }
fn default_autoscale_max() -> u32 { 10 }
fn default_cooldown() -> u64 { 60 }

impl DeployManifest {
    /// Parse a YAML deploy manifest from file path
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", path, e))?;
        Self::from_yaml(&content)
    }

    /// Parse a YAML deploy manifest from string
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let manifest: DeployManifest = serde_yaml::from_str(yaml)
            .map_err(|e| anyhow::anyhow!("Failed to parse deploy manifest: {}", e))?;

        if manifest.api_version != "sparrow/v1" {
            anyhow::bail!("Unsupported apiVersion '{}', expected 'sparrow/v1'", manifest.api_version);
        }
        if manifest.kind != "Service" {
            anyhow::bail!("Unsupported kind '{}', expected 'Service'", manifest.kind);
        }

        Ok(manifest)
    }

    /// Convert to ServiceSpec for state store
    pub fn to_service_spec(&self) -> ServiceSpec {
        let image = ensure_registry(&self.spec.image);
        let mut spec = ServiceSpec::new(&self.metadata.name, &image);
        spec.desired_replicas = self.spec.replicas;

        spec.ports = self.spec.ports.iter().map(|p| PortMapping {
            published: p.published,
            target: p.target,
            protocol: if p.protocol.to_lowercase() == "udp" { Protocol::Udp } else { Protocol::Tcp },
        }).collect();

        spec.env = self.spec.env.iter().map(|e| EnvVar {
            key: e.name.clone(),
            value: e.value.clone(),
        }).collect();

        spec.volumes = self.spec.volumes.iter().map(|v| VolumeMount {
            source: v.source.clone(),
            target: v.target.clone(),
            read_only: v.read_only,
        }).collect();

        spec.networks = self.spec.networks.clone();

        spec.restart_policy = match self.spec.restart.to_lowercase().as_str() {
            "no" => RestartPolicy::No,
            "on-failure" | "onfailure" => RestartPolicy::OnFailure,
            _ => RestartPolicy::Always,
        };

        spec.autoscaling = self.spec.autoscale.as_ref().map(|a| AutoscalingConfig {
            min_replicas: a.min_replicas,
            max_replicas: a.max_replicas,
            cpu_target_percent: a.cpu_target_percent,
            memory_target_percent: a.memory_target_percent,
            cooldown_seconds: a.cooldown_seconds,
        });

        spec
    }
}

/// If image name has no registry prefix (no dot or colon before first slash),
/// prepend docker.io/library/.
fn ensure_registry(image: &str) -> String {
    let has_registry = image.contains('/') && (
        image.starts_with("docker.io/") ||
        image.starts_with("ghcr.io/") ||
        image.starts_with("quay.io/") ||
        image.starts_with("registry.") ||
        image.starts_with("localhost/")
    );
    // Also detect if first segment before / contains a dot (domain) or colon (port)
    let has_domain = if let Some((prefix, _)) = image.split_once('/') {
        prefix.contains('.') || prefix.contains(':')
    } else {
        false
    };

    if image.contains('/') && (has_registry || has_domain) {
        image.to_string()
    } else if image.contains('/') {
        // Has a slash but no domain-like prefix: e.g. "myuser/myimage"
        format!("docker.io/{image}")
    } else {
        // Plain name like "nginx:alpine"
        format!("docker.io/library/{image}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal() {
        let yaml = r#"
apiVersion: sparrow/v1
kind: Service
metadata:
  name: test-app
spec:
  image: nginx:alpine
"#;
        let manifest = DeployManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.metadata.name, "test-app");
        assert_eq!(manifest.spec.image, "nginx:alpine");
        assert_eq!(manifest.spec.replicas, 1);
    }

    #[test]
    fn test_parse_full() {
        let yaml = r#"
apiVersion: sparrow/v1
kind: Service
metadata:
  name: web
spec:
  image: nginx:1.25
  replicas: 3
  ports:
    - published: 80
      target: 80
    - published: 443
      target: 443
      protocol: TCP
  env:
    - name: DOMAIN
      value: example.com
  networks:
    - overlay-net
  restart: always
  autoscale:
    min_replicas: 2
    max_replicas: 10
    cpu_target_percent: 70
    cooldown_seconds: 60
"#;
        let manifest = DeployManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.spec.replicas, 3);
        assert_eq!(manifest.spec.ports.len(), 2);
        assert_eq!(manifest.spec.env.len(), 1);
        assert!(manifest.spec.autoscale.is_some());
        let as_config = manifest.spec.autoscale.unwrap();
        assert_eq!(as_config.min_replicas, 2);
        assert_eq!(as_config.max_replicas, 10);
        assert_eq!(as_config.cpu_target_percent, Some(70.0));
    }

    #[test]
    fn test_rejects_invalid_version() {
        let yaml = r#"
apiVersion: v1
kind: Service
metadata:
  name: test
spec:
  image: nginx
"#;
        assert!(DeployManifest::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_to_service_spec() {
        let yaml = r#"
apiVersion: sparrow/v1
kind: Service
metadata:
  name: test-app
spec:
  image: nginx:alpine
  replicas: 3
  ports:
    - published: 80
      target: 8080
  env:
    - name: FOO
      value: bar
"#;
        let manifest = DeployManifest::from_yaml(yaml).unwrap();
        let spec = manifest.to_service_spec();
        assert_eq!(spec.name, "test-app");
        assert_eq!(spec.image, "docker.io/library/nginx:alpine");
        assert_eq!(spec.desired_replicas, 3);
        assert_eq!(spec.ports.len(), 1);
        assert_eq!(spec.ports[0].published, 80);
        assert_eq!(spec.ports[0].target, 8080);
        assert_eq!(spec.env.len(), 1);
        assert_eq!(spec.env[0].key, "FOO");
        assert_eq!(spec.env[0].value, "bar");
    }

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
