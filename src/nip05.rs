//! User verification using NIP-05 names
//!
//! NIP-05 defines a mechanism for authors to associate an internet
//! address with their public key, in metadata events.  This module
//! consumes a stream of metadata events, and keeps a database table
//! updated with the current NIP-05 verification status.
use crate::config::VerifiedUsers;
use crate::db;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::utils::unix_time;
use hyper::body::HttpBody;
use hyper::client::connect::HttpConnector;
use hyper::Client;
use hyper_tls::HttpsConnector;
use rand::Rng;
use rusqlite::params;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use tokio::time::Interval;
use tracing::{debug, info, warn};

/// NIP-05 verifier state
pub struct Verifier {
    /// Metadata events for us to inspect
    metadata_rx: tokio::sync::broadcast::Receiver<Event>,
    /// Newly validated events get written and then broadcast on this channel to subscribers
    event_tx: tokio::sync::broadcast::Sender<Event>,
    /// SQLite read query pool
    read_pool: db::SqlitePool,
    /// SQLite write query pool
    write_pool: db::SqlitePool,
    /// Settings
    settings: crate::config::Settings,
    /// HTTP client
    client: hyper::Client<HttpsConnector<HttpConnector>, hyper::Body>,
    /// After all accounts are updated, wait this long before checking again.
    wait_after_finish: Duration,
    /// Minimum amount of time between HTTP queries
    http_wait_duration: Duration,
    /// Interval for updating verification records
    reverify_interval: Interval,
}

/// A NIP-05 identifier is a local part and domain.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct Nip05Name {
    local: String,
    domain: String,
}

impl Nip05Name {
    /// Does this name represent the entire domain?
    pub fn is_domain_only(&self) -> bool {
        self.local == "_"
    }

    /// Determine the URL to query for verification
    fn to_url(&self) -> Option<http::Uri> {
        format!(
            "https://{}/.well-known/nostr.json?name={}",
            self.domain, self.local
        )
        .parse::<http::Uri>()
        .ok()
    }
}

// Parsing Nip05Names from strings
impl std::convert::TryFrom<&str> for Nip05Name {
    type Error = Error;
    fn try_from(inet: &str) -> Result<Self, Self::Error> {
        // break full name at the @ boundary.
        let components: Vec<&str> = inet.split('@').collect();
        if components.len() != 2 {
            Err(Error::CustomError("too many/few components".to_owned()))
        } else {
            // check if local name is valid
            let local = components[0];
            let domain = components[1];
            if local
                .chars()
                .all(|x| x.is_alphanumeric() || x == '_' || x == '-' || x == '.')
            {
                if domain
                    .chars()
                    .all(|x| x.is_alphanumeric() || x == '-' || x == '.')
                {
                    Ok(Nip05Name {
                        local: local.to_owned(),
                        domain: domain.to_owned(),
                    })
                } else {
                    Err(Error::CustomError(
                        "invalid character in domain part".to_owned(),
                    ))
                }
            } else {
                Err(Error::CustomError(
                    "invalid character in local part".to_owned(),
                ))
            }
        }
    }
}

impl std::fmt::Display for Nip05Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.local, self.domain)
    }
}

// Current time, with a slight foward jitter in seconds
fn now_jitter(sec: u64) -> u64 {
    // random time between now, and 10min in future.
    let mut rng = rand::thread_rng();
    let jitter_amount = rng.gen_range(0..sec);
    let now = unix_time();
    now.saturating_add(jitter_amount)
}

/// Check if the specified username and address are present and match in this response body
fn body_contains_user(username: &str, address: &str, bytes: hyper::body::Bytes) -> Result<bool> {
    // convert the body into json
    let body: serde_json::Value = serde_json::from_slice(&bytes)?;
    // ensure we have a names object.
    let names_map = body
        .as_object()
        .and_then(|x| x.get("names"))
        .and_then(|x| x.as_object())
        .ok_or_else(|| Error::CustomError("not a map".to_owned()))?;
    // get the pubkey for the requested user
    let check_name = names_map.get(username).and_then(|x| x.as_str());
    // ensure the address is a match
    Ok(check_name.map(|x| x == address).unwrap_or(false))
}

