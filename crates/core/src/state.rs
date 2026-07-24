use crate::crypto;
use chrono::Utc;
use rusqlite::{params, Connection};
use sparrow_proto::*;
use std::path::Path;
use std::sync::Mutex;

pub struct StateStore {
    conn: Mutex<Connection>,
    db_path: String,
}

impl StateStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA wal_autocheckpoint=1000;",
        )?;

        let store = Self {
            conn: Mutex::new(conn),
            db_path: path.to_string_lossy().to_string(),
        };
        store.migrate()?;
        store.wal_checkpoint();
        Ok(store)
    }

    fn wal_checkpoint(&self) {
        if let Ok(conn) = self.conn() {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
    }

    fn conn(&self) -> anyhow::Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))
    }

    pub fn backup(&self, dest: &str) -> anyhow::Result<()> {
        // Flush WAL then copy the database file
        let conn = self.conn()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(conn);
        std::fs::copy(&self.db_path, dest)?;
        Ok(())
    }

    pub fn vacuum(&self) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );

            CREATE TABLE IF NOT EXISTS services (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL UNIQUE,
                image       TEXT NOT NULL,
                desired_replicas INTEGER NOT NULL DEFAULT 1,
                restart_policy  TEXT NOT NULL DEFAULT 'Always',
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS service_ports (
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                published   INTEGER NOT NULL,
                target      INTEGER NOT NULL,
                protocol    TEXT NOT NULL DEFAULT 'Tcp'
            );

            CREATE TABLE IF NOT EXISTS service_env (
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                key         TEXT NOT NULL,
                value       TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS service_volumes (
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                source      TEXT NOT NULL,
                target      TEXT NOT NULL,
                read_only   INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS service_networks (
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                network     TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS containers (
                container_name TEXT PRIMARY KEY,
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                image       TEXT NOT NULL,
                replica_seq INTEGER NOT NULL,
                state       TEXT NOT NULL DEFAULT 'Created',
                created_at  TEXT NOT NULL,
                node_id     TEXT NOT NULL DEFAULT '',
                ip_address  TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS deployments (
                id          TEXT PRIMARY KEY,
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                image       TEXT NOT NULL,
                replicas    INTEGER NOT NULL,
                strategy    TEXT NOT NULL DEFAULT 'rolling',
                status      TEXT NOT NULL DEFAULT 'pending',
                created_at  TEXT NOT NULL,
                finished_at TEXT
            );

            CREATE TABLE IF NOT EXISTS autoscale_config (
                service_id          TEXT PRIMARY KEY REFERENCES services(id) ON DELETE CASCADE,
                min_replicas        INTEGER NOT NULL DEFAULT 1,
                max_replicas        INTEGER NOT NULL DEFAULT 10,
                cpu_target_percent  REAL,
                mem_target_percent  REAL,
                cooldown_seconds    INTEGER NOT NULL DEFAULT 60,
                paused              INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS alert_rules (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                metric      TEXT NOT NULL,
                operator    TEXT NOT NULL,
                threshold   REAL NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                enabled     INTEGER NOT NULL DEFAULT 1,
                created_at  TEXT NOT NULL
            );

            INSERT OR IGNORE INTO schema_version (version) VALUES (1);

            CREATE TABLE IF NOT EXISTS autoscale_events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                decision    TEXT NOT NULL,
                replicas_from INTEGER NOT NULL,
                replicas_to INTEGER NOT NULL,
                reason      TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL
            );
            INSERT OR IGNORE INTO schema_version (version) VALUES (2);

            CREATE TABLE IF NOT EXISTS alert_events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id  TEXT NOT NULL,
                channel_type TEXT NOT NULL DEFAULT '',
                metric      TEXT NOT NULL DEFAULT '',
                value       REAL NOT NULL DEFAULT 0,
                threshold   REAL NOT NULL DEFAULT 0,
                message     TEXT NOT NULL DEFAULT '',
                severity    TEXT NOT NULL DEFAULT 'info',
                status      TEXT NOT NULL DEFAULT 'sent',
                created_at  TEXT NOT NULL
            );
            INSERT OR IGNORE INTO schema_version (version) VALUES (3);

            CREATE TABLE IF NOT EXISTS alert_channels (
                id           TEXT PRIMARY KEY,
                channel_type TEXT NOT NULL,
                name         TEXT NOT NULL DEFAULT '',
                config_json  TEXT NOT NULL DEFAULT '{}',
                enabled      INTEGER NOT NULL DEFAULT 1,
                created_at   TEXT NOT NULL
            );
            INSERT OR IGNORE INTO schema_version (version) VALUES (4);
            ",
        )?;

        // Migration v5: add ip_address column to containers table (if not exists)
        let has_ip = conn
            .prepare("SELECT ip_address FROM containers LIMIT 1")
            .is_ok();
        if !has_ip {
            conn.execute_batch(
                "ALTER TABLE containers ADD COLUMN ip_address TEXT NOT NULL DEFAULT '';
                 INSERT OR IGNORE INTO schema_version (version) VALUES (5);",
            )?;
        }

        // Migration v6: add secrets vault
        let has_secrets = conn.prepare("SELECT name FROM secrets LIMIT 1").is_ok();
        if !has_secrets {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS secrets (
                    name            TEXT PRIMARY KEY,
                    encrypted_value TEXT NOT NULL,
                    created_at      TEXT NOT NULL,
                    updated_at      TEXT NOT NULL
                );
                INSERT OR IGNORE INTO schema_version (version) VALUES (6);",
            )?;
        }

        // Migration v7: add proxy_routes table
        let has_proxy_routes = conn
            .prepare("SELECT domain FROM proxy_routes LIMIT 1")
            .is_ok();
        if !has_proxy_routes {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS proxy_routes (
                    domain       TEXT PRIMARY KEY,
                    target_port  INTEGER NOT NULL,
                    service_name TEXT NOT NULL,
                    tls          INTEGER NOT NULL DEFAULT 0
                );
                INSERT OR IGNORE INTO schema_version (version) VALUES (7);",
            )?;
        }

        // Migration v8: autoscale metrics history for predictive scaling
        let has_metrics = conn
            .prepare("SELECT service_id FROM autoscale_metrics LIMIT 1")
            .is_ok();
        if !has_metrics {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS autoscale_metrics (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    service_id  TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
                    avg_cpu     REAL NOT NULL,
                    max_mem_pct REAL NOT NULL DEFAULT 0,
                    replicas    INTEGER NOT NULL DEFAULT 0,
                    collected_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_autoscale_metrics_svc_time
                    ON autoscale_metrics(service_id, collected_at);
                INSERT OR IGNORE INTO schema_version (version) VALUES (8);",
            )?;
        }

        Ok(())
    }

    fn load_ports(&self, conn: &Connection, service_id: &str) -> anyhow::Result<Vec<PortMapping>> {
        let mut stmt = conn.prepare(
            "SELECT published, target, protocol FROM service_ports WHERE service_id = ?1",
        )?;
        let ports = stmt
            .query_map(params![service_id], |row| {
                Ok(PortMapping {
                    published: row.get(0)?,
                    target: row.get(1)?,
                    protocol: {
                        let p: String = row.get(2)?;
                        serde_json::from_str(&format!("\"{p}\"")).unwrap_or_default()
                    },
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ports)
    }

    fn load_env(&self, conn: &Connection, service_id: &str) -> anyhow::Result<Vec<EnvVar>> {
        let mut stmt = conn.prepare("SELECT key, value FROM service_env WHERE service_id = ?1")?;
        let env = stmt
            .query_map(params![service_id], |row| {
                Ok(EnvVar {
                    key: row.get(0)?,
                    value: row.get(1)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(env)
    }

    fn load_volumes(
        &self,
        conn: &Connection,
        service_id: &str,
    ) -> anyhow::Result<Vec<VolumeMount>> {
        let mut stmt = conn.prepare(
            "SELECT source, target, read_only FROM service_volumes WHERE service_id = ?1",
        )?;
        let volumes = stmt
            .query_map(params![service_id], |row| {
                let read_only: i32 = row.get(2)?;
                Ok(VolumeMount {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    read_only: read_only != 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(volumes)
    }

    fn load_networks(&self, conn: &Connection, service_id: &str) -> anyhow::Result<Vec<String>> {
        let mut stmt =
            conn.prepare("SELECT network FROM service_networks WHERE service_id = ?1")?;
        let networks = stmt
            .query_map(params![service_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(networks)
    }

    pub fn create_service(&self, spec: &ServiceSpec) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO services (id, name, image, desired_replicas, restart_policy, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                spec.id,
                spec.name,
                spec.image,
                spec.desired_replicas,
                format!("{:?}", spec.restart_policy),
                spec.created_at.to_rfc3339(),
                spec.updated_at.to_rfc3339(),
            ],
        )?;

        // Ports
        let mut stmt = conn.prepare(
            "INSERT INTO service_ports (service_id, published, target, protocol) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for p in &spec.ports {
            stmt.execute(params![
                spec.id,
                p.published,
                p.target,
                format!("{:?}", p.protocol)
            ])?;
        }

        // Env vars
        let mut stmt =
            conn.prepare("INSERT INTO service_env (service_id, key, value) VALUES (?1, ?2, ?3)")?;
        for e in &spec.env {
            stmt.execute(params![spec.id, e.key, e.value])?;
        }

        // Volumes
        let mut stmt = conn.prepare(
            "INSERT INTO service_volumes (service_id, source, target, read_only) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for v in &spec.volumes {
            stmt.execute(params![spec.id, v.source, v.target, v.read_only as i32])?;
        }

        // Networks
        let mut stmt =
            conn.prepare("INSERT INTO service_networks (service_id, network) VALUES (?1, ?2)")?;
        for n in &spec.networks {
            stmt.execute(params![spec.id, n])?;
        }

        Ok(())
    }

    pub fn list_services(&self) -> anyhow::Result<Vec<ServiceSpec>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, image, desired_replicas, restart_policy, created_at, updated_at FROM services ORDER BY created_at DESC",
        )?;

        let services = stmt
            .query_map([], |row| {
                let created: String = row.get(5)?;
                let updated: String = row.get(6)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, String>(4)?,
                    created,
                    updated,
                ))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, name, image, desired, rp, created, updated)| {
                let ports = self.load_ports(&conn, &id).unwrap_or_default();
                let env = self.load_env(&conn, &id).unwrap_or_default();
                let volumes = self.load_volumes(&conn, &id).unwrap_or_default();
                let networks = self.load_networks(&conn, &id).unwrap_or_default();
                Ok(ServiceSpec {
                    id,
                    name,
                    image,
                    desired_replicas: desired,
                    ports,
                    env,
                    volumes,
                    networks,
                    labels: std::collections::HashMap::new(),
                    resources: None,
                    restart_policy: match rp.as_str() {
                        "Always" => RestartPolicy::Always,
                        "OnFailure" => RestartPolicy::OnFailure,
                        "No" => RestartPolicy::No,
                        _ => RestartPolicy::Always,
                    },
                    command: None,
                    autoscaling: None,
                    created_at: created.parse().unwrap_or_else(|_| Utc::now()),
                    updated_at: updated.parse().unwrap_or_else(|_| Utc::now()),
                })
            })
            .filter_map(|r: anyhow::Result<ServiceSpec>| r.ok())
            .collect();

        Ok(services)
    }

    pub fn get_service_with_conn(
        &self,
        conn: &Connection,
        id_or_name: &str,
    ) -> anyhow::Result<Option<ServiceSpec>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, image, desired_replicas, restart_policy, created_at, updated_at
             FROM services WHERE id = ?1 OR name = ?1",
        )?;

        let mut rows = stmt.query_map(params![id_or_name], |row| {
            let created: String = row.get(5)?;
            let updated: String = row.get(6)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, String>(4)?,
                created,
                updated,
            ))
        })?;

        match rows.next() {
            Some(Ok((id, name, image, desired, rp, created, updated))) => {
                let ports = self.load_ports(conn, &id).unwrap_or_default();
                let env = self.load_env(conn, &id).unwrap_or_default();
                let volumes = self.load_volumes(conn, &id).unwrap_or_default();
                let networks = self.load_networks(conn, &id).unwrap_or_default();
                Ok(Some(ServiceSpec {
                    id,
                    name,
                    image,
                    desired_replicas: desired,
                    ports,
                    env,
                    volumes,
                    networks,
                    labels: std::collections::HashMap::new(),
                    resources: None,
                    restart_policy: match rp.as_str() {
                        "Always" => RestartPolicy::Always,
                        "OnFailure" => RestartPolicy::OnFailure,
                        "No" => RestartPolicy::No,
                        _ => RestartPolicy::Always,
                    },
                    command: None,
                    autoscaling: None,
                    created_at: created.parse().unwrap_or_else(|_| Utc::now()),
                    updated_at: updated.parse().unwrap_or_else(|_| Utc::now()),
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn get_service(&self, id_or_name: &str) -> anyhow::Result<Option<ServiceSpec>> {
        let conn = self.conn()?;
        self.get_service_with_conn(&conn, id_or_name)
    }

    pub fn delete_service(&self, id_or_name: &str) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        let svc = {
            let mut stmt = conn.prepare("SELECT id FROM services WHERE id = ?1 OR name = ?1")?;
            let mut rows = stmt.query_map(params![id_or_name], |row| row.get::<_, String>(0))?;
            rows.next().transpose()?
        };

        if let Some(svc_id) = svc {
            conn.execute(
                "DELETE FROM service_ports WHERE service_id = ?1",
                params![svc_id],
            )?;
            conn.execute(
                "DELETE FROM service_env WHERE service_id = ?1",
                params![svc_id],
            )?;
            conn.execute(
                "DELETE FROM service_volumes WHERE service_id = ?1",
                params![svc_id],
            )?;
            conn.execute(
                "DELETE FROM service_networks WHERE service_id = ?1",
                params![svc_id],
            )?;
            conn.execute("DELETE FROM services WHERE id = ?1", params![svc_id])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn update_replicas(&self, id_or_name: &str, replicas: u32) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        let now = Utc::now().to_rfc3339();
        let affected = conn.execute(
            "UPDATE services SET desired_replicas = ?1, updated_at = ?2 WHERE id = ?3 OR name = ?3",
            params![replicas, now, id_or_name],
        )?;
        Ok(affected > 0)
    }

    // ── Container tracking ──

    pub fn record_container(
        &self,
        container_name: &str,
        service_id: &str,
        image: &str,
        replica_seq: u32,
        state: &str,
        ip_address: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO containers (container_name, service_id, image, replica_seq, state, created_at, ip_address)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![container_name, service_id, image, replica_seq, state, now, ip_address],
        )?;
        Ok(())
    }

    pub fn record_container_with_ip(
        &self,
        container_name: &str,
        service_id: &str,
        image: &str,
        replica_seq: u32,
        state: &str,
    ) -> anyhow::Result<()> {
        self.record_container(container_name, service_id, image, replica_seq, state, "")
    }

    pub fn update_container_state(
        &self,
        container_name: &str,
        state: &str,
    ) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "UPDATE containers SET state = ?1 WHERE container_name = ?2",
            params![state, container_name],
        )?;
        Ok(affected > 0)
    }

    pub fn get_service_containers(&self, service_id: &str) -> anyhow::Result<Vec<ContainerStatus>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT container_name, service_id, image, replica_seq, state, created_at, node_id, ip_address
             FROM containers WHERE service_id = ?1 ORDER BY replica_seq",
        )?;

        let containers = stmt
            .query_map(params![service_id], |row| {
                let state_str: String = row.get(4)?;
                let created: String = row.get(5)?;
                let ip: String = row.get(7)?;
                let container_state = match state_str.as_str() {
                    "Running" => ContainerState::Running,
                    "Exited" => ContainerState::Exited,
                    "Paused" => ContainerState::Paused,
                    "Created" => ContainerState::Created,
                    _ => ContainerState::Unknown,
                };
                Ok(ContainerStatus {
                    id: row.get(0)?,
                    service_id: row.get(1)?,
                    node_id: row.get(6)?,
                    name: row.get(0)?,
                    image: row.get(2)?,
                    state: container_state,
                    exit_code: None,
                    cpu_percent: None,
                    mem_bytes: None,
                    started_at: created.parse().ok(),
                    ip_address: if ip.is_empty() { None } else { Some(ip) },
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(containers)
    }

    // ── Autoscaling config ──

    pub fn set_autoscale(
        &self,
        service_id: &str,
        config: &AutoscalingConfig,
        paused: bool,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO autoscale_config
             (service_id, min_replicas, max_replicas, cpu_target_percent, mem_target_percent, cooldown_seconds, paused)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                service_id,
                config.min_replicas,
                config.max_replicas,
                config.cpu_target_percent,
                config.memory_target_percent,
                config.cooldown_seconds,
                paused as i32,
            ],
        )?;
        Ok(())
    }

    pub fn get_autoscale(
        &self,
        service_id: &str,
    ) -> anyhow::Result<Option<(AutoscalingConfig, bool)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT min_replicas, max_replicas, cpu_target_percent, mem_target_percent, cooldown_seconds, paused
             FROM autoscale_config WHERE service_id = ?1",
        )?;

        let mut rows = stmt.query_map(params![service_id], |row| {
            let paused: i32 = row.get(5)?;
            Ok((
                AutoscalingConfig {
                    min_replicas: row.get(0)?,
                    max_replicas: row.get(1)?,
                    cpu_target_percent: row.get(2)?,
                    memory_target_percent: row.get(3)?,
                    cooldown_seconds: row.get::<_, i64>(4)? as u64,
                },
                paused != 0,
            ))
        })?;

        Ok(rows.next().transpose()?)
    }

    pub fn record_autoscale_event(
        &self,
        service_id: &str,
        decision: &str,
        replicas_from: u32,
        replicas_to: u32,
        reason: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO autoscale_events (service_id, decision, replicas_from, replicas_to, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![service_id, decision, replicas_from, replicas_to, reason, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn list_autoscale_events(
        &self,
        service_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<AutoscaleEvent>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT decision, replicas_from, replicas_to, reason, created_at
             FROM autoscale_events WHERE service_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let events = stmt
            .query_map(params![service_id, limit], |row| {
                Ok(AutoscaleEvent {
                    decision: row.get(0)?,
                    replicas_from: row.get(1)?,
                    replicas_to: row.get(2)?,
                    reason: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(events)
    }

    pub fn list_alert_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, metric, operator, threshold, duration_secs, enabled, created_at
             FROM alert_rules ORDER BY created_at DESC",
        )?;
        let rules = stmt
            .query_map([], |row| {
                Ok(AlertRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    metric: row.get(2)?,
                    operator: row.get(3)?,
                    threshold: row.get(4)?,
                    duration_secs: row.get(5)?,
                    enabled: row.get::<_, i32>(6)? != 0,
                    created_at: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rules)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_alert_event(
        &self,
        channel_id: &str,
        channel_type: &str,
        metric: &str,
        value: f64,
        threshold: f64,
        message: &str,
        severity: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO alert_events (channel_id, channel_type, metric, value, threshold, message, severity, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'sent', ?8)",
            params![
                channel_id,
                channel_type,
                metric,
                value,
                threshold,
                message,
                severity,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_alert_events(&self, limit: u32) -> anyhow::Result<Vec<AlertEventRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT channel_id, channel_type, metric, value, threshold, message, severity, status, created_at
             FROM alert_events ORDER BY created_at DESC LIMIT ?1",
        )?;
        let events = stmt
            .query_map(params![limit], |row| {
                Ok(AlertEventRecord {
                    channel_id: row.get(0)?,
                    channel_type: row.get(1)?,
                    metric: row.get(2)?,
                    value: row.get(3)?,
                    threshold: row.get(4)?,
                    message: row.get(5)?,
                    severity: row.get(6)?,
                    status: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(events)
    }

    pub fn record_alert_channel(
        &self,
        id: &str,
        channel_type: &str,
        name: &str,
        config_json: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        let encrypted = crypto::encrypt(config_json, "sparrow-alert-key");
        conn.execute(
            "INSERT INTO alert_channels (id, channel_type, name, config_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                channel_type,
                name,
                encrypted,
                enabled as i32,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_alert_channels(&self) -> anyhow::Result<Vec<AlertChannelRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, channel_type, name, config_json, enabled, created_at
             FROM alert_channels ORDER BY created_at DESC",
        )?;
        let channels = stmt
            .query_map([], |row| {
                let encrypted: String = row.get(3)?;
                let decrypted = crypto::decrypt(&encrypted, "sparrow-alert-key")
                    .unwrap_or_else(|| encrypted.clone());
                Ok(AlertChannelRecord {
                    id: row.get(0)?,
                    channel_type: row.get(1)?,
                    name: row.get(2)?,
                    config_json: decrypted,
                    enabled: row.get::<_, i32>(4)? != 0,
                    created_at: row.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(channels)
    }

    // ── Secrets Vault ──

    /// Store an encrypted secret value (upsert).
    pub fn set_secret(&self, name: &str, encrypted_value: &str) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO secrets (name, encrypted_value, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET encrypted_value = ?2, updated_at = ?4",
            params![
                name,
                encrypted_value,
                Utc::now().to_rfc3339(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Retrieve encrypted secret by name.
    pub fn get_secret(&self, name: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT encrypted_value FROM secrets WHERE name = ?1")?;
        let result = stmt
            .query_map(params![name], |row| row.get::<_, String>(0))?
            .next()
            .transpose()?;
        Ok(result)
    }

    /// List all secret names (never decrypts).
    pub fn list_secrets(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT name FROM secrets ORDER BY name")?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    /// Delete a secret by name. Returns true if existed.
    pub fn delete_secret(&self, name: &str) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        let affected = conn.execute("DELETE FROM secrets WHERE name = ?1", params![name])?;
        Ok(affected > 0)
    }

    pub fn save_proxy_route(
        &self,
        domain: &str,
        target_port: u16,
        service_name: &str,
        tls: bool,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO proxy_routes (domain, target_port, service_name, tls) VALUES (?1, ?2, ?3, ?4)",
            params![domain, target_port, service_name, tls as i32],
        )?;
        Ok(())
    }

    pub fn delete_proxy_route(&self, domain: &str) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        let affected = conn.execute(
            "DELETE FROM proxy_routes WHERE domain = ?1",
            params![domain],
        )?;
        Ok(affected > 0)
    }

    pub fn list_proxy_routes(&self) -> anyhow::Result<Vec<(String, u16, String, bool)>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT domain, target_port, service_name, tls FROM proxy_routes")?;
        let rows = stmt.query_map([], |row| {
            let tls_val: i32 = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u16>(1)?,
                row.get::<_, String>(2)?,
                tls_val != 0,
            ))
        })?;
        let mut routes = vec![];
        for val in rows.flatten() {
            routes.push(val);
        }
        Ok(routes)
    }

    pub fn get_active_container_ips(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Vec<String>>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT s.name, c.ip_address 
             FROM containers c 
             JOIN services s ON c.service_id = s.id 
             WHERE c.state = 'Running' AND c.ip_address != ''",
        )?;

        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for (service_name, ip) in rows.flatten() {
            map.entry(service_name).or_default().push(ip);
        }
        Ok(map)
    }

    /// Record autoscale metric data point for predictive analysis.
    pub fn record_autoscale_metric(
        &self,
        service_id: &str,
        avg_cpu: f64,
        max_mem_pct: f64,
        replicas: u32,
    ) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO autoscale_metrics (service_id, avg_cpu, max_mem_pct, replicas)
             VALUES (?1, ?2, ?3, ?4)",
            params![service_id, avg_cpu, max_mem_pct, replicas],
        )?;
        Ok(())
    }

    /// Load recent metric history for predictive scaling (last N data points).
    /// Returns Vec of (avg_cpu, max_mem_pct) ordered oldest-first.
    pub fn load_recent_metrics(
        &self,
        service_id: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<(f64, f64)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT avg_cpu, max_mem_pct FROM autoscale_metrics
             WHERE service_id = ?1 ORDER BY collected_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![service_id, limit], |row| {
            Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
        })?;
        let mut points: Vec<(f64, f64)> = rows.flatten().collect();
        points.reverse();
        Ok(points)
    }

    /// Prune old metric data older than the given number of hours.
    pub fn prune_autoscale_metrics(&self, older_than_hours: u32) -> anyhow::Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM autoscale_metrics WHERE collected_at < datetime('now', ?1)",
            params![format!("-{older_than_hours} hours")],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparrow_proto::ContainerState;

    fn setup_store() -> (StateStore, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = StateStore::new(db.to_str().unwrap()).unwrap();
        (store, dir.keep())
    }

    fn make_spec(name: &str, image: &str, replicas: u32) -> ServiceSpec {
        let mut spec = ServiceSpec::new(name, image);
        spec.desired_replicas = replicas;
        spec
    }

    #[test]
    fn test_create_service() {
        let (store, dir) = setup_store();
        let spec = make_spec("test-svc", "nginx:alpine", 2);
        store.create_service(&spec).unwrap();

        let services = store.list_services().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "test-svc");
        assert_eq!(services[0].image, "nginx:alpine");
        assert_eq!(services[0].desired_replicas, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_service_by_id() {
        let (store, dir) = setup_store();
        let spec = make_spec("get-by-id", "redis:7", 1);
        store.create_service(&spec).unwrap();

        let found = store.get_service(&spec.id).unwrap().unwrap();
        assert_eq!(found.name, "get-by-id");
        assert_eq!(found.id, spec.id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_service_by_name() {
        let (store, dir) = setup_store();
        let spec = make_spec("get-by-name", "postgres:16", 1);
        store.create_service(&spec).unwrap();

        let found = store.get_service("get-by-name").unwrap().unwrap();
        assert_eq!(found.id, spec.id);
        assert_eq!(found.image, "postgres:16");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_replicas() {
        let (store, dir) = setup_store();
        let spec = make_spec("scale-test", "nginx", 1);
        store.create_service(&spec).unwrap();

        store.update_replicas(&spec.id, 5).unwrap();
        let updated = store.get_service(&spec.id).unwrap().unwrap();
        assert_eq!(updated.desired_replicas, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_service() {
        let (store, dir) = setup_store();
        let spec = make_spec("del-svc", "alpine", 1);
        store.create_service(&spec).unwrap();

        store.delete_service(&spec.id).unwrap();
        let services = store.list_services().unwrap();
        assert_eq!(services.len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_service_not_found() {
        let (store, dir) = setup_store();
        let result = store.get_service("nonexistent-id").unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_record_container() {
        let (store, dir) = setup_store();
        let spec = make_spec("container-test", "nginx", 1);
        store.create_service(&spec).unwrap();

        store
            .record_container("web-1", &spec.id, "nginx", 1, "Running", "")
            .unwrap();
        let containers = store.get_service_containers(&spec.id).unwrap();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "web-1");
        assert_eq!(containers[0].state, ContainerState::Running);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_service_containers_multiple() {
        let (store, dir) = setup_store();
        let spec = make_spec("multi-container", "nginx", 3);
        store.create_service(&spec).unwrap();

        store
            .record_container("svc-1", &spec.id, "nginx", 1, "Running", "")
            .unwrap();
        store
            .record_container("svc-2", &spec.id, "nginx", 2, "Running", "")
            .unwrap();
        store
            .record_container("svc-3", &spec.id, "nginx", 3, "Failed", "")
            .unwrap();

        let containers = store.get_service_containers(&spec.id).unwrap();
        assert_eq!(containers.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_container_state() {
        let (store, dir) = setup_store();
        let spec = make_spec("state-test", "nginx", 1);
        store.create_service(&spec).unwrap();
        store
            .record_container("c1", &spec.id, "nginx", 1, "Running", "")
            .unwrap();
        store.update_container_state("c1", "Stopped").unwrap();

        let containers = store.get_service_containers(&spec.id).unwrap();
        // "Stopped" is not a known variant, maps to ContainerState::Unknown
        assert_eq!(containers[0].state, ContainerState::Unknown);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_set_autoscale() {
        let (store, dir) = setup_store();
        let spec = make_spec("as-test", "nginx", 1);
        store.create_service(&spec).unwrap();

        let config = AutoscalingConfig {
            min_replicas: 2,
            max_replicas: 10,
            cpu_target_percent: Some(70.0),
            memory_target_percent: None,
            cooldown_seconds: 60,
        };
        store.set_autoscale(&spec.id, &config, false).unwrap();

        let (loaded, paused) = store.get_autoscale(&spec.id).unwrap().unwrap();
        assert_eq!(loaded.min_replicas, 2);
        assert_eq!(loaded.max_replicas, 10);
        assert_eq!(loaded.cpu_target_percent, Some(70.0));
        assert!(!paused);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pause_autoscale() {
        let (store, dir) = setup_store();
        let spec = make_spec("pause-test", "nginx", 1);
        store.create_service(&spec).unwrap();

        let config = AutoscalingConfig {
            min_replicas: 1,
            max_replicas: 5,
            cpu_target_percent: None,
            memory_target_percent: None,
            cooldown_seconds: 30,
        };
        store.set_autoscale(&spec.id, &config, true).unwrap();

        let (_, paused) = store.get_autoscale(&spec.id).unwrap().unwrap();
        assert!(paused);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_autoscale_not_found() {
        let (store, dir) = setup_store();
        let result = store.get_autoscale("no-such-service").unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_duplicate_service_name() {
        let (store, dir) = setup_store();
        let spec1 = make_spec("unique-svc", "nginx", 1);
        store.create_service(&spec1).unwrap();

        let spec2 = make_spec("unique-svc", "alpine", 1);
        let result = store.create_service(&spec2);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_services_empty() {
        let (store, dir) = setup_store();
        let services = store.list_services().unwrap();
        assert!(services.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_multiple_services() {
        let (store, dir) = setup_store();
        store
            .create_service(&make_spec("svc-a", "nginx", 1))
            .unwrap();
        store
            .create_service(&make_spec("svc-b", "redis", 1))
            .unwrap();
        store
            .create_service(&make_spec("svc-c", "postgres", 1))
            .unwrap();

        let services = store.list_services().unwrap();
        assert_eq!(services.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
