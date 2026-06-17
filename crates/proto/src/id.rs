use uuid::Uuid;

pub type ServiceId = String;
pub type ContainerId = String;
pub type NodeId = String;

pub fn new_service_id() -> String {
    format!("svc_{}", &Uuid::new_v4().to_string()[..8])
}

pub fn new_container_id(service_name: &str, seq: u32) -> String {
    format!("{}-{}", service_name, seq)
}

pub fn new_node_id() -> String {
    format!("node_{}", &Uuid::new_v4().to_string()[..8])
}

pub fn new_session_id() -> String {
    format!("ses_{}", &Uuid::new_v4().to_string()[..8])
}