impl Verifier {
    pub fn new(
        metadata_rx: tokio::sync::broadcast::Receiver<Event>,
        event_tx: tokio::sync::broadcast::Sender<Event>,
        settings: crate::config::Settings,
    ) -> Result<Self> {
        info!("creating NIP-05 verifier");
        // build a database connection for reading and writing.
        let write_pool = db::build_pool(
            "nip05 writer",
            &settings,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            1,    // min conns
            4,    // max conns
            true, // wait for DB
        );
        let read_pool = db::build_pool(
            "nip05 reader",
            &settings,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            1,    // min conns
            8,    // max conns
            true, // wait for DB
        );
        // setup hyper client
        let https = HttpsConnector::new();
        let client = Client::builder().build::<_, hyper::Body>(https);

        // After all accounts have been re-verified, don't check again
        // for this long.
        let wait_after_finish = Duration::from_secs(60 * 10);
        // when we have an active queue of accounts to validate, we
        // will wait this duration between HTTP requests.
        let http_wait_duration = Duration::from_secs(1);
        // setup initial interval for re-verification.  If we find
        // there is no work to be done, it will be reset to a longer
        // duration.
        let reverify_interval = tokio::time::interval(http_wait_duration);
        Ok(Verifier {
            metadata_rx,
            event_tx,
            read_pool,
            write_pool,
            settings,
            client,
            wait_after_finish,
            http_wait_duration,
            reverify_interval,
        })
    }

    /// Perform web verification against a NIP-05 name and address.
    pub async fn get_web_verification(
        &mut self,
        nip: &Nip05Name,
        pubkey: &str,
    ) -> UserWebVerificationStatus {
        self.get_web_verification_res(nip, pubkey)
            .await
            .unwrap_or(UserWebVerificationStatus::Unknown)
    }

    /// Perform web verification against an `Event` (must be metadata).
    pub async fn get_web_verification_from_event(
        &mut self,
        e: &Event,
    ) -> UserWebVerificationStatus {
        let nip_parse = e.get_nip05_addr();
        if let Some(nip) = nip_parse {
            self.get_web_verification_res(&nip, &e.pubkey)
                .await
                .unwrap_or(UserWebVerificationStatus::Unknown)
        } else {
            UserWebVerificationStatus::Unknown
        }
    }

    /// Perform web verification, with a `Result` return.
    async fn get_web_verification_res(
        &mut self,
        nip: &Nip05Name,
        pubkey: &str,
    ) -> Result<UserWebVerificationStatus> {
        // determine if this domain should be checked
        if !is_domain_allowed(
            &nip.domain,
            &self.settings.verified_users.domain_whitelist,
            &self.settings.verified_users.domain_blacklist,
        ) {
            return Ok(UserWebVerificationStatus::DomainNotAllowed);
        }
        let url = nip
            .to_url()
            .ok_or_else(|| Error::CustomError("invalid NIP-05 URL".to_owned()))?;
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(url)
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                format!(
                    "nostr-rs-relay/{} NIP-05 Verifier",
                    crate::info::CARGO_PKG_VERSION.unwrap()
                ),
            )
            .body(hyper::Body::empty())
            .expect("request builder");

        let response_fut = self.client.request(req);

