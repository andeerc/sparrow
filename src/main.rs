use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use tokio::time::{sleep, Duration};
use tracing_subscriber::EnvFilter;

use sparrow_core::autoscale::AutoscaleEngine;
use sparrow_core::cli::{Cli, Command, UpdateAction, ServiceAction, ClusterAction, NodeAction, NetworkAction, AutoscaleAction, AlertAction, ConfigAction};
use sparrow_core::config::SparrowConfig;
use sparrow_core::state::StateStore;
use sparrow_podman::PodmanRuntime;
use sparrow_proto::*;
use sparrow_api::init_cluster;
use sparrow_mcp::start_mcp;

mod update;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();

    // Resolve config path
    let config_path: std::path::PathBuf = match cli.config.as_ref() {
        Some(p) => Path::new(p).to_path_buf(),
        None => SparrowConfig::default_path(),
    };

    // Load config (skip for `config init` — we create the file)
    let config = match &cli.command {
        Command::Config { action } => match action {
            ConfigAction::Init { .. } => SparrowConfig::default(),
            ConfigAction::Show { .. } => load_config_or_default(&config_path),
        },
        _ => load_config_or_default(&config_path),
    };

    // Use XDG data dir or fallback with permission check
    let data_dir = if config.cluster.data_dir.is_empty() {
        dirs::data_dir()
            .unwrap_or_else(|| Path::new("/var/lib").to_path_buf())
            .join("sparrow")
    } else {
        Path::new(&config.cluster.data_dir).to_path_buf()
    };

    // Create data dir, fall back to user-local if permission denied
    let data_dir = match std::fs::create_dir_all(&data_dir) {
        Ok(_) => data_dir,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let fallback = dirs::data_dir()
                .unwrap_or_else(|| Path::new("/tmp").to_path_buf())
                .join("sparrow");
            tracing::warn!(
                "Cannot write to {}, falling back to {}: {e}",
                data_dir.display(), fallback.display()
            );
            std::fs::create_dir_all(&fallback)?;
            fallback
        }
        Err(e) => return Err(anyhow::anyhow!(
            "Failed to create data dir {}: {e}", data_dir.display()
        )),
    };

    let db_path = data_dir.join("sparrow.db");
    let db_path_str = db_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("Data path is not valid UTF-8: {}", db_path.display())
    })?;
    let state = Arc::new(StateStore::new(db_path_str)?);

    let runtime = Arc::new(PodmanRuntime::new(config.runtime.rootless));

    // Check podman availability
    let podman_ok = match runtime.check_available().await {
        Ok(true) => { tracing::debug!("Podman available"); true }
        Ok(false) => { tracing::warn!("Podman not found"); false }
        Err(e) => { tracing::warn!("Podman check: {}", e); false }
    };

    if podman_ok {
        let hc_state = Arc::clone(&state);
        let hc_runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            health_check_loop(hc_state, hc_runtime).await;
        });

        // Start autoscaling engine
        let as_state = Arc::clone(&state);
        let as_runtime = Arc::clone(&runtime);
        let engine = AutoscaleEngine::new(as_state, as_runtime);
        tokio::spawn(async move {
            let _ = engine.start().await;
        });
    }

    let mut cluster_state: Option<sparrow_api::SharedAppState> = None;

    match &cli.command {
        Command::Config { action } => {
            handle_config(action, &config_path).await?;
            return Ok(());
        }
        _ => {}
    }

    match cli.command {
        Command::Cluster { action } => handle_cluster(action, &state, &config, &data_dir, &mut cluster_state).await?,

        // ── Service Commands ──
        Command::Service { action } => handle_service(action, &state, &runtime, &data_dir, podman_ok).await?,

        // ── Node Commands (Fase 2) ──
        Command::Node { action } => handle_node(action, &state, &cluster_state).await?,

        // ── Network Commands (Fase 2) ──
        Command::Network { action } => handle_network(action).await?,

        // ── Autoscale Commands (Fase 3) ──
        Command::Autoscale { action } => handle_autoscale(action, &state).await?,

        // ── Alert Commands (Fase 4) ──
        Command::Alert { action } => handle_alert(action, &state).await?,

        Command::Mcp { port, host } => {
            let cluster_name = &config.cluster.name;
            let app_state = cluster_state.clone().unwrap_or_else(|| {
                init_cluster(cluster_name, "localhost", &format!("{host}:{port}"), None)
            });
            let addr = format!("{host}:{port}");
            println!("🔌 Starting MCP server on {addr}...");
            start_mcp(app_state, state.clone(), runtime.clone(), &addr).await?;
        }

        // ── Deploy (Fase 3) ──
        Command::Deploy { file } => {
            match sparrow_core::deploy::DeployManifest::from_file(&file) {
                Ok(manifest) => {
                    let spec = manifest.to_service_spec();
                    println!("📦 Deploying '{}' ({} replicas of {})...", spec.name, spec.desired_replicas, spec.image);

                    if !podman_ok {
                        eprintln!("❌ Podman not available");
                        return Ok(());
                    }

                    // Persist to state store
                    state.create_service(&spec)?;
                    println!("📦 Service '{}' ({}) created", spec.name, spec.id);

                    // Run containers
                    let mut success = 0u32;
                    for i in 1..=spec.desired_replicas {
                        let cname = format!("{}-{}", spec.name, i);
                        let ports: Vec<PortMapping> = spec.ports.clone();
                        let env_refs: Vec<(String, String)> = spec.env.iter().map(|e| (e.key.clone(), e.value.clone())).collect();
                        match runtime.run_container(&cname, &spec.image, &ports, &env_refs, &std::collections::HashMap::new()).await {
                            Ok(cid) => {
                                state.record_container(&cname, &spec.id, &spec.image, i, "Running")?;
                                println!("  ✅ {cname} -> {cid:.12}");
                                success += 1;
                            }
                            Err(e) => {
                                state.record_container(&cname, &spec.id, &spec.image, i, "Failed")?;
                                eprintln!("  ❌ {cname}: {e}");
                            }
                        }
                    }

                    // Configure autoscale if specified
                    if let Some(as_config) = &spec.autoscaling {
                        state.set_autoscale(&spec.id, as_config, false)?;
                        println!("📊 Autoscale configured: min={} max={} cpu={}% mem={}%",
                            as_config.min_replicas, as_config.max_replicas,
                            as_config.cpu_target_percent.map_or("-".to_string(), |v| v.to_string()),
                            as_config.memory_target_percent.map_or("-".to_string(), |v| v.to_string()));
                    }

                    println!("🎯 Deploy complete: {}/{} replicas running", success, spec.desired_replicas);
                }
                Err(e) => eprintln!("❌ Deploy failed: {e}"),
            }
        }

        // ── Status ──
        Command::Status => {
            let services = state.list_services().unwrap_or_default();
            let podman_status = if podman_ok { "✓ available" } else { "✗ not found" };
            print_box("Sparrow Cluster Status", &[
                ("Mode",     "Single-node"),
                ("Podman",   podman_status),
                ("Data",     &data_dir.display().to_string()),
                ("Services", &services.len().to_string()),
                ("Raft",     "Stopped (single-node)"),
            ]);
        }

        // Config handled in early return above
        Command::Config { .. } => unreachable!(),

        // ── Update Commands (Fase 4) ──
        Command::Update { action } => handle_update(action).await?,
    }

    Ok(())
}

