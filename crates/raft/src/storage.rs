use std::fmt;
use std::io::{self, Cursor};
use std::ops::RangeBounds;
use std::sync::{Arc, Mutex};

use futures_util::Stream;
use openraft::{
    self,
    entry::{RaftEntry, RaftPayload},
    storage::{
        EntryResponder, IOFlushed, LogState, RaftLogReader, RaftLogStorage, RaftSnapshotBuilder,
        RaftStateMachine,
    },
    type_config::alias,
    OptionalSend, Snapshot, StoredMembership,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::types::{RaftRequest, RaftResponse};

openraft::declare_raft_types!(
    pub TypeConfig: D = RaftRequest, R = RaftResponse
);

type C = TypeConfig;
type E = alias::EntryOf<C>;
type LE = alias::LogIdOf<C>;
type VO = alias::VoteOf<C>;
type SO = alias::SnapshotOf<C>;
type Smo = alias::SnapshotMetaOf<C>;
type Sdo = alias::SnapshotDataOf<C>;
type StoredM = alias::StoredMembershipOf<C>;

#[derive(Clone)]
pub struct StoredRaftLog {
    db: Arc<Mutex<Connection>>,
    entries: Arc<Mutex<Vec<E>>>,
    last_purged: Arc<Mutex<Option<LE>>>,
}

impl StoredRaftLog {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS raft_hard_state (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 term INTEGER NOT NULL DEFAULT 0,
                 node_id INTEGER NOT NULL DEFAULT 0,
                 committed INTEGER NOT NULL DEFAULT 0
             );
             INSERT OR IGNORE INTO raft_hard_state (id, term, node_id, committed) VALUES (1, 0, 0, 0);
             CREATE TABLE IF NOT EXISTS raft_committed (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 log_index INTEGER
             );
             INSERT OR IGNORE INTO raft_committed (id, log_index) VALUES (1, NULL);",
        )?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            entries: Arc::new(Mutex::new(Vec::new())),
            last_purged: Arc::new(Mutex::new(None)),
        })
    }
}

impl RaftLogReader<C> for StoredRaftLog {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + fmt::Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<E>, io::Error> {
        let entries = self.entries.lock().unwrap();
        let base = entries.first().map(|e| e.index()).unwrap_or(0);
        let len = entries.len() as u64;

        let raw_start = match range.start_bound() {
            std::ops::Bound::Included(s) => *s,
            std::ops::Bound::Excluded(s) => *s + 1,
            std::ops::Bound::Unbounded => base,
        };
        let raw_end = match range.end_bound() {
            std::ops::Bound::Included(e) => *e + 1,
            std::ops::Bound::Excluded(e) => *e,
            std::ops::Bound::Unbounded => base + len,
        };
        let start = raw_start.saturating_sub(base) as usize;
        let end = (raw_end.saturating_sub(base) as usize).min(entries.len());
        if start >= entries.len() {
            return Ok(Vec::new());
        }
        Ok(entries[start..end].to_vec())
    }

    async fn read_vote(&mut self) -> Result<Option<VO>, io::Error> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT term, node_id, committed FROM raft_hard_state WHERE id = 1")
            .map_err(io::Error::other)?;

