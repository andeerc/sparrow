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

/// Compose subset accepted by [`DeployManifest::from_compose`]:
/// `services.<name>.image` (required), `ports` (`"HOST:TARGET"` or numbers),
/// `environment` (map or `KEY=val` list), `volumes` (`"src:dst[:ro]"`),
/// `networks`, `restart`, `deploy.replicas`. Anything else is rejected
/// loudly, never silently dropped.
#[derive(Debug, Deserialize)]
pub struct ComposeFile {
    #[serde(default)]
    pub services: std::collections::HashMap<String, ComposeService>,
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
pub struct ComposeService {
    pub image: String,
    #[serde(default)]
    pub ports: Vec<ComposePort>,
    #[serde(default)]
    pub environment: ComposeEnv,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub restart: Option<String>,
    #[serde(default)]
    pub deploy: Option<ComposeDeploy>,
}

/// Compose `ports` entries: `"HOST:TARGET"`, `"TARGET"`, or numbers.
/// Long syntax (`target:/published:/mode:` maps) is rejected with a
/// pointing error — not silently misread.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ComposePort {
    Number(u16),
    Text(String),
    Long {
        target: u16,
        #[serde(default)]
        published: Option<String>,
    },
}

/// Compose `environment`: either a mapping or a `KEY=val` list.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum ComposeEnv {
    #[default]
    Empty,
    Map(std::collections::HashMap<String, Option<String>>),
    List(Vec<String>),
}

#[derive(Debug, Deserialize, Default)]
pub struct ComposeDeploy {
    #[serde(default)]
    pub replicas: Option<u32>,
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

fn default_replicas() -> u32 {
    1
}
fn default_restart() -> String {
    "always".to_string()
}
fn default_protocol() -> String {
    "tcp".to_string()
}
fn default_autoscale_min() -> u32 {
    1
}
fn default_autoscale_max() -> u32 {
    10
}
fn default_cooldown() -> u64 {
    60
}
impl DeployManifest {
    /// Parse a YAML deploy manifest from file path
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{path}': {e}"))?;
        Self::from_yaml(&content)
    }

    /// Parse a docker-compose subset file into one manifest per service.
    pub fn from_compose_file(path: &str) -> anyhow::Result<Vec<Self>> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{path}': {e}"))?;
        Self::from_compose(&content)
    }
    /// Parse a YAML deploy manifest from string
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let manifest: DeployManifest = serde_yaml::from_str(yaml)
            .map_err(|e| anyhow::anyhow!("Failed to parse deploy manifest: {}", e))?;

        if manifest.api_version != "sparrow/v1" {
            anyhow::bail!(
                "Unsupported apiVersion '{}', expected 'sparrow/v1'",
                manifest.api_version
            );
        }
        if manifest.kind != "Service" {
            anyhow::bail!("Unsupported kind '{}', expected 'Service'", manifest.kind);
        }

        Ok(manifest)
    }

    /// Translate a docker-compose subset into one [`DeployManifest`] per
    /// service. Rejects long-syntax ports, `build:`, `depends_on:`,
    /// `healthcheck:`, and any other unsupported key with the service name
    /// and offending input — never silently drops config the user depends on.
    pub fn from_compose(yaml: &str) -> anyhow::Result<Vec<Self>> {
        let raw: serde_yaml::Value = serde_yaml::from_str(yaml)
            .map_err(|e| anyhow::anyhow!("Failed to parse compose file: {e}"))?;
        reject_compose_keys(
            &raw,
            &["services", "version", "networks", "volumes"],
            "<root>",
        )?;
        let services_raw = raw
            .get("services")
            .ok_or_else(|| anyhow::anyhow!("Compose file has no services"))?;
        reject_service_keys(services_raw)?;
        let file: ComposeFile = serde_yaml::from_value(raw)
            .map_err(|e| anyhow::anyhow!("Failed to parse compose services: {e}"))?;
        if file.services.is_empty() {
            anyhow::bail!("Compose file has no services");
        }
        file.services
            .into_iter()
            .map(|(name, svc)| compose_service_to_manifest(&name, svc))
            .collect()
    }

    /// Convert to ServiceSpec for state store
    pub fn to_service_spec(&self) -> ServiceSpec {
        let image = ensure_registry(&self.spec.image);
        let mut spec = ServiceSpec::new(&self.metadata.name, &image);
        spec.desired_replicas = self.spec.replicas;

        spec.ports = self
            .spec
            .ports
            .iter()
            .map(|p| PortMapping {
                published: p.published,
                target: p.target,
                protocol: if p.protocol.to_lowercase() == "udp" {
                    Protocol::Udp
                } else {
                    Protocol::Tcp
                },
            })
            .collect();

        spec.env = self
            .spec
            .env
            .iter()
            .map(|e| EnvVar {
                key: e.name.clone(),
                value: e.value.clone(),
            })
            .collect();

        spec.volumes = self
            .spec
            .volumes
            .iter()
            .map(|v| VolumeMount {
                source: v.source.clone(),
                target: v.target.clone(),
                read_only: v.read_only,
            })
            .collect();

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

fn reject_compose_keys(
    value: &serde_yaml::Value,
    allowed: &[&str],
    where_: &str,
) -> anyhow::Result<()> {
    let map = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("{where_}: expected a mapping, got {value:?}"))?;
    for key in map.keys() {
        let k = key.as_str().unwrap_or("<non-string>");
        if !allowed.contains(&k) {
            anyhow::bail!(
                "{where_}: unsupported compose key '{k}' — Sparrow implements a subset (image, ports, environment, volumes, networks, restart, deploy.replicas); remove it or open an issue"
            );
        }
    }
    Ok(())
}

