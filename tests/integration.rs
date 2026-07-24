use sparrow_core::cli::*;
use sparrow_core::config::SparrowConfig;
use sparrow_core::deploy::DeployManifest;
use sparrow_core::state::StateStore;
use sparrow_core::vault::Vault;
use sparrow_proto::*;

// ── Fixtures ──

fn setup_store() -> (StateStore, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("test.db");
    let store = StateStore::new(db.to_str().unwrap()).unwrap();
    (store, dir)
}

fn make_spec(name: &str, image: &str, replicas: u32) -> ServiceSpec {
    let mut spec = ServiceSpec::new(name, image);
    spec.desired_replicas = replicas;
    spec.ports = vec![PortMapping {
        published: 80,
        target: 8080,
        protocol: Protocol::Tcp,
    }];
    spec.env = vec![EnvVar {
        key: "ENV".to_string(),
        value: "prod".to_string(),
    }];
    spec.volumes = vec![VolumeMount {
        source: "/host/data".to_string(),
        target: "/container/data".to_string(),
        read_only: true,
    }];
    spec.networks = vec!["overlay-net".to_string()];
    spec
}

// ── State Store E2E ──

#[test]
fn test_state_store_full_lifecycle() {
    let (store, _dir) = setup_store();

    // Create
    let spec = make_spec("e2e-test", "nginx:alpine", 3);
    store.create_service(&spec).unwrap();

    // List
    let services = store.list_services().unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "e2e-test");

    // Get by ID
    let found = store.get_service(&spec.id).unwrap().unwrap();
    assert_eq!(found.name, "e2e-test");

    // Get by name
    let found = store.get_service("e2e-test").unwrap().unwrap();
    assert_eq!(found.image, "nginx:alpine");

    // Verify ports, env, volumes, networks persisted
    assert!(!found.ports.is_empty());
    assert!(!found.env.is_empty());
    assert!(!found.volumes.is_empty());
    assert!(!found.networks.is_empty());

    // Scale
    store.update_replicas(&spec.id, 5).unwrap();
    let updated = store.get_service(&spec.id).unwrap().unwrap();
    assert_eq!(updated.desired_replicas, 5);

    // Autoscale config
    let as_config = AutoscalingConfig {
        min_replicas: 1,
        max_replicas: 10,
        cpu_target_percent: Some(70.0),
        memory_target_percent: None,
        cooldown_seconds: 60,
    };
    store.set_autoscale(&spec.id, &as_config, false).unwrap();
    let (loaded, paused) = store.get_autoscale(&spec.id).unwrap().unwrap();
    assert_eq!(loaded.max_replicas, 10);
    assert!(!paused);

    // Container tracking
    store
        .record_container(
            "e2e-test-1",
            &spec.id,
            "nginx:alpine",
            1,
            "Running",
            "10.0.0.1",
        )
        .unwrap();
    let containers = store.get_service_containers(&spec.id).unwrap();
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].ip_address, Some("10.0.0.1".to_string()));

    // Delete
    store.delete_service(&spec.id).unwrap();
    assert!(store.get_service(&spec.id).unwrap().is_none());
}

// ── Autoscale Metrics History ──

#[test]
fn test_autoscale_metrics_history() {
    let (store, _dir) = setup_store();
    let spec = make_spec("metrics-svc", "redis:7", 2);
    store.create_service(&spec).unwrap();

    // Record metrics over time
    store
        .record_autoscale_metric(&spec.id, 45.0, 30.0, 2)
        .unwrap();
    store
        .record_autoscale_metric(&spec.id, 55.0, 35.0, 2)
        .unwrap();
    store
        .record_autoscale_metric(&spec.id, 65.0, 40.0, 2)
        .unwrap();

    let history = store.load_recent_metrics(&spec.id, 10).unwrap();
    assert_eq!(history.len(), 3);
    // Check ordering (oldest first)
    assert_eq!(history[0].0, 45.0);
    assert_eq!(history[2].0, 65.0);
}

// ── Proxy Routes Persistence ──

#[test]
fn test_proxy_routes_persistence() {
    let (store, _dir) = setup_store();

    store
        .save_proxy_route("app.example.com", 3000, "web", true)
        .unwrap();
    store
        .save_proxy_route("api.example.com", 8080, "api", false)
        .unwrap();

    let routes = store.list_proxy_routes().unwrap();
    assert_eq!(routes.len(), 2);

    store.delete_proxy_route("app.example.com").unwrap();
    let routes = store.list_proxy_routes().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0, "api.example.com");
}

// ── Deploy Manifest Parsing ──

#[test]
fn test_deploy_manifest_full() {
    let yaml = r#"
apiVersion: sparrow/v1
kind: Service
metadata:
  name: web-app
spec:
  image: nginx:1.25
  replicas: 3
  ports:
    - published: 80
      target: 80
    - published: 443
      target: 443
      protocol: UDP
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
    assert_eq!(manifest.spec.image, "nginx:1.25");
    assert_eq!(manifest.spec.replicas, 3);
    assert_eq!(manifest.spec.ports.len(), 2);
    assert_eq!(manifest.spec.env.len(), 1);
    assert_eq!(manifest.spec.networks.len(), 1);
    assert!(manifest.spec.autoscale.is_some());

    let spec = manifest.to_service_spec();
    assert_eq!(spec.name, "web-app");
    assert_eq!(spec.desired_replicas, 3);
    assert_eq!(spec.autoscaling.unwrap().min_replicas, 2);
}