        // HTTP request with timeout
        match tokio::time::timeout(Duration::from_secs(5), response_fut).await {
            Ok(response_res) => {
                // limit size of verification document to 1MB.
                const MAX_ALLOWED_RESPONSE_SIZE: u64 = 1024 * 1024;
                let response = response_res?;
                // determine content length from response
                let response_content_length = match response.body().size_hint().upper() {
                    Some(v) => v,
                    None => MAX_ALLOWED_RESPONSE_SIZE + 1, // reject missing content length
                };
                // TODO: test how hyper handles the client providing an inaccurate content-length.
                if response_content_length <= MAX_ALLOWED_RESPONSE_SIZE {
                    let (parts, body) = response.into_parts();
                    // TODO: consider redirects
                    if parts.status == http::StatusCode::OK {
                        // parse body, determine if the username / key / address is present
                        let body_bytes = hyper::body::to_bytes(body).await?;
                        let body_matches = body_contains_user(&nip.local, pubkey, body_bytes)?;
                        if body_matches {
                            return Ok(UserWebVerificationStatus::Verified);
                        }
                        // successful response, parsed as a nip-05
                        // document, but this name/pubkey was not
                        // present.
                        return Ok(UserWebVerificationStatus::Unverified);
                    }
                } else {
                    info!(
                        "content length missing or exceeded limits for account: {:?}",
                        nip.to_string()
                    );
                }
            }
            Err(_) => {
                info!("timeout verifying account {:?}", nip);
                return Ok(UserWebVerificationStatus::Unknown);
            }
        }
        Ok(UserWebVerificationStatus::Unknown)
    }

    /// Perform NIP-05 verifier tasks.
    pub async fn run(&mut self) {
        // use this to schedule periodic re-validation tasks
        // run a loop, restarting on failure
        loop {
            let res = self.run_internal().await;
            if let Err(e) = res {
                info!("error in verifier: {:?}", e);
            }
        }
    }

    /// Internal select loop for performing verification
    async fn run_internal(&mut self) -> Result<()> {
        tokio::select! {
            m = self.metadata_rx.recv() => {
                match m {
                    Ok(e) => {
                        if let Some(naddr) = e.get_nip05_addr() {
                            info!("got metadata event for ({:?},{:?})", naddr.to_string() ,e.get_author_prefix());
                            // Process a new author, checking if they are verified:
                            let check_verified = get_latest_user_verification(self.read_pool.get().expect("could not get connection"), &e.pubkey).await;
                            // ensure the event we got is more recent than the one we have, otherwise we can ignore it.
                            if let Ok(last_check) = check_verified {
                                if e.created_at <= last_check.event_created {
                                    // this metadata is from the same author as an existing verification.
                                    // it is older than what we have, so we can ignore it.
                                    debug!("received older metadata event for author {:?}", e.get_author_prefix());
                                    return Ok(());
                                }
                            }
                            // old, or no existing record for this user.  In either case, we just create a new one.
                            let start = Instant::now();
                            let v = self.get_web_verification_from_event(&e).await;
                            info!(
                                "checked name {:?}, result: {:?}, in: {:?}",
                                naddr.to_string(),
                                v,
                                start.elapsed()
                            );
                            // sleep to limit how frequently we make HTTP requests for new metadata events.  This should limit us to 4 req/sec.
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            // if this user was verified, we need to write the
                            // record, persist the event, and broadcast.
                            if let UserWebVerificationStatus::Verified = v {
                                self.create_new_verified_user(&naddr.to_string(), &e).await?;
                            }
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(c)) => {
                        warn!("incoming metadata events overwhelmed buffer, {} events dropped",c);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("metadata broadcast channel closed");
                    }
                }
            },
            _ = self.reverify_interval.tick() => {
                // check and see if there is an old account that needs
                // to be reverified
                self.do_reverify().await?;
            },
        }
        Ok(())
    }

    /// Reverify the oldest user verification record.
    async fn do_reverify(&mut self) -> Result<()> {
        let reverify_setting = self
            .settings
            .verified_users
            .verify_update_frequency_duration;
        let max_failures = self.settings.verified_users.max_consecutive_failures;
        // get from settings, but default to 6hrs between re-checking an account
        let reverify_dur = reverify_setting.unwrap_or_else(|| Duration::from_secs(60 * 60 * 6));
        // find all verification records that have success or failure OLDER than the reverify_dur.
        let now = SystemTime::now();
        let earliest = now - reverify_dur;
        let earliest_epoch = earliest
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|x| x.as_secs())
            .unwrap_or(0);
        let vr = get_oldest_user_verification(self.read_pool.get()?, earliest_epoch).await;
        match vr {
            Ok(ref v) => {
                let new_status = self.get_web_verification(&v.name, &v.address).await;
                match new_status {
                    UserWebVerificationStatus::Verified => {
                        // freshly verified account, update the
                        // timestamp.
                        self.update_verification_record(self.write_pool.get()?, v)
                            .await?;
                    }
                    UserWebVerificationStatus::DomainNotAllowed
                    | UserWebVerificationStatus::Unknown => {
                        // server may be offline, or temporarily
                        // blocked by the config file.  Note the
                        // failure so we can process something
                        // else.

                        // have we had enough failures to give up?
                        if v.failure_count >= max_failures as u64 {
                            info!(
                                "giving up on verifying {:?} after {} failures",
                                v.name, v.failure_count
                            );
                            self.delete_verification_record(self.write_pool.get()?, v)
                                .await?;
                        } else {
                            // record normal failure, incrementing failure count
                            self.fail_verification_record(self.write_pool.get()?, v)
                                .await?;
                        }
                    }
                    UserWebVerificationStatus::Unverified => {
                        // domain has removed the verification, drop
                        // the record on our side.
                        self.delete_verification_record(self.write_pool.get()?, v)
                            .await?;
                    }
                }
            }
            Err(Error::SqlError(rusqlite::Error::QueryReturnedNoRows)) => {
                // No users need verification.  Reset the interval to
                // the next verification attempt.
                let start = tokio::time::Instant::now() + self.wait_after_finish;
                self.reverify_interval = tokio::time::interval_at(start, self.http_wait_duration);
            }
            Err(ref e) => {
                warn!(
                    "Error when checking for NIP-05 verification records: {:?}",
                    e
                );
            }
        }
        Ok(())
    }

    /// Reset the verification timestamp on a VerificationRecord
    pub async fn update_verification_record(
        &mut self,
        mut conn: db::PooledConnection,
        vr: &VerificationRecord,
    ) -> Result<()> {
        let vr_id = vr.rowid;
        let vr_str = vr.to_string();
        tokio::task::spawn_blocking(move || {
            // add some jitter to the verification to prevent everything from stacking up together.
            let verif_time = now_jitter(600);
            let tx = conn.transaction()?;
            {
                // update verification time and reset any failure count
                let query =
                    "UPDATE user_verification SET verified_at=?, failure_count=0 WHERE id=?";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![verif_time, vr_id])?;
            }
            tx.commit()?;
            info!("verification updated for {}", vr_str);
            let ok: Result<()> = Ok(());
            ok
        })
        .await?
    }
    /// Reset the failure timestamp on a VerificationRecord
    pub async fn fail_verification_record(
        &mut self,
        mut conn: db::PooledConnection,
        vr: &VerificationRecord,
    ) -> Result<()> {
        let vr_id = vr.rowid;
        let vr_str = vr.to_string();
        let fail_count = vr.failure_count.saturating_add(1);
        tokio::task::spawn_blocking(move || {
            // add some jitter to the verification to prevent everything from stacking up together.
            let fail_time = now_jitter(600);
            let tx = conn.transaction()?;
            {
                let query = "UPDATE user_verification SET failed_at=?, failure_count=? WHERE id=?";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![fail_time, fail_count, vr_id])?;
            }
            tx.commit()?;
            info!("verification failed for {}", vr_str);
            let ok: Result<()> = Ok(());
            ok
        })
        .await?
    }
    /// Delete a VerificationRecord that is no longer valid
    pub async fn delete_verification_record(
        &mut self,
        mut conn: db::PooledConnection,
        vr: &VerificationRecord,
    ) -> Result<()> {
        let vr_id = vr.rowid;
        let vr_str = vr.to_string();
        tokio::task::spawn_blocking(move || {
            let tx = conn.transaction()?;
            {
                let query = "DELETE FROM user_verification WHERE id=?;";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![vr_id])?;
            }
            tx.commit()?;
            info!("verification rescinded for {}", vr_str);
            let ok: Result<()> = Ok(());
            ok
        })
        .await?
    }

    /// Persist an event, create a verification record, and broadcast.
    // TODO: have more event-writing logic handled in the db module.
    // Right now, these events avoid the rate limit.  That is
    // acceptable since as soon as the user is registered, this path
    // is no longer used.
    // TODO: refactor these into spawn_blocking
    // calls to get them off the async executors.
    async fn create_new_verified_user(&mut self, name: &str, event: &Event) -> Result<()> {
        let start = Instant::now();
        // we should only do this if we are enabled.  if we are
        // disabled/passive, the event has already been persisted.
        let should_write_event = self.settings.verified_users.is_enabled();
        if should_write_event {
            match db::write_event(&mut self.write_pool.get()?, event) {
                Ok(updated) => {
                    if updated != 0 {
                        info!(
                            "persisted event (new verified pubkey): {:?} in {:?}",
                            event.get_event_id_prefix(),
                            start.elapsed()
                        );
                        self.event_tx.send(event.clone()).ok();
                    }
                }
                Err(err) => {
                    warn!("event insert failed: {:?}", err);
                    if let Error::SqlError(r) = err {
                        warn!("because: : {:?}", r);
                    }
                }
            }
        }
        // write the verification record
        save_verification_record(self.write_pool.get()?, event, name).await?;
        Ok(())
    }
}

