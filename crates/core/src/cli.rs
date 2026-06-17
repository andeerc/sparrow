use clap::{Parser, Subcommand};

/// Sparrow - Container orchestrator for Podman
#[derive(Parser, Debug)]
#[command(name = "sparrow", version, about)]
pub struct Cli {
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

    /// Show cluster status
    Status,
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
pub enum AlertAction {
    /// Set alert rule
    Set,
    /// List alert rules
    List,
    /// Show alert history
    History,
}
