pub use crate::cluster::SnapshotWire;

use crate::storage::TypeConfig;

pub type AppendEntriesReq = openraft::raft::AppendEntriesRequest<TypeConfig>;
pub type AppendEntriesResp = openraft::raft::AppendEntriesResponse<TypeConfig>;
pub type VoteReq = openraft::raft::VoteRequest<TypeConfig>;
pub type VoteResp = openraft::raft::VoteResponse<TypeConfig>;
pub type SnapshotResp = openraft::raft::SnapshotResponse<TypeConfig>;