/// Result of checking user's verification status against DNS/HTTP.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum UserWebVerificationStatus {
    Verified,         // user is verified, as of now.
    DomainNotAllowed, // domain blacklist or whitelist denied us from attempting a verification
    Unknown,          // user's status could not be determined (timeout, server error)
    Unverified,       // user's status is not verified (successful check, name / addr do not match)
}

/// A NIP-05 verification record.
#[derive(PartialEq, Eq, Debug, Clone)]
// Basic information for a verification event.  Gives us all we need to assert a NIP-05 address is good.
pub struct VerificationRecord {
    pub rowid: u64,                // database row for this verification event
    pub name: Nip05Name,           // address being verified
    pub address: String,           // pubkey
    pub event: String,             // event ID hash providing the verification
    pub event_created: u64,        // when the metadata event was published
    pub last_success: Option<u64>, // the most recent time a verification was provided.  None if verification under this name has never succeeded.
    pub last_failure: Option<u64>, // the most recent time verification was attempted, but could not be completed.
    pub failure_count: u64,        // how many consecutive failures have been observed.
}

/// Check with settings to determine if a given domain is allowed to
/// publish.
pub fn is_domain_allowed(
    domain: &str,
    whitelist: &Option<Vec<String>>,
    blacklist: &Option<Vec<String>>,
) -> bool {
    // if there is a whitelist, domain must be present in it.
    if let Some(wl) = whitelist {
        // workaround for Vec contains not accepting &str
        return wl.iter().any(|x| x == domain);
    }
    // otherwise, check that user is not in the blacklist
    if let Some(bl) = blacklist {
        return !bl.iter().any(|x| x == domain);
    }
    true
}

