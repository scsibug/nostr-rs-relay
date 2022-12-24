use crate::db::QueryResult;
use crate::error::Result;
use crate::event::Event;
use crate::nip05::VerificationRecord;
use crate::repo::{NostrRepo, PostgresPool};
use crate::subscription::Subscription;
use async_trait::async_trait;

use crate::repo::postgres_migration::run_migrations;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;

#[derive(Clone)]
pub struct PostgresRepo {
    conn: PostgresPool
}

impl PostgresRepo {
    pub fn new(c: PostgresPool) -> PostgresRepo {
        PostgresRepo { conn: c }
    }
}

#[async_trait]
impl NostrRepo for PostgresRepo {
    async fn migrate_up(&self) -> Result<usize> {
        run_migrations(&self.conn).await
    }

    async fn write_event(&self, e: &Event) -> Result<u64> {
        todo!()
    }

    async fn query_subscription(
        &self,
        sub: Subscription,
        client_id: String,
        query_tx: Sender<QueryResult>,
        abandon_query_rx: Receiver<()>,
    ) -> Result<()> {
        todo!()
    }

    async fn optimize_db(&self) -> Result<()> {
        todo!()
    }

    async fn create_verification_record(&self, event_id: &str, name: &str) -> Result<()> {
        todo!()
    }

    async fn update_verification_timestamp(&self, id: u64) -> Result<()> {
        todo!()
    }

    async fn fail_verification(&self, id: u64) -> Result<()> {
        todo!()
    }

    async fn delete_verification(&self, id: u64) -> Result<()> {
        todo!()
    }

    async fn get_latest_user_verification(&self, pub_key: &str) -> Result<VerificationRecord> {
        todo!()
    }

    async fn get_oldest_user_verification(&self, before: u64) -> Result<VerificationRecord> {
        todo!()
    }
}