// ── Config Helpers ──

fn load_config_or_default(path: &Path) -> SparrowConfig {
    if path.exists() {
        SparrowConfig::load(path).unwrap_or_else(|e| {
            eprintln!("⚠ Failed to parse config: {e}. Using defaults.");
            SparrowConfig::default()
        })
    } else {
        tracing::info!("No config file at {}, using defaults", path.display());
        SparrowConfig::default()
    }
}

// ── Config Handler ──

async fn handle_config(action: &ConfigAction, config_path: &Path) -> anyhow::Result<()> {
    match action {
        ConfigAction::Init {
            path,
            cluster_name,
            listen,
            raft_port,
            data_dir,
            runtime_backend,
            rootless,
            podman_socket,
            log_level,
            log_format,
            log_file,
            api_listen,
        } => {
            let out_path = path.as_ref().map(Path::new).unwrap_or(config_path);

            if out_path.exists() {
                eprint!("⚠ Config already exists at {}. Overwrite? [y/N] ", out_path.display());
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("✖ Aborted.");
                    return Ok(());
                }
            }

            let mut cfg = SparrowConfig::default();

            if let Some(v) = cluster_name {
                cfg.cluster.name = v.clone();
            }
            if let Some(v) = listen {
                cfg.cluster.listen = v.clone();
            }
            if let Some(v) = raft_port {
                cfg.cluster.raft_port = Some(*v);
            }
            if let Some(v) = data_dir {
                cfg.cluster.data_dir = v.clone();
            }
            if let Some(v) = runtime_backend {
                cfg.runtime.backend = v.clone();
            }
            if let Some(v) = rootless {
                cfg.runtime.rootless = *v;
            }
            if let Some(v) = podman_socket {
                cfg.runtime.podman_socket = v.clone();
            }
            if let Some(v) = log_level {
                cfg.logging.level = v.clone();
            }
            if let Some(v) = log_format {
                cfg.logging.format = v.clone();
            }
            if let Some(v) = log_file {
                cfg.logging.file = Some(v.clone());
            }
            if let Some(v) = api_listen {
                cfg.api.listen = v.clone();
            }

            cfg.save(out_path)?;
            println!("✅ Config written to {}", out_path.display());
            println!("{}", cfg.to_yaml()?);
        }

        ConfigAction::Show { path, yaml } => {
            let show_path = path.as_ref().map(Path::new).unwrap_or(config_path);
            let cfg = if show_path.exists() {
                SparrowConfig::load(show_path)?
            } else {
                eprintln!("⚠ No config at {}, showing defaults", show_path.display());
                SparrowConfig::default()
            };

            if *yaml {
                println!("{}", cfg.to_yaml()?);
            } else {
                print_box("Sparrow Configuration", &[
                    ("Config path", &show_path.display().to_string()),
                    ("Cluster name", &cfg.cluster.name),
                    ("Listen", &cfg.cluster.listen),
                    ("Raft port", &cfg.cluster.raft_port.map_or("none".to_string(), |p| p.to_string())),
                    ("Data dir", &cfg.cluster.data_dir),
                    ("Runtime", &cfg.runtime.backend),
                    ("Rootless", if cfg.runtime.rootless { "yes" } else { "no" }),
                    ("Socket", if cfg.runtime.podman_socket.is_empty() { "default" } else { &cfg.runtime.podman_socket }),
                    ("Log level", &cfg.logging.level),
                    ("Log format", &cfg.logging.format),
                    ("Log file", cfg.logging.file.as_deref().unwrap_or("stdout")),
                    ("API listen", &cfg.api.listen),
                ]);
            }
        }
    }
    Ok(())
}

