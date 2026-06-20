use clap::{Parser, Subcommand};

/// Sparrow - Container orchestrator for Podman
#[derive(Parser, Debug)]
#[command(name = "sparrow", version, about)]
pub struct Cli {
    /// Path to config file
    #[arg(long, global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage the cluster
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },

    /// Manage services
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// Manage nodes
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },

    /// Manage networks
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },

    /// Manage auto-scaling
    Autoscale {
        #[command(subcommand)]
        action: AutoscaleAction,
    },

    /// Manage alerts
    Alert {
        #[command(subcommand)]
        action: AlertAction,
    },

    /// MCP server mode
    Mcp {
        /// Port to listen on
        #[arg(long, default_value = "3000")]
        port: u16,

        /// Host to bind
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },

    /// Deploy from YAML file
    Deploy {
        /// Path to YAML file
        file: String,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Database management (backup, vacuum)
    Db {
        /// Database operation: backup, vacuum
        operation: String,
        /// Output path for backup file
        path: Option<String>,
    },

    /// Check for updates and upgrade Sparrow
    Update {
        #[command(subcommand)]
        action: UpdateAction,
    },

    /// Generate shell completion script
    Completion {
        /// Shell type: bash, zsh, fish, powershell, elvish
        shell: String,
    },

    /// Manage secrets vault
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },

    /// Show cluster status
    Status,
}

#[derive(Subcommand, Debug)]
pub enum UpdateAction {
    /// Check if a new version is available
    Check,
    /// Download and install the latest version
    Install,
}

