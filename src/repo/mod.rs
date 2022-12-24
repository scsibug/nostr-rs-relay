use crate::db::QueryResult;
use crate::error::Result;
use crate::event::Event;
use crate::nip05::VerificationRecord;
use crate::subscription::Subscription;
use async_trait::async_trait;
use sqlx::Postgres;

pub(crate) mod sqlite;
mod sqlite_migration;

pub(crate) mod postgres;
mod postgres_migration;
mod common;

pub type PostgresPool = sqlx::pool::Pool<Postgres>;

#[async_trait]
pub trait NostrRepo : Send + Sync {
    /// Run migrations and return current version
    async fn migrate_up(&self) -> Result<usize>;

    /// Persist event to database
    async fn write_event(&self, e: &Event) -> Result<u64>;

    /// Perform a database query using a subscription.
    ///
    /// The [`Subscription`] is converted into a SQL query.  Each result
    /// is published on the `query_tx` channel as it is returned.  If a
    /// message becomes available on the `abandon_query_rx` channel, the
    /// query is immediately aborted.
    async fn query_subscription(
        &self,
        sub: Subscription,
        client_id: String,
        query_tx: tokio::sync::mpsc::Sender<QueryResult>,
        mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()>;

    /// Perform normal maintenance
    async fn optimize_db(&self) -> Result<()>;

    /// Create a new verification record connected to a specific event
    async fn create_verification_record(&self, event_id: &str, name: &str) -> Result<()>;

    /// Update verification timestamp
    async fn update_verification_timestamp(&self, id: u64) -> Result<()>;

    /// Update verification record as failed
    async fn fail_verification(&self, id: u64) -> Result<()>;

    /// Delete verification record
    async fn delete_verification(&self, id: u64) -> Result<()>;

    /// Get the latest verification record for a given pubkey.
    async fn get_latest_user_verification(&self, pub_key: &str) -> Result<VerificationRecord>;

    /// Get oldest verification before timestamp
    async fn get_oldest_user_verification(&self, before: u64) -> Result<VerificationRecord>;
}