// ── Cluster Handler ──

async fn handle_cluster(
    action: ClusterAction,
    _state: &StateStore,
    config: &SparrowConfig,
    data_dir: &Path,
    cluster_state: &mut Option<sparrow_api::SharedAppState>,
) -> anyhow::Result<()> {
    let raft_data_dir = data_dir.display().to_string();
    match action {
        ClusterAction::Init { name, listen } => {
            let addr = if listen.is_empty() { "0.0.0.0:7443" } else { &listen };
            println!("🔧 Initializing cluster '{name}' on {addr}...");

            let raft_cluster = match (&config.cluster.tls_ca, &config.cluster.tls_cert, &config.cluster.tls_key) {
                (Some(ca), Some(cert), Some(key)) => {
                    let tls = sparrow_raft::TlsConfig {
                        ca: std::fs::read(ca)?,
                        cert: std::fs::read(cert)?,
                        key: std::fs::read(key)?,
                    };
                    std::sync::Arc::new(sparrow_raft::RaftCluster::with_tls(1, addr, tls, &raft_data_dir))
                }
                _ => {
                    std::sync::Arc::new(sparrow_raft::RaftCluster::new(1, addr, &raft_data_dir))
                }
            };
            raft_cluster.init().await?;

            let app_state = sparrow_api::init_cluster(&name, "localhost", addr, Some(raft_cluster));
            let cs_clone = app_state.clone();
            let listen_addr = addr.to_string();
            let tls_cert = config.cluster.tls_cert.clone();
            let tls_key = config.cluster.tls_key.clone();
            tokio::spawn(async move {
                if let Err(e) = sparrow_api::start_api(cs_clone, &listen_addr, tls_cert.as_deref(), tls_key.as_deref()).await {
                    tracing::error!("API server failed: {e}");
                }
            });

            let proxy_state = app_state.clone();
            tokio::spawn(async move {
                if let Err(e) = sparrow_api::start_proxy(proxy_state, 7444).await {
                    tracing::error!("Proxy server failed: {e}");
                }
            });

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            println!("✅ Cluster '{name}' initialized. API on {addr}, proxy on 7444");
            println!("   Join token: sparrow-{}-tok-{:x}", name, name.len());
            *cluster_state = Some(app_state);
        }
        ClusterAction::Join { addr, token } => {
            println!("🔗 Joining cluster at {addr} with token {token}...");
            let raft_cluster = std::sync::Arc::new(sparrow_raft::RaftCluster::new(2, "0.0.0.0:7443", &raft_data_dir));
            raft_cluster.join(&addr).await?;
            let app_state = sparrow_api::init_cluster("default", "localhost", "0.0.0.0:7443", Some(raft_cluster));
            *cluster_state = Some(app_state);
            println!("✅ Joined cluster at {addr}");
        }
        ClusterAction::Status => {
            match cluster_state {
                Some(cs) => {
                    let cluster = cs.cluster.read().await;
                    let cluster_name = cluster.name.clone();
                    let nodes: Vec<_> = cluster.nodes.values().map(|n| {
                        format!("   {} @ {} — {} ({})", n.name, n.addr, n.role, n.status)
                    }).collect();
                    drop(cluster);

                    let raft_leader = {
                        let ra = cs.raft_cluster.read().await;
                        match ra.as_ref() {
                            Some(rc) => rc.current_leader().await,
                            None => None,
                        }
                    };

                    println!("📊 Cluster: '{cluster_name}' ({} nodes)", nodes.len());
                    for line in &nodes {
                        println!("{line}");
                    }
                    if let Some(lid) = raft_leader {
                        println!("   Raft leader: node {lid}");
                    }
                }
                None => {
                    println!("📊 Cluster: single-node mode (no peers)");
                    println!("   To form a multi-node cluster, use: sparrow cluster init");
                }
            }
        }
        ClusterAction::Members => {
            match cluster_state {
                Some(cs) => {
                    let cluster = cs.cluster.read().await;
                    println!("📋 Cluster members:");
                    for node in cluster.nodes.values() {
                        println!("   {} ({}) — {} — {}", node.id, node.name, node.role, node.status);
                    }
                }
                None => println!("📋 Cluster members:\n   localhost (self) — Leader"),
            }
        }
    }
    Ok(())
}

// ── Service Handler ──

