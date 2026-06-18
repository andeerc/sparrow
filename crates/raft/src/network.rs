use std::future::Future;

use openraft::{
    BasicNode, RaftNetworkFactory, RaftNetworkV2,
    errors::{RPCError, ReplicationClosed, StreamingError, Unreachable},
    network::RPCOption,
    raft::{AppendEntriesRequest, AppendEntriesResponse, SnapshotResponse, VoteRequest, VoteResponse},
    type_config::alias::{SnapshotOf, VoteOf},
    OptionalSend,
};

use crate::storage::TypeConfig;

/// Network connection to a specific Raft peer.
/// Stub - returns Unreachable until TCP transport is implemented (Phase 2).
pub struct NetworkConnection;

impl RaftNetworkV2<TypeConfig> for NetworkConnection {
    fn append_entries(
        &mut self,
        _rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> impl Future<Output = Result<AppendEntriesResponse<TypeConfig>, RPCError<TypeConfig>>> + Send
    {
        async move {
            Err(RPCError::Unreachable(Unreachable::from_string(
                "Raft network transport not yet implemented (Phase 2)",
            )))
        }
    }

    fn vote(
        &mut self,
        _rpc: VoteRequest<TypeConfig>,
        _option: RPCOption,
    ) -> impl Future<Output = Result<VoteResponse<TypeConfig>, RPCError<TypeConfig>>> + Send
    {
        async move {
            Err(RPCError::Unreachable(Unreachable::from_string(
                "Raft network transport not yet implemented (Phase 2)",
            )))
        }
    }

    fn full_snapshot(
        &mut self,
        _vote: VoteOf<TypeConfig>,
        _snapshot: SnapshotOf<TypeConfig>,
        _cancel: impl Future<Output = ReplicationClosed> + OptionalSend + 'static,
        _option: RPCOption,
    ) -> impl Future<Output = Result<SnapshotResponse<TypeConfig>, StreamingError<TypeConfig>>> + Send
    {
        async move {
            Err(StreamingError::Unreachable(Unreachable::from_string(
                "Raft network transport not yet implemented (Phase 2)",
            )))
        }
    }
}

/// Factory that creates network connections to peer Raft nodes.
pub struct NetworkFactory;

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkConnection;

    fn new_client(
        &mut self,
        _target: u64,
        _node: &BasicNode,
    ) -> impl Future<Output = Self::Network> + Send {
        async move { NetworkConnection }
    }
}
