//! User verification using NIP-05 names
//!
//! NIP-05 defines a mechanism for authors to associate an internet
//! address with their public key, in metadata events.  This module
//! consumes a stream of metadata events, and keeps a database table
//! updated with the current NIP-05 verification status.
use crate::config::VerifiedUsers;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::repo::NostrRepo;
use hyper::body::HttpBody;
use hyper::client::connect::HttpConnector;
use hyper::Client;
use hyper_rustls::HttpsConnector;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use tokio::time::Interval;
use tracing::{debug, info, warn};

/// NIP-05 verifier state
pub struct Verifier {
    /// Repository for saving/retrieving events and records
    repo: Arc<dyn NostrRepo>,
    /// Metadata events for us to inspect
    metadata_rx: tokio::sync::broadcast::Receiver<Event>,
    /// Newly validated events get written and then broadcast on this channel to subscribers
    event_tx: tokio::sync::broadcast::Sender<Event>,
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
    pub local: String,
    pub domain: String,
}

impl Nip05Name {
    /// Does this name represent the entire domain?
    #[must_use]
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
        if components.len() == 2 {
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
        } else {
            Err(Error::CustomError("too many/few components".to_owned()))
        }
    }
}

impl std::fmt::Display for Nip05Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.local, self.domain)
    }
}

/// Check if the specified username and address are present and match in this response body
fn body_contains_user(username: &str, address: &str, bytes: &hyper::body::Bytes) -> Result<bool> {
    // convert the body into json
    let body: serde_json::Value = serde_json::from_slice(bytes)?;
    // ensure we have a names object.
    let names_map = body
        .as_object()
        .and_then(|x| x.get("names"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| Error::CustomError("not a map".to_owned()))?;
    // get the pubkey for the requested user
    let check_name = names_map.get(username).and_then(serde_json::Value::as_str);
    // ensure the address is a match
    Ok(check_name.map_or(false, |x| x == address))
}

impl Verifier {
    pub fn new(
        repo: Arc<dyn NostrRepo>,
        metadata_rx: tokio::sync::broadcast::Receiver<Event>,
        event_tx: tokio::sync::broadcast::Sender<Event>,
        settings: crate::config::Settings,
    ) -> Result<Self> {
        info!("creating NIP-05 verifier");
        // setup hyper client
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http1()
            .build();

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
            repo,
            metadata_rx,
            event_tx,
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
            .uri(url.clone())
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

        if let Ok(response_res) = tokio::time::timeout(Duration::from_secs(5), response_fut).await {
            // limit size of verification document to 1MB.
            const MAX_ALLOWED_RESPONSE_SIZE: u64 = 1024 * 1024;
            let response = response_res?;
            let status = response.status();

            // Log non-2XX status codes
            if !status.is_success() {
                info!(
                    "unexpected status code {} received for account {:?} at URL: {}",
                    status,
                    nip.to_string(),
                    url
                );
                return Ok(UserWebVerificationStatus::Unknown);
            }

            // determine content length from response
            let response_content_length = match response.body().size_hint().upper() {
                Some(v) => v,
                None => {
                    info!("missing content length header for account {:?} at URL: {}", nip.to_string(), url);
                    return Ok(UserWebVerificationStatus::Unknown);
                }
            };

            if response_content_length > MAX_ALLOWED_RESPONSE_SIZE {
                info!(
                    "content length {} exceeded limit of {} bytes for account {:?} at URL: {}",
                    response_content_length,
                    MAX_ALLOWED_RESPONSE_SIZE,
                    nip.to_string(),
                    url
                );
                return Ok(UserWebVerificationStatus::Unknown);
            }

            let (parts, body) = response.into_parts();
            // TODO: consider redirects
            if parts.status == http::StatusCode::OK {
                // parse body, determine if the username / key / address is present
                let body_bytes = match hyper::body::to_bytes(body).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        info!(
                            "failed to read response body for account {:?} at URL: {}: {:?}",
                            nip.to_string(),
                            url,
                            e
                        );
                        return Ok(UserWebVerificationStatus::Unknown);
                    }
                };

                match body_contains_user(&nip.local, pubkey, &body_bytes) {
                    Ok(true) => Ok(UserWebVerificationStatus::Verified),
                    Ok(false) => Ok(UserWebVerificationStatus::Unverified),
                    Err(e) => {
                        info!(
                            "error parsing response body for account {:?}: {:?}",
                            nip.to_string(),
                            e
                        );
                        Ok(UserWebVerificationStatus::Unknown)
                    }
                }
            } else {
                info!(
                    "unexpected status code {} for account {:?}",
                    parts.status,
                    nip.to_string()
                );
                Ok(UserWebVerificationStatus::Unknown)
            }
        } else {
            info!("timeout verifying account {:?}", nip);
            Ok(UserWebVerificationStatus::Unknown)
        }
    }

    /// Perform NIP-05 verifier tasks.
    pub async fn run(&mut self) {
        // use this to schedule periodic re-validation tasks
        // run a loop, restarting on failure
        loop {
            let res = self.run_internal().await;
            match res {
                Err(Error::ChannelClosed) => {
                    // channel was closed, we are shutting down
                    return;
                }
                Err(e) => {
                    info!("error in verifier: {:?}", e);
                }
                _ => {}
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
                            let check_verified = self.repo.get_latest_user_verification(&e.pubkey).await;
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
                        return Err(Error::ChannelClosed);
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
        let vr = self.repo.get_oldest_user_verification(earliest_epoch).await;
        match vr {
            Ok(ref v) => {
                let new_status = self.get_web_verification(&v.name, &v.address).await;
                match new_status {
                    UserWebVerificationStatus::Verified => {
                        // freshly verified account, update the
                        // timestamp.
                        self.repo.update_verification_timestamp(v.rowid).await?;
                        info!("verification updated for {}", v.to_string());
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
                            self.repo.delete_verification(v.rowid).await?;
                        } else {
                            // record normal failure, incrementing failure count
                            info!("verification failed for {}", v.to_string());
                            self.repo.fail_verification(v.rowid).await?;
                        }
                    }
                    UserWebVerificationStatus::Unverified => {
                        // domain has removed the verification, drop
                        // the record on our side.
                        info!("verification rescinded for {}", v.to_string());
                        self.repo.delete_verification(v.rowid).await?;
                    }
                }
            }
            Err(
                Error::SqlError(rusqlite::Error::QueryReturnedNoRows)
                | Error::SqlxError(sqlx::Error::RowNotFound),
            ) => {
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
            match self.repo.write_event(event).await {
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
        self.repo
            .create_verification_record(&event.id, name)
            .await?;
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
#[must_use]
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
    #[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_from_inet() {
        let addr = "bob@example.com";
        let parsed = Nip05Name::try_from(addr);
        assert!(parsed.is_ok());
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