async fn handle_service(
    action: ServiceAction,
    state: &StateStore,
    runtime: &PodmanRuntime,
    _data_dir: &Path,
    podman_ok: bool,
) -> anyhow::Result<()> {
    match action {
        ServiceAction::Create { name, image, replicas, port, env, volume: _, network: _, restart: _, domain: _, autoscale: _ } => {
            if !podman_ok {
                eprintln!("❌ Podman not available");
                std::process::exit(1);
            }

            // Parse ports
            let ports: Vec<PortMapping> = port.iter().filter_map(|p| {
                let parts: Vec<&str> = p.split(':').collect();
                if parts.len() == 2 {
                    Some(PortMapping { published: parts[0].parse().unwrap_or(80), target: parts[1].parse().unwrap_or(80), protocol: Protocol::Tcp })
                } else { None }
            }).collect();

            // Parse env
            let env_vars: Vec<EnvVar> = env.iter().filter_map(|e| {
                let mut parts = e.splitn(2, '=');
                match (parts.next(), parts.next()) {
                    (Some(k), Some(v)) => Some(EnvVar { key: k.to_string(), value: v.to_string() }),
                    _ => None,
                }
            }).collect();

            // Create service spec
            let mut spec = ServiceSpec::new(&name, &image);
            spec.desired_replicas = replicas;
            spec.ports = ports;
            spec.env = env_vars;

            // Persist to state store
            state.create_service(&spec)?;
            println!("📦 Service '{}' ({}) created", name, spec.id);

            // Run containers
            let mut success = 0u32;
            for i in 1..=replicas {
                let container_name = format!("{}-{}", name, i);
                let port_refs: Vec<PortMapping> = spec.ports.clone();
                let env_refs: Vec<(String, String)> = spec.env.iter().map(|e| (e.key.clone(), e.value.clone())).collect();
                let labels = std::collections::HashMap::new();

                match runtime.run_container(&container_name, &image, &port_refs, &env_refs, &labels).await {
                    Ok(cid) => {
                        state.record_container(&container_name, &spec.id, &image, i, "Running")?;
                        println!("  ✅ {container_name} -> {cid:.12}");
                        success += 1;
                    }
                    Err(e) => {
                        state.record_container(&container_name, &spec.id, &image, i, "Failed")?;
                        eprintln!("  ❌ {container_name}: {e}");
                    }
                }
            }

            println!("🎯 Service '{name}' created with {success}/{replicas} replicas");
        }

        ServiceAction::List => {
            let services = state.list_services()?;
            if services.is_empty() {
                println!("📭 No services. Create one: sparrow service create --name myapp --image nginx");
                return Ok(());
            }

            print_table(&["NAME", "IMAGE", "REPLICAS", "CREATED"], services.iter().map(|s| {
                vec![
                    s.name.clone(),
                    s.image.clone(),
                    s.desired_replicas.to_string(),
                    s.created_at.format("%Y-%m-%d %H:%M").to_string(),
                ]
            }).collect());
        }

        ServiceAction::Ps { name } => {
            // Resolve service
            let svc = state.get_service(&name)?;
            match svc {
                Some(s) => {
                    let containers = runtime.list_containers(&s.name).await?;
                    if containers.is_empty() {
                        println!("📭 No containers for service '{}'", s.name);
                    } else {
                        let mut rows: Vec<Vec<String>> = Vec::new();
                        for c in &containers {
                            let (cpu, mem) = if c.state == ContainerState::Running {
                                runtime.stats(&c.name).await.unwrap_or((0.0, 0))
                            } else { (0.0, 0) };
                            rows.push(vec![
                                c.name.clone(),
                                c.state.to_string(),
                                format_cpu(cpu),
                                if mem > 0 { format_bytes(mem) } else { "-".to_string() },
                            ]);
                        }
                        print_table(&["CONTAINER", "STATUS", "CPU", "MEM"], rows);
                    }
                }
                None => eprintln!("❌ Service '{name}' not found"),
            }
        }

        ServiceAction::Inspect { name } => {
            match runtime.inspect_container(&format!("{name}-1")).await {
                Ok(status) => {
                    println!("{:#?}", status);

                    // Show ports from state
                    if let Ok(Some(svc)) = state.get_service(&name) {
                        if !svc.ports.is_empty() {
                            println!("\nPorts:");
                            for p in &svc.ports {
                                println!("  {}:{} -> {}", match p.protocol { Protocol::Tcp => "tcp", Protocol::Udp => "udp" }, p.published, p.target);
                            }
                        }
                    }
                }
                Err(e) => eprintln!("❌ Inspect failed: {e}"),
            }
        }

        ServiceAction::Scale { name, replicas } => {
            if !podman_ok { eprintln!("❌ Podman unavailable"); return Ok(()); }

            let svc = match state.get_service(&name)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{name}' not found"); return Ok(()); }
            };

            let current = runtime.list_containers(&svc.name).await?.len() as u32;

            if replicas > current {
                // Scale up
                for i in (current + 1)..=replicas {
                    let container_name = format!("{}-{}", svc.name, i);
                    let port_refs: Vec<PortMapping> = svc.ports.clone();
                    let labels = std::collections::HashMap::new();

                    match runtime.run_container(&container_name, &svc.image, &port_refs, &[], &labels).await {
                        Ok(cid) => {
                            state.record_container(&container_name, &svc.id, &svc.image, i, "Running")?;
                            println!("  ✅ {container_name} -> {cid:.12}");
                        }
                        Err(e) => {
                            state.record_container(&container_name, &svc.id, &svc.image, i, "Failed")?;
                            eprintln!("  ❌ {container_name}: {e}");
                        }
                    }
                }
            } else {
                // Scale down - remove last containers
                for i in (replicas + 1)..=current {
                    let container_name = format!("{}-{}", svc.name, i);
                    match runtime.remove_container(&container_name).await {
                        Ok(_) => {
                            state.update_container_state(&container_name, "Removed")?;
                            println!("  ✅ {container_name} removed");
                        }
                        Err(e) => eprintln!("  ❌ {container_name}: {e}"),
                    }
                }
            }

            state.update_replicas(&svc.id, replicas)?;
            println!("🎯 Service '{}' scaled to {} replicas", svc.name, replicas);
        }

        ServiceAction::Rm { name } => {
            if !podman_ok { eprintln!("❌ Podman unavailable"); return Ok(()); }

            let svc = match state.get_service(&name)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{name}' not found"); return Ok(()); }
            };

            let containers = runtime.list_containers(&svc.name).await?;
            let mut removed = 0u32;
            for c in &containers {
                match runtime.remove_container(&c.name).await {
                    Ok(_) => {
                        state.update_container_state(&c.name, "Removed")?;
                        println!("  ✅ {} removed", c.name);
                        removed += 1;
                    }
                    Err(e) => eprintln!("  ❌ {}: {e}", c.name),
                }
            }

            state.delete_service(&svc.id)?;
            println!("📦 Service '{}' removed ({removed} containers)", svc.name);
        }

        ServiceAction::Logs { name, tail, follow } => {
            if !podman_ok { eprintln!("❌ Podman unavailable"); return Ok(()); }

            let svc = match state.get_service(&name)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{name}' not found"); return Ok(()); }
            };

            let containers = runtime.list_containers(&svc.name).await?;
            if containers.is_empty() {
                println!("📭 No containers for service '{}'", name);
                return Ok(());
            }

            if follow {
                let mut children = Vec::new();
                for c in &containers {
                    if c.state != ContainerState::Running { continue; }
                    let prefix = if containers.len() > 1 { format!("[{}] ", c.name) } else { String::new() };
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                    match runtime.logs_follow(&c.name, tail, tx).await {
                        Ok(child) => {
                            children.push(child);
                            tokio::spawn(async move {
                                while let Some(line) = rx.recv().await {
                                    println!("{prefix}{line}");
                                }
                            });
                        }
                        Err(e) => eprintln!("{prefix}Error: {e}"),
                    }
                }

                if children.is_empty() {
                    return Ok(());
                }

                let _ = tokio::signal::ctrl_c().await;
                for mut child in children {
                    let _ = child.start_kill();
                }
            } else {
                for c in &containers {
                    if c.state != ContainerState::Running { continue; }
                    let prefix = if containers.len() > 1 { format!("[{}] ", c.name) } else { String::new() };
                    match runtime.logs(&c.name, tail).await {
                        Ok(lines) => {
                            for line in &lines {
                                println!("{prefix}{line}");
                            }
                        }
                        Err(e) => eprintln!("{prefix}Error: {e}"),
                    }
                }
            }
        }

        ServiceAction::Update { name, image, parallelism, delay } => {
            println!("🔄 Updating '{name}' (par={parallelism}, delay={delay})...");
            let svc = match state.get_service(&name)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{name}' not found"); return Ok(()); }
            };

            let new_image = match image {
                Some(ref img) => img.clone(),
                None => { eprintln!("❌ No --image specified for update"); return Ok(()); }
            };

            // Simple rolling update: create new, remove old, one by one
            let containers = runtime.list_containers(&svc.name).await?;
            for (i, c) in containers.iter().enumerate() {
                let new_name = format!("{}-new-{}", svc.name, i + 1);
                let port_refs: Vec<PortMapping> = svc.ports.clone();
                let labels = std::collections::HashMap::new();

                println!("  🚀 Deploying {new_name} ({new_image})...");
                match runtime.run_container(&new_name, &new_image, &port_refs, &[], &labels).await {
                    Ok(cid) => {
                        println!("    ✅ {new_name} -> {cid:.12} (waiting for {delay})");
                        state.record_container(&new_name, &svc.id, &new_image, (containers.len() + i + 1) as u32, "Running")?;

                        // Remove old
                        match runtime.remove_container(&c.name).await {
                            Ok(_) => println!("    ✅ {} removed", c.name),
                            Err(e) => eprintln!("    ❌ {}: {e}", c.name),
                        }
                    }
                    Err(e) => eprintln!("    ❌ {new_name}: {e}"),
                }
            }

            println!("🎯 Update complete for '{name}'");
        }
    }
    Ok(())
}

