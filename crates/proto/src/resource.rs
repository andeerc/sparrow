use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSpec {
    pub cpu_limit: String,
    pub mem_limit: String,
    pub cpu_reservation: Option<String>,
    pub mem_reservation: Option<String>,
}
