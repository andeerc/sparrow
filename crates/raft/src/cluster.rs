/// Raft cluster consensus.
/// Stub for Fase 0. Full implementation in Fase 2.
pub struct RaftCluster;

impl RaftCluster {
    pub fn new() -> Self {
        Self
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        tracing::info!("Raft: cluster init (stub - Fase 2)");
        Ok(())
    }

    pub async fn join(&self, _addr: &str, _token: &str) -> anyhow::Result<()> {
        tracing::info!("Raft: cluster join (stub - Fase 2)");
        Ok(())
    }
}