// ── Node Handler (Fase 2) ──

async fn handle_node(action: NodeAction, state: &StateStore, cluster_state: &Option<sparrow_api::SharedAppState>) -> anyhow::Result<()> {
    match action {
        NodeAction::List => {
            let nodes = if let Some(app_state) = cluster_state {
                let cluster = app_state.cluster.read().await;
                cluster.nodes.keys().cloned().collect::<Vec<_>>()
            } else {
                vec![]
            };
            if nodes.is_empty() {
                println!("📋 Nodes:\n  localhost (self) - Ready");
            } else {
                println!("📋 Nodes:");
                for node in &nodes {
                    println!("  {node} - Ready");
                }
            }
        }
        NodeAction::Inspect { name } => {
            let (in_cluster, _node_list) = if let Some(app_state) = cluster_state {
                let cluster = app_state.cluster.read().await;
                (cluster.nodes.contains_key(&name), cluster.nodes.keys().cloned().collect::<Vec<_>>())
            } else {
                (false, vec![])
            };
            if in_cluster || name == "localhost" || name == "self" {
                println!("📋 Node '{}':", name);
                let services = state.list_services()?;
                let total: usize = services.iter().map(|s| {
                    state.get_service_containers(&s.id).map(|cs| {
                        cs.iter().filter(|c| c.node_id == name || name == "localhost" || name == "self").count()
                    }).unwrap_or(0)
                }).sum();
                println!("  Containers running: {total}");
                println!("  Status: Ready");
            } else {
                eprintln!("❌ Node '{name}' not found in cluster");
            }
        }
        NodeAction::Drain { name } => {
            if let Some(app_state) = cluster_state {
                let in_cluster = {
                    let cluster = app_state.cluster.read().await;
                    cluster.nodes.contains_key(&name)
                };
                if in_cluster {
                    let services = state.list_services()?;
                    for svc in &services {
                        let containers = state.get_service_containers(&svc.id)?;
                        for c in containers.iter().filter(|c| c.node_id == name) {
                            state.update_container_state(&c.name, "Drained")?;
                            println!("  Drained {}/{}", svc.name, c.name);
                        }
                    }
                    println!("✅ Node '{name}' drained");
                } else {
                    eprintln!("❌ Node '{name}' not found in cluster");
                }
            } else {
                eprintln!("❌ Drain requires multi-node cluster (init with `sparrow cluster init`)");
            }
        }
        NodeAction::Rm { name } => {
            if let Some(app_state) = cluster_state {
                let mut cluster = app_state.cluster.write().await;
                if cluster.nodes.contains_key(&name) {
                    cluster.nodes.remove(&name);
                    println!("✅ Node '{name}' removed from cluster");
                } else if name == "localhost" || name == "self" {
                    eprintln!("❌ Cannot remove self node");
                } else {
                    eprintln!("❌ Node '{name}' not found in cluster");
                }
            } else {
                eprintln!("❌ Remove requires multi-node cluster (init with `sparrow cluster init`)");
            }
        }
    }
    Ok(())
}