impl VerificationRecord {
    /// Check if the record is recent enough to be considered valid,
    /// and the domain is allowed.
    pub fn is_valid(&self, verified_users_settings: &VerifiedUsers) -> bool {
        //let settings = SETTINGS.read().unwrap();
        // how long a verification record is good for
        let nip05_expiration = &verified_users_settings.verify_expiration_duration;
        if let Some(e) = nip05_expiration {
            if !self.is_current(e) {
                return false;
            }
        }
        // check domains
        is_domain_allowed(
            &self.name.domain,
            &verified_users_settings.domain_whitelist,
            &verified_users_settings.domain_blacklist,
        )
    }

    /// Check if this record has been validated since the given
    /// duration.
    fn is_current(&self, d: &Duration) -> bool {
        match self.last_success {
            Some(s) => {
                // current time - duration
                let now = SystemTime::now();
                let cutoff = now - *d;
                let cutoff_epoch = cutoff
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|x| x.as_secs())
                    .unwrap_or(0);
                s > cutoff_epoch
            }
            None => false,
        }
    }
}

impl std::fmt::Display for VerificationRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({:?},{:?})",
            self.name.to_string(),
            self.address.chars().take(8).collect::<String>()
        )
    }
}

/// Create a new verification record based on an event
pub async fn save_verification_record(
    mut conn: db::PooledConnection,
    event: &Event,
    name: &str,
) -> Result<()> {
    let e = hex::decode(&event.id).ok();
    let n = name.to_owned();
    let a_prefix = event.get_author_prefix();
    tokio::task::spawn_blocking(move || {
        let tx = conn.transaction()?;
        {
            // if we create a /new/ one, we should get rid of any old ones.  or group the new ones by name and only consider the latest.
            let query = "INSERT INTO user_verification (metadata_event, name, verified_at) VALUES ((SELECT id from event WHERE event_hash=?), ?, strftime('%s','now'));";
            let mut stmt = tx.prepare(query)?;
            stmt.execute(params![e, n])?;
            // get the row ID
            let v_id = tx.last_insert_rowid();
            // delete everything else by this name
            let del_query = "DELETE FROM user_verification WHERE name = ? AND id != ?;";
            let mut del_stmt = tx.prepare(del_query)?;
            let count = del_stmt.execute(params![n,v_id])?;
            if count > 0 {
                info!("removed {} old verification records for ({:?},{:?})", count, n, a_prefix);
            }
        }
        tx.commit()?;
        info!("saved new verification record for ({:?},{:?})", n, a_prefix);
        let ok: Result<()> = Ok(());
        ok
    }).await?
}

