use async_trait::async_trait;
use crate::db::QueryResult;
use crate::event::Event;
use crate::nip05::VerificationRecord;
use crate::repo::{Nip05Repo, NostrRepo, RepoMigrate};
use crate::subscription::Subscription;
use sqlx::Postgres;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;

pub type PostgresPool = sqlx::pool::Pool<Postgres>;

struct PostgresRepo {
    conn: PostgresPool,
}

impl PostgresRepo {
    fn new(c: PostgresPool) -> PostgresRepo {
        PostgresRepo { conn: c }
    }
}

#[async_trait]
impl RepoMigrate for PostgresRepo {
    async fn migrate_up(&mut self) -> crate::error::Result<usize> {
        todo!()
    }
}

#[async_trait]
impl NostrRepo for PostgresRepo {
    async fn write_event(&mut self, e: &Event) -> crate::error::Result<u64> {
        todo!()
    }

    async fn query_subscription(
        &mut self,
        sub: Subscription,
        client_id: String,
        query_tx: Sender<QueryResult>,
        abandon_query_rx: Receiver<()>,
    ) -> crate::error::Result<()> {
        todo!()
    }

    async fn optimize_db(&mut self) -> crate::error::Result<()> {
        todo!()
    }
}

#[async_trait]
impl Nip05Repo for PostgresRepo {
    async fn create_verification_record(
        &mut self,
        event_id: &str,
        name: &str,
    ) -> crate::error::Result<()> {
        todo!()
    }

    async fn update_verification_timestamp(&mut self, id: u64) -> crate::error::Result<()> {
        todo!()
    }

    async fn fail_verification(&mut self, id: u64) -> crate::error::Result<()> {
        todo!()
    }

    async fn delete_verification(&mut self, id: u64) -> crate::error::Result<()> {
        todo!()
    }

    async fn get_latest_user_verification(
        &mut self,
        pub_key: &str,
    ) -> crate::error::Result<VerificationRecord> {
        todo!()
    }

    async fn get_oldest_user_verification(
        &mut self,
        before: u64,
    ) -> crate::error::Result<VerificationRecord> {
        todo!()
    }
}