// ── Network Handler (Fase 2) ──

async fn handle_network(action: NetworkAction) -> anyhow::Result<()> {
    match action {
        NetworkAction::Create { name, subnet } => {
            let subnet_str = subnet.as_deref().unwrap_or("10.88.0.0/16");
            println!("🌐 Creating network '{name}' ({subnet_str})...");
            let out = tokio::process::Command::new("podman")
                .args(["network", "create", "--subnet", subnet_str, &name])
                .output().await?;
            if out.status.success() {
                println!("  ✅ Network '{name}' created");
            } else {
                eprintln!("  ❌ {}", String::from_utf8_lossy(&out.stderr));
            }
        }
        NetworkAction::List => {
            let out = tokio::process::Command::new("podman")
                .args(["network", "ls", "--format", "{{.Name}}\t{{.Driver}}\t{{.Subnet}}"])
                .output().await?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let rows: Vec<Vec<String>> = stdout.lines().filter(|l| !l.is_empty()).map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                vec![
                    parts.first().unwrap_or(&"").to_string(),
                    parts.get(1).unwrap_or(&"").to_string(),
                    parts.get(2).unwrap_or(&"").to_string(),
                ]
            }).collect();
            if !rows.is_empty() {
                print_table(&["NAME", "DRIVER", "SUBNET"], rows);
            }
        }
        NetworkAction::Rm { name } => {
            let out = tokio::process::Command::new("podman")
                .args(["network", "rm", &name])
                .output().await?;
            if out.status.success() {
                println!("  ✅ Network '{name}' removed");
            } else {
                eprintln!("  ❌ {}", String::from_utf8_lossy(&out.stderr));
            }
        }
    }
    Ok(())
}

// ── Autoscale Handler (Fase 3) ──

async fn handle_autoscale(action: AutoscaleAction, state: &StateStore) -> anyhow::Result<()> {
    match action {
        AutoscaleAction::Set { service, min, max, cpu_target, mem_target, cooldown } => {
            let svc = match state.get_service(&service)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{service}' not found"); return Ok(()); }
            };

            let config = AutoscalingConfig {
                min_replicas: min.unwrap_or(1),
                max_replicas: max.unwrap_or(10),
                cpu_target_percent: cpu_target,
                memory_target_percent: mem_target,
                cooldown_seconds: cooldown.unwrap_or(60),
            };
            state.set_autoscale(&svc.id, &config, false)?;
            println!("📊 Autoscale configured for '{}'", svc.name);
            println!("   Min: {}  Max: {}  CPU: {}%  Mem: {}%  Cooldown: {}s",
                config.min_replicas, config.max_replicas,
                config.cpu_target_percent.map_or("-".to_string(), |v| v.to_string()),
                config.memory_target_percent.map_or("-".to_string(), |v| v.to_string()),
                config.cooldown_seconds);
        }
        AutoscaleAction::Status { service } => {
            let svc = match state.get_service(&service)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{service}' not found"); return Ok(()); }
            };
            match state.get_autoscale(&svc.id)? {
                Some((config, paused)) => {
                    println!("📊 Autoscale for '{}'", svc.name);
                    println!("   Status:   {}", if paused { "⏸ Paused" } else { "▶ Active" });
                    println!("   Min: {}  Max: {}", config.min_replicas, config.max_replicas);
                    println!("   CPU target: {}%  Mem target: {}%", config.cpu_target_percent.map_or("-".to_string(), |v| v.to_string()), config.memory_target_percent.map_or("-".to_string(), |v| v.to_string()));
                    println!("   Cooldown: {}s", config.cooldown_seconds);
                }
                None => println!("📭 No autoscale config for '{}'", svc.name),
            }
        }
        AutoscaleAction::History { service, last } => {
            let svc = match state.get_service(&service)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{service}' not found"); return Ok(()); }
            };
            let limit: u32 = last.parse().unwrap_or(10);
            let events = state.list_autoscale_events(&svc.id, limit)?;
            if events.is_empty() {
                println!("📭 No autoscale events for '{}'", svc.name);
            } else {
                let rows: Vec<Vec<String>> = events.iter().map(|e| {
                    vec![
                        e.created_at.clone(),
                        e.decision.clone(),
                        format!("{} → {}", e.replicas_from, e.replicas_to),
                        e.reason.clone(),
                    ]
                }).collect();
                print_table(&["TIME", "DECISION", "REPLICAS", "REASON"], rows);
            }
        }
        AutoscaleAction::Pause { service } => {
            let svc = match state.get_service(&service)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{service}' not found"); return Ok(()); }
            };
            if let Some((config, _)) = state.get_autoscale(&svc.id)? {
                state.set_autoscale(&svc.id, &config, true)?;
                println!("⏸ Autoscale paused for '{}'", svc.name);
            } else {
                eprintln!("❌ No autoscale config for '{}'", svc.name);
            }
        }
        AutoscaleAction::Resume { service } => {
            let svc = match state.get_service(&service)? {
                Some(s) => s,
                None => { eprintln!("❌ Service '{service}' not found"); return Ok(()); }
            };
            if let Some((config, _)) = state.get_autoscale(&svc.id)? {
                state.set_autoscale(&svc.id, &config, false)?;
                println!("▶ Autoscale resumed for '{}'", svc.name);
            } else {
                eprintln!("❌ No autoscale config for '{}'", svc.name);
            }
        }
    }
    Ok(())
}