        let result: Result<(u64, u64, i32), _> =
            stmt.query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)));

        match result {
            Ok((term, node_id, committed)) => {
                if committed != 0 {
                    Ok(Some(VO::new_committed(term, node_id)))
                } else {
                    Ok(Some(VO::new(term, node_id)))
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

impl RaftLogStorage<C> for StoredRaftLog {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<C>, io::Error> {
        let entries = self.entries.lock().unwrap();
        let last_purged = self.last_purged.lock().unwrap();

        let last_log_id = entries.last().map(|e| e.log_id());
        let last_purged_log_id = *last_purged;

        Ok(LogState {
            last_purged_log_id,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        Self {
            db: self.db.clone(),
            entries: self.entries.clone(),
            last_purged: self.last_purged.clone(),
        }
    }

    async fn save_vote(&mut self, vote: &VO) -> Result<(), io::Error> {
        let conn = self.db.lock().unwrap();
        let term: u64 = vote.leader_id().term;
        let node_id: u64 = vote.leader_id().node_id;
        let committed: i32 = if vote.is_committed() { 1 } else { 0 };
        conn.execute(
            "UPDATE raft_hard_state SET term = ?1, node_id = ?2, committed = ?3 WHERE id = 1",
            params![term, node_id, committed],
        )
        .map_err(io::Error::other)?;
        Ok(())
    }

    async fn append<I>(&mut self, entries: I, callback: IOFlushed<C>) -> Result<(), io::Error>
    where
        I: IntoIterator<Item = E> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut store = self.entries.lock().unwrap();
        for entry in entries {
            store.push(entry);
        }
        callback.io_completed(Ok(()));
        Ok(())
    }

    async fn truncate_after(&mut self, last_log_id: Option<LE>) -> Result<(), io::Error> {
        let mut entries = self.entries.lock().unwrap();
        let base = entries.first().map(|e| e.index()).unwrap_or(0);
        match last_log_id {
            Some(ref lid) => {
                let idx = lid.index;
                if idx >= base {
                    let truncate_at = (idx - base + 1) as usize;
                    entries.truncate(truncate_at);
                }
            }
            None => {
                entries.clear();
            }
        }
        Ok(())
    }

    async fn purge(&mut self, log_id: LE) -> Result<(), io::Error> {
        let mut entries = self.entries.lock().unwrap();
        let base = entries.first().map(|e| e.index()).unwrap_or(0);
        let purge_idx = log_id.index;
        if purge_idx >= base {
            let remove_up_to = (purge_idx - base + 1) as usize;
            if remove_up_to < entries.len() {
                entries.drain(0..remove_up_to);
            } else {
                entries.clear();
            }
        }
        let mut last_purged = self.last_purged.lock().unwrap();
        *last_purged = Some(log_id);
        Ok(())
    }

    async fn save_committed(&mut self, committed: Option<LE>) -> Result<(), io::Error> {
        let conn = self.db.lock().unwrap();
        let idx: Option<u64> = committed.map(|lid| lid.index);
        conn.execute(
            "UPDATE raft_committed SET log_index = ?1 WHERE id = 1",
            params![idx],
        )
        .map_err(io::Error::other)?;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LE>, io::Error> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT log_index FROM raft_committed WHERE id = 1")
            .map_err(io::Error::other)?;

        let idx: Result<Option<u64>, _> = stmt.query_row([], |row| row.get(0));
        match idx {
            Ok(Some(index)) => {
                let entries = self.entries.lock().unwrap();
                let base = entries.first().map(|e| e.index()).unwrap_or(0);
                if index >= base && (index - base) < entries.len() as u64 {
                    let log_id = entries[(index - base) as usize].log_id();
                    Ok(Some(log_id))
                } else {
                    Ok(None)
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

pub struct StoredStateMachine {
    #[allow(dead_code)]
    db: Arc<Mutex<Connection>>,
    last_applied: Arc<Mutex<Option<LE>>>,
    last_membership: Arc<Mutex<StoredM>>,
    snapshot_meta: Arc<Mutex<Option<Smo>>>,
    snapshot_data: Arc<Mutex<Option<Sdo>>>,
}

impl StoredStateMachine {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS raft_sm_state (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 last_applied_term INTEGER,
                 last_applied_idx INTEGER,
                 last_applied_leader_node_id INTEGER
             );
             INSERT OR IGNORE INTO raft_sm_state (id) VALUES (1);",
        )?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            last_applied: Arc::new(Mutex::new(None)),
            last_membership: Arc::new(Mutex::new(StoredMembership::default())),
            snapshot_meta: Arc::new(Mutex::new(None)),
            snapshot_data: Arc::new(Mutex::new(None)),
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct SnapshotState {
    pub last_applied_term: u64,
    pub last_applied_index: u64,
    pub last_applied_leader_node_id: u64,
    pub membership_term: u64,
    pub membership_node_ids: Vec<u64>,
}

pub struct SnapshotBuilder {
    pub db: Arc<Mutex<Connection>>,
    pub last_applied: Arc<Mutex<Option<LE>>>,
    pub last_membership: Arc<Mutex<StoredM>>,
}

impl RaftSnapshotBuilder<C> for SnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<SO, io::Error> {
        let la = *self.last_applied.lock().unwrap();
        let membership = self.last_membership.lock().unwrap().clone();

        let mut members = Vec::new();
        for config in membership.get_joint_config() {
            for nid in config.iter() {
                members.push(*nid);
            }
        }

        let last_applied = la.unwrap_or_else(|| LE::new(*VO::new(0, 0).leader_id(), 0));

        let state = SnapshotState {
            last_applied_term: last_applied.committed_leader_id().term,
            last_applied_index: last_applied.index,
            last_applied_leader_node_id: last_applied.committed_leader_id().node_id,
            membership_term: last_applied.committed_leader_id().term,
            membership_node_ids: members,
        };

        let data = bincode::serialize(&state).map_err(io::Error::other)?;

        let snapshot_id = format!(
            "{}-{}-{}",
            last_applied.committed_leader_id().term,
            last_applied.index,
            last_applied.committed_leader_id().node_id
        );

        let meta = openraft::SnapshotMeta {
            snapshot_id,
            last_log_id: la,
            last_membership: membership,
        };

        Ok(Snapshot {
            meta,
            snapshot: Cursor::new(data),
        })
    }
}

impl RaftStateMachine<C> for StoredStateMachine {
    type SnapshotBuilder = SnapshotBuilder;

    async fn applied_state(&mut self) -> Result<(Option<LE>, StoredM), io::Error> {
        let la = *self.last_applied.lock().unwrap();
        let mem = self.last_membership.lock().unwrap().clone();
        Ok((la, mem))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<(), io::Error>
    where
        I: Stream<Item = Result<EntryResponder<C>, io::Error>> + Unpin + OptionalSend,
    {
        use futures_util::StreamExt;

        tokio::pin!(entries);
        while let Some(result) = entries.next().await {
            let (entry, responder) = result?;
            let log_id = entry.log_id();

            if let Some(membership) = entry.get_membership() {
                let mut mem = self.last_membership.lock().unwrap();
                *mem = StoredMembership::new(Some(log_id), membership);
            }

            *self.last_applied.lock().unwrap() = Some(log_id);

            if let Some(responder) = responder {
                responder.send(RaftResponse {
                    success: true,
                    data: Vec::new(),
                    error: None,
                });
            }
        }
        Ok(())
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Sdo, io::Error> {
        Ok(Cursor::new(Vec::new()))
    }

    async fn install_snapshot(&mut self, meta: &Smo, snapshot: Sdo) -> Result<(), io::Error> {
        let data: Vec<u8> = snapshot.into_inner();
        *self.snapshot_meta.lock().unwrap() = Some(meta.clone());
        *self.snapshot_data.lock().unwrap() = Some(Cursor::new(data));

        if let Some(ref last_log_id) = meta.last_log_id {
            *self.last_applied.lock().unwrap() = Some(*last_log_id);
        }
        *self.last_membership.lock().unwrap() = meta.last_membership.clone();
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<SO>, io::Error> {
        let meta = self.snapshot_meta.lock().unwrap().clone();
        let data = self.snapshot_data.lock().unwrap().clone();
        match (meta, data) {
            (Some(meta), Some(data)) => {
                let snap = Snapshot {
                    meta,
                    snapshot: data,
                };
                Ok(Some(snap))
            }
            _ => Ok(None),
        }
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        SnapshotBuilder {
            db: self.db.clone(),
            last_applied: self.last_applied.clone(),
            last_membership: self.last_membership.clone(),
        }
    }
}
