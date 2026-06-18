use chrono::Utc;
use rusqlite::{params, Connection};
use sparrow_proto::*;
use std::path::Path;
use std::sync::Mutex;

pub struct StateStore {
    conn: Mutex<Connection>,
}

impl StateStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
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
                node_id     TEXT NOT NULL DEFAULT ''
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
        Ok(())
    }

    // ── Service CRUD ──

    pub fn create_service(&self, spec: &ServiceSpec) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
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
            stmt.execute(params![spec.id, p.published, p.target, format!("{:?}", p.protocol)])?;
        }

        // Env vars
        let mut stmt = conn.prepare(
            "INSERT INTO service_env (service_id, key, value) VALUES (?1, ?2, ?3)",
        )?;
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
        let mut stmt = conn.prepare(
            "INSERT INTO service_networks (service_id, network) VALUES (?1, ?2)",
        )?;
        for n in &spec.networks {
            stmt.execute(params![spec.id, n])?;
        }

        Ok(())
    }

    pub fn list_services(&self) -> anyhow::Result<Vec<ServiceSpec>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, image, desired_replicas, restart_policy, created_at, updated_at FROM services ORDER BY created_at DESC",
        )?;

        let services = stmt
            .query_map([], |row| {
                let created: String = row.get(5)?;
                let updated: String = row.get(6)?;
                Ok(ServiceSpec {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    image: row.get(2)?,
                    desired_replicas: row.get(3)?,
                    ports: vec![],
                    env: vec![],
                    volumes: vec![],
                    networks: vec![],
                    labels: std::collections::HashMap::new(),
                    resources: None,
                    restart_policy: RestartPolicy::Always,
                    command: None,
                    autoscaling: None,
                    created_at: created.parse().unwrap_or_else(|_| Utc::now()),
                    updated_at: updated.parse().unwrap_or_else(|_| Utc::now()),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(services)
    }

    pub fn get_service(&self, id_or_name: &str) -> anyhow::Result<Option<ServiceSpec>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, image, desired_replicas, restart_policy, created_at, updated_at
             FROM services WHERE id = ?1 OR name = ?1",
        )?;

        let mut rows = stmt.query_map(params![id_or_name], |row| {
            let created: String = row.get(5)?;
            let updated: String = row.get(6)?;
            Ok(ServiceSpec {
                id: row.get(0)?,
                name: row.get(1)?,
                image: row.get(2)?,
                desired_replicas: row.get(3)?,
                ports: vec![],
                env: vec![],
                volumes: vec![],
                networks: vec![],
                labels: std::collections::HashMap::new(),
                resources: None,
                restart_policy: RestartPolicy::Always,
                command: None,
                autoscaling: None,
                created_at: created.parse().unwrap_or_else(|_| Utc::now()),
                updated_at: updated.parse().unwrap_or_else(|_| Utc::now()),
            })
        })?;

        match rows.next() {
            Some(Ok(svc)) => Ok(Some(svc)),
            _ => Ok(None),
        }
    }

    pub fn delete_service(&self, id_or_name: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        // First get the service id if name was given
        let svc = {
            let mut stmt = conn.prepare("SELECT id FROM services WHERE id = ?1 OR name = ?1")?;
            let mut rows = stmt.query_map(params![id_or_name], |row| row.get::<_, String>(0))?;
            rows.next().transpose()?
        };

        if let Some(svc_id) = svc {
            conn.execute("DELETE FROM services WHERE id = ?1", params![svc_id])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn update_replicas(&self, id_or_name: &str, replicas: u32) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
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
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO containers (container_name, service_id, image, replica_seq, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![container_name, service_id, image, replica_seq, state, now],
        )?;
        Ok(())
    }

    pub fn update_container_state(&self, container_name: &str, state: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE containers SET state = ?1 WHERE container_name = ?2",
            params![state, container_name],
        )?;
        Ok(affected > 0)
    }

    pub fn get_service_containers(&self, service_id: &str) -> anyhow::Result<Vec<ContainerStatus>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT container_name, service_id, image, replica_seq, state, created_at, node_id
             FROM containers WHERE service_id = ?1 ORDER BY replica_seq",
        )?;

        let containers = stmt
            .query_map(params![service_id], |row| {
                let state_str: String = row.get(4)?;
                let created: String = row.get(5)?;
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
                    ip_address: None,
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
        let conn = self.conn.lock().unwrap();
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

    pub fn get_autoscale(&self, service_id: &str) -> anyhow::Result<Option<(AutoscalingConfig, bool)>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO autoscale_events (service_id, decision, replicas_from, replicas_to, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![service_id, decision, replicas_from, replicas_to, reason, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn list_autoscale_events(&self, service_id: &str, limit: u32) -> anyhow::Result<Vec<AutoscaleEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT decision, replicas_from, replicas_to, reason, created_at
             FROM autoscale_events WHERE service_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let events = stmt.query_map(params![service_id, limit], |row| {
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, metric, operator, threshold, duration_secs, enabled, created_at
             FROM alert_rules ORDER BY created_at DESC",
        )?;
        let rules = stmt.query_map([], |row| {
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, channel_type, metric, value, threshold, message, severity, status, created_at
             FROM alert_events ORDER BY created_at DESC LIMIT ?1",
        )?;
        let events = stmt.query_map(params![limit], |row| {
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alert_channels (id, channel_type, name, config_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                channel_type,
                name,
                config_json,
                enabled as i32,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_alert_channels(&self) -> anyhow::Result<Vec<AlertChannelRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_type, name, config_json, enabled, created_at
             FROM alert_channels ORDER BY created_at DESC",
        )?;
        let channels = stmt.query_map([], |row| {
            Ok(AlertChannelRecord {
                id: row.get(0)?,
                channel_type: row.get(1)?,
                name: row.get(2)?,
                config_json: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
        Ok(channels)
    }
}