// ── Alert Handler (Fase 4) ──

async fn handle_alert(action: AlertAction, state: &StateStore) -> anyhow::Result<()> {
    match action {
        AlertAction::Set { id, channel_type, name, bot_token, chat_id, smtp_host, smtp_port, smtp_username, smtp_password, from, to } => {
            let config = match channel_type.as_str() {
                "telegram" => {
                    let bot_token = bot_token.ok_or_else(|| anyhow::anyhow!("--bot_token required for telegram"))?;
                    let chat_id = chat_id.ok_or_else(|| anyhow::anyhow!("--chat_id required for telegram"))?;
                    serde_json::json!({ "bot_token": bot_token, "chat_id": chat_id })
                }
                "smtp" | "email" => {
                    let smtp_host = smtp_host.ok_or_else(|| anyhow::anyhow!("--smtp_host required for smtp"))?;
                    let smtp_username = smtp_username.ok_or_else(|| anyhow::anyhow!("--smtp_username required for smtp"))?;
                    let smtp_password = smtp_password.ok_or_else(|| anyhow::anyhow!("--smtp_password required for smtp"))?;
                    let from = from.ok_or_else(|| anyhow::anyhow!("--from required for smtp"))?;
                    let to = to.ok_or_else(|| anyhow::anyhow!("--to required for smtp"))?;
                    serde_json::json!({
                        "smtp_host": smtp_host,
                        "smtp_port": smtp_port.unwrap_or(587),
                        "username": smtp_username,
                        "password": smtp_password,
                        "from": from,
                        "to": to,
                    })
                }
                other => anyhow::bail!("unsupported channel type: {other} (use telegram or smtp)"),
            };
            state.record_alert_channel(&id, &channel_type, &name, &config.to_string(), true)?;
            println!("✅ Alert channel '{}' ({}) saved", id, channel_type);
        }
        AlertAction::List => {
            let rules = state.list_alert_rules()?;
            if rules.is_empty() {
                println!("📭 No alert rules configured");
            } else {
                let rows: Vec<Vec<String>> = rules.iter().map(|r| {
                    vec![
                        r.id.clone(),
                        r.name.clone(),
                        r.metric.clone(),
                        format!("{} {}", r.operator, r.threshold),
                        format!("{}s", r.duration_secs),
                        if r.enabled { "yes" } else { "no" }.to_string(),
                    ]
                }).collect();
                print_table(&["ID", "NAME", "METRIC", "CONDITION", "DURATION", "ENABLED"], rows);
            }
        }
        AlertAction::History => {
            let events = state.list_alert_events(50)?;
            if events.is_empty() {
                println!("📭 No alert events recorded");
            } else {
                let rows: Vec<Vec<String>> = events.iter().map(|e| {
                    vec![
                        e.created_at.clone(),
                        e.severity.clone(),
                        e.channel_type.clone(),
                        e.message.clone(),
                    ]
                }).collect();
                print_table(&["TIME", "SEVERITY", "CHANNEL", "MESSAGE"], rows);
            }
        }
    }
    Ok(())
}

// ── Update Handler (Fase 4) ──