/// Retrieve the most recent verification record for a given pubkey (async).
pub async fn get_latest_user_verification(
    conn: db::PooledConnection,
    pubkey: &str,
) -> Result<VerificationRecord> {
    let p = pubkey.to_owned();
    tokio::task::spawn_blocking(move || query_latest_user_verification(conn, p)).await?
}

/// Query database for the latest verification record for a given pubkey.
pub fn query_latest_user_verification(
    mut conn: db::PooledConnection,
    pubkey: String,
) -> Result<VerificationRecord> {
    let tx = conn.transaction()?;
    let query = "SELECT v.id, v.name, e.event_hash, e.created_at, v.verified_at, v.failed_at, v.failure_count FROM user_verification v LEFT JOIN event e ON e.id=v.metadata_event WHERE e.author=? ORDER BY e.created_at DESC, v.verified_at DESC, v.failed_at DESC LIMIT 1;";
    let mut stmt = tx.prepare_cached(query)?;
    let fields = stmt.query_row(params![hex::decode(&pubkey).ok()], |r| {
        let rowid: u64 = r.get(0)?;
        let rowname: String = r.get(1)?;
        let eventid: Vec<u8> = r.get(2)?;
        let created_at: u64 = r.get(3)?;
        // create a tuple since we can't throw non-rusqlite errors in this closure
        Ok((
            rowid,
            rowname,
            eventid,
            created_at,
            r.get(4).ok(),
            r.get(5).ok(),
            r.get(6)?,
        ))
    })?;
    Ok(VerificationRecord {
        rowid: fields.0,
        name: Nip05Name::try_from(&fields.1[..])?,
        address: pubkey,
        event: hex::encode(fields.2),
        event_created: fields.3,
        last_success: fields.4,
        last_failure: fields.5,
        failure_count: fields.6,
    })
}

/// Retrieve the oldest user verification (async)
pub async fn get_oldest_user_verification(
    conn: db::PooledConnection,
    earliest: u64,
) -> Result<VerificationRecord> {
    tokio::task::spawn_blocking(move || query_oldest_user_verification(conn, earliest)).await?
}