// ── Config ──

#[test]
fn test_config_default_and_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("sparrow.yaml");

    let config = SparrowConfig::default();
    config.save(&path).unwrap();

    let loaded = SparrowConfig::load(&path).unwrap();
    assert_eq!(loaded.cluster.name, "sparrow");
    assert_eq!(loaded.cluster.listen, "0.0.0.0:7443");
    assert_eq!(loaded.runtime.backend, "podman");
    assert!(loaded.runtime.rootless);
}

// ── Vault ──

#[test]
fn test_vault_secret_lifecycle() {
    let dir = tempfile::TempDir::new().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let (store, _dir) = setup_store();

    // Store encrypted secret
    let enc = vault.encrypt("my-super-secret");
    store.set_secret("db/password", &enc).unwrap();

    // Retrieve and decrypt
    let stored = store.get_secret("db/password").unwrap().unwrap();
    let decrypted = vault.decrypt(&stored).unwrap();
    assert_eq!(decrypted, "my-super-secret");

    // List
    let names = store.list_secrets().unwrap();
    assert_eq!(names, vec!["db/password"]);

    // Delete
    assert!(store.delete_secret("db/password").unwrap());
    assert!(store.get_secret("db/password").unwrap().is_none());
}

// ── CLI Parsing ──

#[test]
fn test_cli_parse_create() {
    use clap::Parser;
    let args = vec![
        "sparrow",
        "service",
        "create",
        "--name",
        "myapp",
        "--image",
        "nginx",
        "--replicas",
        "3",
        "--port",
        "80:80",
        "--env",
        "KEY=val",
        "--volume",
        "/src:/dst:ro",
    ];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Command::Service { action } => match action {
            ServiceAction::Create {
                name,
                image: _,
                replicas,
                port,
                env,
                volume,
                ..
            } => {
                assert_eq!(name, "myapp");
                assert_eq!(replicas, 3);
                assert_eq!(port, vec!["80:80"]);
                assert_eq!(env, vec!["KEY=val"]);
                assert_eq!(volume, vec!["/src:/dst:ro"]);
            }
            _ => panic!("expected Create"),
        },
        _ => panic!("expected Service command"),
    }
}

// ── Proxy Route Matching ──

#[test]
fn test_proxy_route_matching() {
    let (store, _dir) = setup_store();
    store
        .save_proxy_route("app.example.com", 3000, "web", true)
        .unwrap();
    let routes = store.list_proxy_routes().unwrap();

    // Exact match
    let found = routes.iter().find(|r| r.0 == "app.example.com");
    assert!(found.is_some());
    assert_eq!(found.unwrap().1, 3000);

    // No match
    let missing = routes.iter().find(|r| r.0 == "other.example.com");
    assert!(missing.is_none());
}

// ── Active Container IPs ──

#[test]
fn test_active_container_ips_query() {
    let (store, _dir) = setup_store();
    let spec = make_spec("my-svc", "nginx", 2);
    store.create_service(&spec).unwrap();

    store
        .record_container("my-svc-1", &spec.id, "nginx", 1, "Running", "10.0.0.2")
        .unwrap();
    store
        .record_container("my-svc-2", &spec.id, "nginx", 2, "Running", "10.0.0.3")
        .unwrap();
    store
        .record_container("my-svc-stopped", &spec.id, "nginx", 3, "Exited", "")
        .unwrap();

    let ips = store.get_active_container_ips().unwrap();
    let svc_ips = ips.get("my-svc").unwrap();
    assert_eq!(svc_ips.len(), 2);
    assert!(svc_ips.contains(&"10.0.0.2".to_string()));
}

// ── Autoscale Decision Logic ──

#[test]
fn test_autoscale_decisions() {
    let (store, _dir) = setup_store();
    let spec = make_spec("autoscale-test", "nginx", 2);
    store.create_service(&spec).unwrap();

    let _config = AutoscalingConfig {
        min_replicas: 1,
        max_replicas: 10,
        cpu_target_percent: Some(70.0),
        memory_target_percent: None,
        cooldown_seconds: 60,
    };

    // Record rising trend (predictive: should trigger scale up even below threshold)
    store
        .record_autoscale_metric(&spec.id, 45.0, 20.0, 2)
        .unwrap();
    store
        .record_autoscale_metric(&spec.id, 55.0, 25.0, 2)
        .unwrap();
    store
        .record_autoscale_metric(&spec.id, 65.0, 30.0, 2)
        .unwrap();

    let history = store.load_recent_metrics(&spec.id, 10).unwrap();
    let cpu_slope = linear_regression_slope(&history);

    // CPU is trending up (slope positive)
    assert!(cpu_slope > 0.0);
}

fn linear_regression_slope(history: &[(f64, f64)]) -> f64 {
    let n = history.len() as f64;
    if n < 3.0 {
        return 0.0;
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_x2 = 0.0;

    for (i, (cpu, _mem)) in history.iter().enumerate() {
        let x = i as f64;
        let y = *cpu;
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_x2 += x * x;
    }

    let denom = n * sum_x2 - sum_x * sum_x;
    if denom.abs() < 1e-10 {
        return 0.0;
    }

    (n * sum_xy - sum_x * sum_y) / denom
}