#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Initialize vault key
    Init,
    /// Set a secret value (encrypts and stores)
    Set {
        /// Secret name (e.g. "myapp/DB_PASSWORD")
        name: String,
        /// Secret value
        value: String,
    },
    /// Get a decrypted secret value
    Get {
        /// Secret name
        name: String,
    },
    /// List all secret names
    List,
    /// Remove a secret
    Rm {
        /// Secret name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ClusterAction {
    /// Initialize a new cluster
    Init {
        /// Cluster name
        #[arg(long, default_value = "default")]
        name: String,

        /// Listen address
        #[arg(long, default_value = "0.0.0.0:7443")]
        listen: String,
    },

    /// Join an existing cluster
    Join {
        /// Leader address
        addr: String,

        /// Join token
        #[arg(long)]
        token: String,
    },

    /// Show cluster status
    Status,

    /// List cluster members
    Members,
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Create a new service
    Create {
        /// Service name
        #[arg(long)]
        name: String,

        /// Container image
        #[arg(long)]
        image: String,

        /// Number of replicas
        #[arg(long, default_value = "1")]
        replicas: u32,

        /// Port mappings (e.g. "80:80")
        #[arg(long)]
        port: Vec<String>,

        /// Environment variables (e.g. "KEY=val")
        #[arg(long)]
        env: Vec<String>,

        /// Volume mounts (e.g. "/host:/container")
        #[arg(long)]
        volume: Vec<String>,

        /// Network to attach
        #[arg(long)]
        network: Option<String>,

        /// Restart policy
        #[arg(long, default_value = "always")]
        restart: String,

        /// Domain for reverse proxy
        #[arg(long)]
        domain: Option<String>,

        /// Enable auto-scaling
        #[arg(long)]
        autoscale: bool,
    },

    /// List services
    List,

    /// Show service details
    Inspect {
        /// Service name or ID
        name: String,
    },

    /// Scale a service
    Scale {
        /// Service name or ID
        name: String,

        /// Number of replicas
        replicas: u32,
    },

    /// Remove a service
    Rm {
        /// Service name or ID
        name: String,
    },

    /// Show container logs
    Logs {
        /// Service name or ID
        name: String,

        /// Number of lines
        #[arg(long, default_value = "100")]
        tail: u32,

        /// Follow logs
        #[arg(long)]
        follow: bool,
    },

    /// Show container list for a service
    Ps {
        /// Service name or ID
        name: String,
    },

    /// Update service image or config
    Update {
        /// Service name or ID
        name: String,

        /// New image
        #[arg(long)]
        image: Option<String>,

        /// Update parallelism
        #[arg(long, default_value = "1")]
        parallelism: u32,

        /// Delay between updates
        #[arg(long, default_value = "10s")]
        delay: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// List nodes
    List,

    /// Show node details
    Inspect { name: String },

    /// Drain a node (migrate containers)
    Drain { name: String },

    /// Remove a node
    Rm { name: String },
}

#[derive(Subcommand, Debug)]
pub enum NetworkAction {
    /// Create a network
    Create {
        /// Network name
        name: String,
        /// CIDR subnet
        #[arg(long)]
        subnet: Option<String>,
    },

    /// List networks
    List,

    /// Remove a network
    Rm { name: String },
}

#[derive(Subcommand, Debug)]
pub enum AutoscaleAction {
    /// Configure auto-scaling
    Set {
        /// Service name or ID
        service: String,

        /// Minimum replicas
        #[arg(long)]
        min: Option<u32>,

        /// Maximum replicas
        #[arg(long)]
        max: Option<u32>,

        /// CPU target percentage
        #[arg(long)]
        cpu_target: Option<f64>,

        /// Memory target percentage
        #[arg(long)]
        mem_target: Option<f64>,

        /// Cooldown in seconds
        #[arg(long)]
        cooldown: Option<u64>,
    },

    /// Show auto-scaling status
    Status { service: String },

    /// Show decision history
    History {
        service: String,
        #[arg(long, default_value = "24h")]
        last: String,
    },

    /// Pause auto-scaling
    Pause { service: String },

    /// Resume auto-scaling
    Resume { service: String },
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum AlertAction {
    /// Set alert channel (telegram or smtp)
    Set {
        /// Channel id
        id: String,
        /// Channel type: telegram or smtp
        channel_type: String,
        /// Channel name (optional)
        #[arg(long, default_value = "")]
        name: String,
        /// Telegram bot token (required for telegram)
        #[arg(long)]
        bot_token: Option<String>,
        /// Telegram chat id (required for telegram)
        #[arg(long)]
        chat_id: Option<String>,
        /// SMTP host (required for smtp)
        #[arg(long)]
        smtp_host: Option<String>,
        /// SMTP port (default: 587)
        #[arg(long)]
        smtp_port: Option<u16>,
        /// SMTP username (required for smtp)
        #[arg(long)]
        smtp_username: Option<String>,
        /// SMTP password (required for smtp)
        #[arg(long)]
        smtp_password: Option<String>,
        /// From address (required for smtp)
        #[arg(long)]
        from: Option<String>,
        /// To address (required for smtp)
        #[arg(long)]
        to: Option<String>,
    },
    /// List alert channels
    List,
    /// Show alert history
    History,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ConfigAction {
    /// Initialize or overwrite config file
    Init {
        /// Path to write config file (default: /etc/sparrow/sparrow.yaml)
        #[arg(long)]
        path: Option<String>,

        /// Cluster name
        #[arg(long)]
        cluster_name: Option<String>,

        /// Listen address for cluster API
        #[arg(long)]
        listen: Option<String>,

        /// Raft port
        #[arg(long)]
        raft_port: Option<u16>,

        /// Data directory
        #[arg(long)]
        data_dir: Option<String>,

        /// Runtime backend (default: podman)
        #[arg(long)]
        runtime_backend: Option<String>,

        /// Run containers rootless
        #[arg(long)]
        rootless: Option<bool>,

        /// Podman socket path
        #[arg(long)]
        podman_socket: Option<String>,

        /// Log level (trace, debug, info, warn, error)
        #[arg(long)]
        log_level: Option<String>,

        /// Log format (plain, json)
        #[arg(long)]
        log_format: Option<String>,

        /// Log file path
        #[arg(long)]
        log_file: Option<String>,

        /// API listen address (for HTTP API)
        #[arg(long)]
        api_listen: Option<String>,
    },

    /// Show current configuration
    Show {
        /// Path to config file (default: /etc/sparrow/sparrow.yaml)
        #[arg(long)]
        path: Option<String>,

        /// Output as YAML
        #[arg(long)]
        yaml: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(args).unwrap().command
    }

    #[test]
    fn test_cli_config_flag() {
        let cmd = Cli::try_parse_from(&["sparrow", "--config", "/tmp/cfg.yaml", "status"]).unwrap();
        assert_eq!(cmd.config, Some("/tmp/cfg.yaml".into()));
        assert!(matches!(cmd.command, Command::Status));
    }

    #[test]
    fn test_parse_cluster_init() {
        let cmd = parse(&[
            "sparrow",
            "cluster",
            "init",
            "--name",
            "prod",
            "--listen",
            "0.0.0.0:7443",
        ]);
        assert!(matches!(&cmd, Command::Cluster { .. }));
        if let Command::Cluster { action } = &cmd {
            assert!(
                matches!(action, ClusterAction::Init { name, listen } if name == "prod" && listen == "0.0.0.0:7443")
            );
        }
    }

    #[test]
    fn test_parse_cluster_status() {
        let cmd = parse(&["sparrow", "cluster", "status"]);
        assert!(matches!(
            &cmd,
            Command::Cluster {
                action: ClusterAction::Status
            }
        ));
    }

    #[test]
    fn test_parse_cluster_members() {
        let cmd = parse(&["sparrow", "cluster", "members"]);
        assert!(matches!(
            &cmd,
            Command::Cluster {
                action: ClusterAction::Members
            }
        ));
    }

    #[test]
    fn test_parse_service_create() {
        let cmd = parse(&[
            "sparrow",
            "service",
            "create",
            "--name",
            "web",
            "--image",
            "nginx",
            "--replicas",
            "3",
            "--port",
            "80:80",
        ]);
        if let Command::Service { action } = &cmd {
            assert!(
                matches!(action, ServiceAction::Create { name, image, replicas, .. } if name == "web" && image == "nginx" && *replicas == 3)
            );
        }
    }

    #[test]
    fn test_parse_service_create_with_env() {
        let cmd = parse(&[
            "sparrow", "service", "create", "--name", "app", "--image", "node", "--env", "FOO=bar",
            "--env", "BAZ=qux",
        ]);
        if let Command::Service { action } = &cmd {
            if let ServiceAction::Create { env, .. } = action {
                assert_eq!(env.len(), 2);
            } else {
                panic!();
            }
        }
    }

    #[test]
    fn test_parse_service_list() {
        let cmd = parse(&["sparrow", "service", "list"]);
        assert!(matches!(
            &cmd,
            Command::Service {
                action: ServiceAction::List
            }
        ));
    }

    #[test]
    fn test_parse_service_scale() {
        let cmd = parse(&["sparrow", "service", "scale", "myapp", "5"]);
        if let Command::Service { action } = &cmd {
            assert!(
                matches!(action, ServiceAction::Scale { name, replicas } if name == "myapp" && *replicas == 5)
            );
        }
    }

    #[test]
    fn test_parse_service_logs() {
        let cmd = parse(&["sparrow", "service", "logs", "myapp", "--tail", "50"]);
        if let Command::Service { action } = &cmd {
            assert!(
                matches!(action, ServiceAction::Logs { name, tail, follow } if name == "myapp" && *tail == 50 && !follow)
            );
        }
    }

    #[test]
    fn test_parse_service_logs_follow() {
        let cmd = parse(&["sparrow", "service", "logs", "myapp", "--follow"]);
        if let Command::Service { action } = &cmd {
            assert!(
                matches!(action, ServiceAction::Logs { name, follow, .. } if name == "myapp" && *follow)
            );
        }
    }

    #[test]
    fn test_parse_service_rm() {
        let cmd = parse(&["sparrow", "service", "rm", "myapp"]);
        if let Command::Service { action } = &cmd {
            assert!(matches!(action, ServiceAction::Rm { name } if name == "myapp"));
        }
    }

    #[test]
    fn test_parse_config_init() {
        let cmd = parse(&[
            "sparrow",
            "config",
            "init",
            "--cluster-name",
            "test",
            "--log-level",
            "debug",
        ]);
        if let Command::Config { action } = &cmd {
            assert!(matches!(action, ConfigAction::Init { .. }));
        }
    }

    #[test]
    fn test_parse_config_show() {
        let cmd = parse(&["sparrow", "config", "show", "--yaml"]);
        if let Command::Config { action } = &cmd {
            assert!(matches!(action, ConfigAction::Show { yaml, .. } if *yaml));
        }
    }

    #[test]
    fn test_parse_config_show_with_path() {
        let cmd = parse(&["sparrow", "config", "show", "--path", "./cfg.yaml"]);
        if let Command::Config { action } = &cmd {
            if let ConfigAction::Show { path, yaml } = action {
                assert_eq!(path.as_deref(), Some("./cfg.yaml"));
                assert!(!yaml);
            } else {
                panic!();
            }
        }
    }

    #[test]
    fn test_parse_deploy() {
        let cmd = parse(&["sparrow", "deploy", "service.yaml"]);
        assert!(matches!(&cmd, Command::Deploy { file } if file == "service.yaml"));
    }

    #[test]
    fn test_parse_status() {
        let cmd = parse(&["sparrow", "status"]);
        assert!(matches!(cmd, Command::Status));
    }

    #[test]
    fn test_parse_mcp_defaults() {
        let cmd = parse(&["sparrow", "mcp"]);
        if let Command::Mcp { port, host } = &cmd {
            assert_eq!(*port, 3000);
            assert_eq!(host, "127.0.0.1");
        } else {
            panic!();
        }
    }

    #[test]
    fn test_parse_mcp_custom() {
        let cmd = parse(&["sparrow", "mcp", "--port", "8080", "--host", "0.0.0.0"]);
        if let Command::Mcp { port, host } = &cmd {
            assert_eq!(*port, 8080);
            assert_eq!(host, "0.0.0.0");
        } else {
            panic!();
        }
    }

    #[test]
    fn test_parse_network_create() {
        let cmd = parse(&[
            "sparrow",
            "network",
            "create",
            "mynet",
            "--subnet",
            "10.88.0.0/16",
        ]);
        if let Command::Network { action } = &cmd {
            assert!(matches!(action, NetworkAction::Create { name, .. } if name == "mynet"));
        }
    }

    #[test]
    fn test_parse_network_list() {
        let cmd = parse(&["sparrow", "network", "list"]);
        assert!(matches!(
            &cmd,
            Command::Network {
                action: NetworkAction::List
            }
        ));
    }

    #[test]
    fn test_parse_network_rm() {
        let cmd = parse(&["sparrow", "network", "rm", "mynet"]);
        assert!(matches!(
            &cmd,
            Command::Network {
                action: NetworkAction::Rm { .. }
            }
        ));
    }

    #[test]
    fn test_parse_autoscale_set() {
        let cmd = parse(&[
            "sparrow",
            "autoscale",
            "set",
            "mysvc",
            "--min",
            "2",
            "--max",
            "10",
            "--cpu-target",
            "70",
        ]);
        if let Command::Autoscale { action } = &cmd {
            assert!(matches!(action, AutoscaleAction::Set { service, .. } if service == "mysvc"));
        }
    }

    #[test]
    fn test_parse_autoscale_status() {
        let cmd = parse(&["sparrow", "autoscale", "status", "mysvc"]);
        if let Command::Autoscale { action } = &cmd {
            assert!(matches!(action, AutoscaleAction::Status { service } if service == "mysvc"));
        }
    }

    #[test]
    fn test_parse_autoscale_pause() {
        let cmd = parse(&["sparrow", "autoscale", "pause", "mysvc"]);
        assert!(matches!(
            &cmd,
            Command::Autoscale {
                action: AutoscaleAction::Pause { .. }
            }
        ));
    }

    #[test]
    fn test_parse_node_list() {
        let cmd = parse(&["sparrow", "node", "list"]);
        assert!(matches!(
            &cmd,
            Command::Node {
                action: NodeAction::List
            }
        ));
    }

    #[test]
    fn test_parse_node_inspect() {
        let cmd = parse(&["sparrow", "node", "inspect", "node1"]);
        if let Command::Node { action } = &cmd {
            assert!(matches!(action, NodeAction::Inspect { name } if name == "node1"));
        }
    }

    #[test]
    fn test_parse_node_drain() {
        let cmd = parse(&["sparrow", "node", "drain", "node1"]);
        assert!(matches!(
            &cmd,
            Command::Node {
                action: NodeAction::Drain { .. }
            }
        ));
    }

    #[test]
    fn test_parse_alert_set_telegram() {
        let cmd = parse(&[
            "sparrow",
            "alert",
            "set",
            "mychan",
            "telegram",
            "--bot-token",
            "tok123",
            "--chat-id",
            "chat456",
        ]);
        if let Command::Alert { action } = &cmd {
            assert!(
                matches!(action, AlertAction::Set { id, channel_type, .. } if id == "mychan" && channel_type == "telegram")
            );
        }
    }

    #[test]
    fn test_parse_alert_list() {
        let cmd = parse(&["sparrow", "alert", "list"]);
        assert!(matches!(
            &cmd,
            Command::Alert {
                action: AlertAction::List
            }
        ));
    }

    #[test]
    fn test_parse_version_flag_not_consuming_command() {
        // --version is handled by clap before reaching our parse, just verify help works
        let result = Cli::try_parse_from(&["sparrow", "status"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_update_check() {
        let cmd = parse(&["sparrow", "update", "check"]);
        assert!(matches!(
            &cmd,
            Command::Update {
                action: UpdateAction::Check
            }
        ));
    }

    #[test]
    fn test_parse_update_install() {
        let cmd = parse(&["sparrow", "update", "install"]);
        assert!(matches!(
            &cmd,
            Command::Update {
                action: UpdateAction::Install
            }
        ));
    }
}
