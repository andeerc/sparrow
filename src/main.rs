use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use tokio::time::{sleep, Duration};
use tracing_subscriber::EnvFilter;

use sparrow_core::cli::{Cli, Command, ServiceAction, ClusterAction, NodeAction, NetworkAction, AutoscaleAction, AlertAction};
use sparrow_core::config::SparrowConfig;
use sparrow_core::state::StateStore;
use sparrow_podman::PodmanRuntime;
use sparrow_proto::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();

    // Load config
    let config_path = Path::new("/etc/sparrow/sparrow.yaml");
    let config = if config_path.exists() {
        SparrowConfig::load(config_path)?
    } else {
        tracing::info!("No config file at {}, using defaults", config_path.display());
        SparrowConfig::default()
    };

    // Use XDG data dir or fallback
    let data_dir = if config.cluster.data_dir.is_empty() {
        dirs::data_dir()
            .unwrap_or_else(|| Path::new("/var/lib").to_path_buf())
            .join("sparrow")
    } else {
        Path::new(&config.cluster.data_dir).to_path_buf()
    };
    std::fs::create_dir_all(&data_dir)?;

    let db_path = data_dir.join("sparrow.db");
    let state = Arc::new(StateStore::new(db_path.to_str().unwrap())?);

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
    }

    let mut cluster_state: Option<sparrow_api::SharedAppState> = None;

    match cli.command {
        Command::Cluster { action } => handle_cluster(action, &state, &config, &mut cluster_state).await?,

        // ── Service Commands ──
        Command::Service { action } => handle_service(action, &state, &runtime, &data_dir, podman_ok).await?,

        // ── Node Commands (Fase 2) ──
        Command::Node { action } => handle_node(action).await?,

        // ── Network Commands (Fase 2) ──
        Command::Network { action } => handle_network(action).await?,

        // ── Autoscale Commands (Fase 3) ──
        Command::Autoscale { action } => handle_autoscale(action, &state).await?,

        // ── Alert Commands (Fase 4) ──
        Command::Alert { action } => handle_alert(action).await?,

        // ── MCP Server (Fase 4) ──
        Command::Mcp { port, host } => {
            println!("⚠️  MCP server ({host}:{port}) — Fase 4, not yet implemented");
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
            println!("╔══════════════════════════════════════╗");
            println!("║     Sparrow Cluster Status          ║");
            println!("╠══════════════════════════════════════╣");
            println!("║  Mode:       Single-node             ║");
            println!("║  Podman:     {}                    ║", if podman_ok { "✓" } else { "✗" });
            println!("║  Data:       {}  ║", data_dir.display());
            println!("║  Services:   {}                     ║", services.len());
            println!("║  Raft:       Stopped (Fase 2)        ║");
            println!("╚══════════════════════════════════════╝");
        }
    }

    Ok(())
}

// ── Cluster Handler ──

async fn handle_cluster(
    action: ClusterAction,
    _state: &StateStore,
    _config: &SparrowConfig,
    cluster_state: &mut Option<sparrow_api::SharedAppState>,
) -> anyhow::Result<()> {
    match action {
        ClusterAction::Init { name, listen } => {
            let addr = if listen.is_empty() { "0.0.0.0:7443" } else { &listen };
            println!("🔧 Initializing cluster '{name}' on {addr}...");

            let app_state = sparrow_api::init_cluster(&name, "localhost", addr);
            let cs_clone = app_state.clone();
            let listen_addr = addr.to_string();
            tokio::spawn(async move {
                if let Err(e) = sparrow_api::start_api(cs_clone, &listen_addr).await {
                    tracing::error!("API server failed: {e}");
                }
            });

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            println!("✅ Cluster '{name}' initialized. API listening on {addr}");
            println!("   Join token: sparrow-{}-tok-{:x}", name, name.len());
            *cluster_state = Some(app_state);
        }
        ClusterAction::Join { addr, token } => {
            println!("🔗 Joining cluster at {addr} with token {token}...");
            println!("⚠️  Full join protocol — Raft consensus (Fase 2)");
        }
        ClusterAction::Status => {
            match cluster_state {
                Some(cs) => {
                    let cluster = cs.cluster.read().await;
                    println!("📊 Cluster: '{}' ({} nodes)", cluster.name, cluster.nodes.len());
                    for node in cluster.nodes.values() {
                        println!("   {} @ {} — {} ({})", node.name, node.addr, node.role, node.status);
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

            println!("{:<24} {:<28} {:<10} {:<20}", "NAME", "IMAGE", "REPLICAS", "CREATED");
            println!("{}", "-".repeat(82));
            for svc in &services {
                println!("{:<24} {:<28} {:<10} {:<20}", svc.name, svc.image, svc.desired_replicas, svc.created_at.format("%Y-%m-%d %H:%M"));
            }
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
                        println!("{:<20} {:<10} {:<10} {:<16}", "CONTAINER", "STATUS", "CPU", "MEM");
                        println!("{}", "-".repeat(56));
                        for c in &containers {
                            let (cpu, mem) = if c.state == ContainerState::Running {
                                runtime.stats(&c.name).await.unwrap_or((0.0, 0))
                            } else { (0.0, 0) };
                            let mem_str = if mem > 0 { format_bytes(mem) } else { "-".to_string() };
                            println!("{:<20} {:<10} {:<10} {:<16}", c.name, c.state.to_string(), format_cpu(cpu), mem_str);
                        }
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

async fn handle_node(action: NodeAction) -> anyhow::Result<()> {
    match action {
        NodeAction::List => println!("📋 Nodes:\n  localhost (self) - Ready"),
        NodeAction::Inspect { name } => println!("⚠️  Node inspect '{name}' — Fase 2"),
        NodeAction::Drain { name } => println!("⚠️  Drain '{name}' — Fase 2"),
        NodeAction::Rm { name } => println!("⚠️  Remove '{name}' — Fase 2"),
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
            println!("{:<24} {:<10} {:<20}", "NAME", "DRIVER", "SUBNET");
            println!("{}", "-".repeat(54));
            for line in stdout.lines().filter(|l| !l.is_empty()) {
                let parts: Vec<&str> = line.split('\t').collect();
                let name = parts.first().unwrap_or(&"");
                let driver = parts.get(1).unwrap_or(&"");
                let subnet = parts.get(2).unwrap_or(&"");
                println!("{:<24} {:<10} {:<20}", name, driver, subnet);
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
            println!("⚠️  Autoscale history '{service}' (last {last}) — Fase 3");
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

async fn handle_alert(action: AlertAction) -> anyhow::Result<()> {
    match action {
        AlertAction::Set => println!("⚠️  Alert set — Fase 4"),
        AlertAction::List => println!("⚠️  Alert list — Fase 4"),
        AlertAction::History => println!("⚠️  Alert history — Fase 4"),
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