async fn handle_update(action: UpdateAction) -> anyhow::Result<()> {
    match action {
        UpdateAction::Check => {
            println!("🔍 Checking for updates...");
            match update::check().await {
                Ok(Some(info)) => {
                    println!("📦 Update available:");
                    println!("   Current:  {}", info.current_tag);
                    println!("   Latest:   {}", info.latest_tag);
                    println!("   Size:     {} bytes", info.download_size);
                    println!("   URL:      {}", info.html_url);
                    println!("\nRun `sparrow update install` to upgrade.");
                }
                Ok(None) => {
                    println!("✅ You are running the latest version ({}).", env!("CARGO_PKG_VERSION"));
                }
                Err(e) => {
                    eprintln!("❌ Update check failed: {e}");
                }
            }
        }
        UpdateAction::Install => {
            println!("🔍 Checking for latest version...");
            match update::check().await {
                Ok(Some(info)) => {
                    println!("📦 Installing {} -> {} ...", info.current_tag, info.latest_tag);
                    if let Err(e) = update::install(&info.download_url).await {
                        eprintln!("❌ Update failed: {e}");
                    }
                }
                Ok(None) => {
                    println!("✅ Already up-to-date ({}).", env!("CARGO_PKG_VERSION"));
                }
                Err(e) => {
                    eprintln!("❌ Update check failed: {e}");
                }
            }
        }
    }
    Ok(())
}

// ── Helpers ──

async fn health_check_loop(state: Arc<StateStore>, runtime: Arc<PodmanRuntime>) {
    loop {
        sleep(Duration::from_secs(30)).await;
        let services = match state.list_services() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Health check: failed to list services: {e}");
                continue;
            }
        };

        for svc in &services {
            let containers = match runtime.list_containers(&svc.name).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Check if we have the right number of running containers
            let running = containers.iter().filter(|c| c.state == ContainerState::Running).count();
            let desired = svc.desired_replicas as usize;

            // Restart failed containers
            for c in &containers {
                if c.state == ContainerState::Exited || c.state == ContainerState::Crashed {
                    tracing::warn!("Health: {} is {:?}, restarting...", c.name, c.state);
                    if let Err(e) = runtime.remove_container(&c.name).await {
                        tracing::error!("Health: failed to remove {}: {e}", c.name);
                        continue;
                    }
                    let port_refs: Vec<PortMapping> = svc.ports.clone();
                    match runtime.run_container(&c.name, &svc.image, &port_refs, &[], &std::collections::HashMap::new()).await {
                        Ok(cid) => {
                            if let Err(e) = state.record_container(&c.name, &svc.id, &svc.image, containers.len() as u32 + 1, "Running") {
                                tracing::error!("Health: failed to record {}: {e}", c.name);
                            }
                            tracing::info!("Health: restarted {} -> {:.12}", c.name, cid);
                        }
                        Err(e) => {
                            tracing::error!("Health: failed to restart {}: {e}", c.name);
                        }
                    }
                }
            }

            // Scale up if missing replicas
            if running < desired {
                tracing::warn!("Health: {} has {}/{} replicas, scaling up...", svc.name, running, desired);
                for i in (running + 1)..=desired {
                    let cname = format!("{}-{}", svc.name, i);
                    let port_refs: Vec<PortMapping> = svc.ports.clone();
                    match runtime.run_container(&cname, &svc.image, &port_refs, &[], &std::collections::HashMap::new()).await {
                        Ok(cid) => {
                            let _ = state.record_container(&cname, &svc.id, &svc.image, i as u32, "Running");
                            tracing::info!("Health: scaled up {} -> {:.12}", cname, cid);
                        }
                        Err(e) => tracing::error!("Health: scale up failed {}: {e}", cname),
                    }
                }
            }
        }
    }
}

fn format_cpu(cpu: f64) -> String {
    if cpu == 0.0 { "-".to_string() } else { format!("{:.1}%", cpu) }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    for unit in UNITS {
        if v < 1024.0 { return format!("{:.1}{}", v, unit); }
        v /= 1024.0;
    }
    format!("{:.1}TiB", v * 1024.0)
}

fn print_box(title: &str, rows: &[(&str, &str)]) {
    let label_w = rows.iter().map(|r| r.0.len()).max().unwrap_or(8);
    let mut min_w = title.len() + 4;
    for (l, v) in rows {
        let line_w = l.len() + 2 + v.len();
        if line_w > min_w { min_w = line_w; }
    }
    let w = min_w.max(40).min(72);
    let val_w = w.saturating_sub(label_w + 4); // 4 = "│ " (2) + "  " (2) before val; trailing " │" (2) included in w+2

    println!("┌{}┐", "─".repeat(w));
    println!("│{:^w$}│", format!(" {} ", title));
    println!("├{}┤", "─".repeat(w));
    for (label, val) in rows {
        println!("│ {:>label_w$}  {:<val_w$} │", label, val);
    }
    println!("└{}┘", "─".repeat(w));
}

fn print_table(headers: &[&str], rows: Vec<Vec<String>>) {
    let n = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(n) {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let render = |cells: &[String], align: fn(usize) -> char| -> String {
        cells.iter().enumerate().map(|(i, c)| {
            let w = widths[i];
            if align(i) == '^' { format!(" {:^w$} ", c) }
            else { format!(" {:<w$} ", c) }
        }).collect::<Vec<_>>().join("│")
    };
    let top = format!("┌{}┐", widths.iter().map(|w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┬"));
    let sep = format!("├{}┤", widths.iter().map(|w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┼"));
    let bot = format!("└{}┘", widths.iter().map(|w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┴"));
    let hdr: Vec<String> = headers.iter().map(|h| h.to_string()).collect();

    println!("{top}");
    println!("│{}│", render(&hdr, |_| '^'));
    println!("{sep}");
    for row in &rows {
        println!("│{}│", render(row, |_| '<'));
    }
    println!("{bot}");
}