/// Reject unknown keys inside each `services.<name>` mapping before serde
/// (serde would silently ignore them and drop user config).
fn reject_service_keys(services: &serde_yaml::Value) -> anyhow::Result<()> {
    let map = services
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("services: expected a mapping"))?;
    const ALLOWED: &[&str] = &[
        "image",
        "ports",
        "environment",
        "volumes",
        "networks",
        "restart",
        "deploy",
    ];
    for (name, svc) in map {
        let svc_name = name.as_str().unwrap_or("<non-string>");
        let svc_map = svc
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("service '{svc_name}': expected a mapping"))?;
        for key in svc_map.keys() {
            let k = key.as_str().unwrap_or("<non-string>");
            if !ALLOWED.contains(&k) {
                anyhow::bail!(
                    "service '{svc_name}': unsupported compose key '{k}' — Sparrow implements a subset (image, ports, environment, volumes, networks, restart, deploy.replicas); remove it or open an issue"
                );
            }
        }
        if let Some(deploy) = svc_map.get("deploy") {
            reject_compose_keys(
                deploy,
                &["replicas"],
                &format!("service '{svc_name}'.deploy"),
            )?;
        }
    }
    Ok(())
}

fn compose_service_to_manifest(name: &str, svc: ComposeService) -> anyhow::Result<DeployManifest> {
    if svc.image.trim().is_empty() {
        anyhow::bail!("service '{name}': missing required 'image'");
    }
    let ports = svc
        .ports
        .iter()
        .map(|p| compose_port(name, p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let env = compose_env(name, &svc.environment)?;
    let volumes = svc
        .volumes
        .iter()
        .map(|v| compose_volume(name, v))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(DeployManifest {
        api_version: "sparrow/v1".to_string(),
        kind: "Service".to_string(),
        metadata: DeployMetadata {
            name: name.to_string(),
            labels: Default::default(),
            annotations: Default::default(),
        },
        spec: DeploySpec {
            image: svc.image,
            replicas: svc.deploy.and_then(|d| d.replicas).unwrap_or(1),
            ports,
            env,
            volumes,
            networks: svc.networks,
            restart: svc.restart.unwrap_or_else(|| "always".to_string()),
            domain: None,
            autoscale: None,
        },
    })
}

fn compose_port(service: &str, port: &ComposePort) -> anyhow::Result<DeployPort> {
    match port {
        ComposePort::Number(n) => Ok(DeployPort {
            published: *n,
            target: *n,
            protocol: "tcp".to_string(),
        }),
        ComposePort::Text(s) => parse_short_port(service, s),
        ComposePort::Long { .. } => anyhow::bail!(
            "service '{service}': long-syntax ports are not supported — use \"HOST:TARGET\" instead"
        ),
    }
}

fn parse_short_port(service: &str, s: &str) -> anyhow::Result<DeployPort> {
    // "HOST:TARGET", "TARGET", each optionally suffixed with /tcp|/udp.
    let (mapping, protocol) = match s.rsplit_once('/') {
        Some((m, proto))
            if proto.eq_ignore_ascii_case("tcp") || proto.eq_ignore_ascii_case("udp") =>
        {
            (m, proto.to_lowercase())
        }
        _ => (s, "tcp".to_string()),
    };
    let mut parts = mapping.split(':');
    let (published, target) = match (parts.next(), parts.next(), parts.next()) {
        (Some(t), None, None) => (t, t),
        (Some(h), Some(t), None) => (h, t),
        _ => anyhow::bail!(
            "service '{service}': invalid port mapping '{s}' — expected \"HOST:TARGET\" or \"TARGET\""
        ),
    };
    // Strip optional IP prefix ("127.0.0.1:80:80" → published 80).
    let published = published.rsplit(':').next().unwrap_or(published);
    Ok(DeployPort {
        published: published
            .parse()
            .map_err(|_| anyhow::anyhow!("service '{service}': invalid published port in '{s}'"))?,
        target: target
            .parse()
            .map_err(|_| anyhow::anyhow!("service '{service}': invalid target port in '{s}'"))?,
        protocol,
    })
}

fn compose_env(service: &str, env: &ComposeEnv) -> anyhow::Result<Vec<DeployEnv>> {
    match env {
        ComposeEnv::Empty => Ok(vec![]),
        ComposeEnv::Map(map) => map
            .iter()
            .map(|(k, v)| {
                // `KEY:` (null) without a vault secret is almost certainly a
                // mistake — fail loudly instead of injecting an empty var.
                let value = v.clone().ok_or_else(|| {
                    anyhow::anyhow!(
                        "service '{service}': env '{k}' has no value — set KEY=value or KEY=secret:name"
                    )
                })?;
                Ok(DeployEnv {
                    name: k.clone(),
                    value,
                })
            })
            .collect(),
        ComposeEnv::List(list) => list
            .iter()
            .map(|item| {
                let (k, v) = item.split_once('=').ok_or_else(|| {
                    anyhow::anyhow!(
                        "service '{service}': env '{item}' needs KEY=value form (bare keys are not supported)"
                    )
                })?;
                Ok(DeployEnv {
                    name: k.to_string(),
                    value: v.to_string(),
                })
            })
            .collect(),
    }
}

fn compose_volume(service: &str, v: &str) -> anyhow::Result<DeployVolume> {
    let mut parts = v.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(src), Some(dst), mode, None) => {
            let read_only = matches!(mode, Some(m) if m.eq_ignore_ascii_case("ro"));
            if let Some(m) = mode {
                if !(m.eq_ignore_ascii_case("ro") || m.eq_ignore_ascii_case("rw")) {
                    anyhow::bail!(
                        "service '{service}': invalid volume mode '{m}' in '{v}' — expected :ro or :rw"
                    );
                }
            }
            Ok(DeployVolume {
                source: src.to_string(),
                target: dst.to_string(),
                read_only,
            })
        }
        _ => anyhow::bail!(
            "service '{service}': invalid volume '{v}' — expected \"source:target[:ro]\""
        ),
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
        assert_eq!(
            ensure_registry("nginx:alpine"),
            "docker.io/library/nginx:alpine"
        );
        assert_eq!(ensure_registry("nginx"), "docker.io/library/nginx");
    }

    #[test]
    fn test_ensure_registry_preserves_full_path() {
        assert_eq!(
            ensure_registry("docker.io/nginx:latest"),
            "docker.io/nginx:latest"
        );
        assert_eq!(
            ensure_registry("ghcr.io/org/image:v1"),
            "ghcr.io/org/image:v1"
        );
    }

    #[test]
    fn test_ensure_registry_user_image() {
        assert_eq!(
            ensure_registry("myuser/myimage:tag"),
            "docker.io/myuser/myimage:tag"
        );
    }

    #[test]
    fn test_compose_basic_subset() {
        let yaml = r#"
services:
  web:
    image: nginx:alpine
    ports:
      - "80:80"
      - 443
    environment:
      DOMAIN: example.com
      SECRET_TOKEN: secret:db/token
    volumes:
      - /data:/usr/share/nginx/html:ro
    restart: always
    deploy:
      replicas: 2
"#;
        let manifests = DeployManifest::from_compose(yaml).unwrap();
        assert_eq!(manifests.len(), 1);
        let m = &manifests[0];
        assert_eq!(m.metadata.name, "web");
        assert_eq!(m.spec.replicas, 2);
        assert_eq!(m.spec.ports.len(), 2);
        assert_eq!(
            (m.spec.ports[0].published, m.spec.ports[0].target),
            (80, 80)
        );
        assert_eq!(
            (m.spec.ports[1].published, m.spec.ports[1].target),
            (443, 443)
        );
        assert!(m.spec.volumes[0].read_only);
        let spec = m.to_service_spec();
        assert_eq!(spec.env.len(), 2);
    }

    #[test]
    fn test_compose_env_list_form() {
        let yaml = r#"
services:
  api:
    image: myapp/api
    environment:
      - FOO=bar
      - BAZ=qux
"#;
        let manifests = DeployManifest::from_compose(yaml).unwrap();
        assert_eq!(manifests[0].spec.env.len(), 2);
    }

    #[test]
    fn test_compose_rejects_loudly() {
        // Long-syntax ports.
        let long = "services:\n  web:\n    image: nginx\n    ports:\n      - target: 80\n";
        assert!(DeployManifest::from_compose(long).is_err());
        // Missing image.
        let no_image = "services:\n  web:\n    ports:\n      - \"80:80\"\n";
        assert!(DeployManifest::from_compose(no_image).is_err());
        // Bare env key without value.
        let bare = "services:\n  web:\n    image: nginx\n    environment:\n      - FOO\n";
        assert!(DeployManifest::from_compose(bare).is_err());
        // Null map value.
        let null_map = "services:\n  web:\n    image: nginx\n    environment:\n      FOO:\n";
        assert!(DeployManifest::from_compose(null_map).is_err());
        // Bad volume.
        let bad_vol = "services:\n  web:\n    image: nginx\n    volumes:\n      - justaname\n";
        assert!(DeployManifest::from_compose(bad_vol).is_err());
        // Unknown top-level key.
        let unknown = "services:\n  web:\n    image: nginx\n    build: .\n";
        assert!(DeployManifest::from_compose(unknown).is_err());
    }
}
