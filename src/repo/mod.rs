use crate::error::{Result};
use crate::event::{Event};
use crate::subscription::{Subscription};
use crate::db::{QueryResult};
use async_trait::async_trait;
use crate::nip05::VerificationRecord;

pub mod sqlite;

#[async_trait]
pub trait RepoMigrate {
    /// Run migrations
    async fn migrate_up(&mut self);
}

#[async_trait]
pub trait NostrRepo: RepoMigrate {
    /// Persist event to database
    async fn write_event(&mut self, e: &Event) -> Result<u64>;

    /// Perform a database query using a subscription.
    ///
    /// The [`Subscription`] is converted into a SQL query.  Each result
    /// is published on the `query_tx` channel as it is returned.  If a
    /// message becomes available on the `abandon_query_rx` channel, the
    /// query is immediately aborted.
    async fn query_subscription(
        &mut self,
        sub: Subscription,
        client_id: String,
        query_tx: tokio::sync::mpsc::Sender<QueryResult>,
        mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()>;

    /// Perform normal maintenance
    async fn optimize_db(&mut self) -> Result<()>;
}

#[async_trait]
pub trait Nip05Repo : RepoMigrate {
    /// Create a new verification record connected to a specific event
    async fn create_verification_record(&mut self, event_id: &str, name: &str) -> Result<()>;

    /// Update verification timestamp
    async fn update_verification_timestamp(&mut self, id: u64) -> Result<()>;

    /// Update verification record as failed
    async fn fail_verification(&mut self, id: u64) -> Result<()>;

    /// Delete verification record
    async fn delete_verification(&mut self, id: u64) -> Result<()>;

    /// Get the latest verification record for a given pubkey.
    async fn get_latest_user_verification(&mut self, pub_key: &str) -> Result<VerificationRecord>;

    /// Get oldest verification before timestamp
    async fn get_oldest_user_verification(&mut self, before: u64) -> Result<VerificationRecord>;
}
