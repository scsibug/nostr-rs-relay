use serde::Serialize;

#[derive(Serialize)]
pub struct KindStatistics {
    pub kind: u64,
    pub count: usize,
    pub last_created_at: u64,
}

#[derive(Serialize)]
pub struct AccountStatistics {
    pub kinds: Vec<KindStatistics>,
}