pub fn query_oldest_user_verification(
    mut conn: db::PooledConnection,
    earliest: u64,
) -> Result<VerificationRecord> {
    let tx = conn.transaction()?;
    let query = "SELECT v.id, v.name, e.event_hash, e.author, e.created_at, v.verified_at, v.failed_at, v.failure_count FROM user_verification v INNER JOIN event e ON e.id=v.metadata_event WHERE (v.verified_at < ? OR v.verified_at IS NULL) AND (v.failed_at < ? OR v.failed_at IS NULL) ORDER BY v.verified_at ASC, v.failed_at ASC LIMIT 1;";
    let mut stmt = tx.prepare_cached(query)?;
    let fields = stmt.query_row(params![earliest, earliest], |r| {
        let rowid: u64 = r.get(0)?;
        let rowname: String = r.get(1)?;
        let eventid: Vec<u8> = r.get(2)?;
        let pubkey: Vec<u8> = r.get(3)?;
        let created_at: u64 = r.get(4)?;
        // create a tuple since we can't throw non-rusqlite errors in this closure
        Ok((
            rowid,
            rowname,
            eventid,
            pubkey,
            created_at,
            r.get(5).ok(),
            r.get(6).ok(),
            r.get(7)?,
        ))
    })?;
    let vr = VerificationRecord {
        rowid: fields.0,
        name: Nip05Name::try_from(&fields.1[..])?,
        address: hex::encode(fields.3),
        event: hex::encode(fields.2),
        event_created: fields.4,
        last_success: fields.5,
        last_failure: fields.6,
        failure_count: fields.7,
    };
    Ok(vr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_from_inet() {
        let addr = "bob@example.com";
        let parsed = Nip05Name::try_from(addr);
        assert!(!parsed.is_err());
        let v = parsed.unwrap();
        assert_eq!(v.local, "bob");
        assert_eq!(v.domain, "example.com");
    }

    #[test]
    fn not_enough_sep() {
        let addr = "bob_example.com";
        let parsed = Nip05Name::try_from(addr);
        assert!(parsed.is_err());
    }

    #[test]
    fn too_many_sep() {
        let addr = "foo@bob@example.com";
        let parsed = Nip05Name::try_from(addr);
        assert!(parsed.is_err());
    }

    #[test]
    fn invalid_local_name() {
        // non-permitted ascii chars
        assert!(Nip05Name::try_from("foo!@example.com").is_err());
        assert!(Nip05Name::try_from("foo @example.com").is_err());
        assert!(Nip05Name::try_from(" foo@example.com").is_err());
        assert!(Nip05Name::try_from("f oo@example.com").is_err());
        assert!(Nip05Name::try_from("foo<@example.com").is_err());
        // unicode dash
        assert!(Nip05Name::try_from("foo‐bar@example.com").is_err());
        // emoji
        assert!(Nip05Name::try_from("foo😭bar@example.com").is_err());
    }
    #[test]
    fn invalid_domain_name() {
        // non-permitted ascii chars
        assert!(Nip05Name::try_from("foo@examp!e.com").is_err());
        assert!(Nip05Name::try_from("foo@ example.com").is_err());
        assert!(Nip05Name::try_from("foo@exa mple.com").is_err());
        assert!(Nip05Name::try_from("foo@example .com").is_err());
        assert!(Nip05Name::try_from("foo@exa<mple.com").is_err());
        // unicode dash
        assert!(Nip05Name::try_from("foobar@exa‐mple.com").is_err());
        // emoji
        assert!(Nip05Name::try_from("foobar@ex😭ample.com").is_err());
    }

    #[test]
    fn to_url() {
        let nip = Nip05Name::try_from("foobar@example.com").unwrap();
        assert_eq!(
            nip.to_url(),
            Some(
                "https://example.com/.well-known/nostr.json?name=foobar"
                    .parse()
                    .unwrap()
            )
        );
    }
}
