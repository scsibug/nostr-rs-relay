use crate::db::QueryResult;
use crate::error::Result;
use crate::event::Event;
use crate::nip05::VerificationRecord;
use crate::payment::{InvoiceInfo, InvoiceStatus};
use crate::subscription::Subscription;
use crate::utils::unix_time;
use async_trait::async_trait;
use nostr::Keys;
use rand::Rng;

pub mod postgres;
pub mod postgres_migration;
pub mod sqlite;
pub mod sqlite_migration;

#[async_trait]
pub trait NostrRepo: Send + Sync {
    /// Start the repository (any initialization or maintenance tasks can be kicked off here)
    async fn start(&self) -> Result<()>;

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

    /// Create a new account
    async fn create_account(&self, pubkey: &Keys) -> Result<bool>;

    /// Admit an account
    async fn admit_account(&self, pubkey: &Keys, admission_cost: u64) -> Result<()>;

    /// Gets user balance if they are an admitted pubkey
    async fn get_account_balance(&self, pubkey: &Keys) -> Result<(bool, u64)>;

    /// Update account balance
    async fn update_account_balance(
        &self,
        pub_key: &Keys,
        positive: bool,
        new_balance: u64,
    ) -> Result<()>;

    /// Create invoice record
    async fn create_invoice_record(&self, pubkey: &Keys, invoice_info: InvoiceInfo) -> Result<()>;

    /// Update Invoice for given payment hash
    async fn update_invoice(&self, payment_hash: &str, status: InvoiceStatus) -> Result<String>;

    /// Get the most recent invoice for a given pubkey
    /// invoice must be unpaid and not expired
    async fn get_unpaid_invoice(&self, pubkey: &Keys) -> Result<Option<InvoiceInfo>>;
}

// Current time, with a slight forward jitter in seconds
pub(crate) fn now_jitter(sec: u64) -> u64 {
    // random time between now, and 10min in future.
    let mut rng = rand::thread_rng();
    let jitter_amount = rng.gen_range(0..sec);
    let now = unix_time();
    now.saturating_add(jitter_amount)
}
