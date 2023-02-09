use crate::error::{Error, Result};
use crate::event::Event;
use crate::repo::NostrRepo;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use nostr::key::{FromPkStr, FromSkStr};
use nostr::{key::Keys, Event as NostrEvent, EventBuilder};

/// Payment handler
pub struct Payment {
    /// Repository for saving/retrieving events and events
    repo: Arc<dyn NostrRepo>,
    /// Newly validated events get written and then broadcast on this channel to subscribers
    event_tx: tokio::sync::broadcast::Sender<Event>,
    /// Events to inspect
    payment_rx: tokio::sync::broadcast::Receiver<Event>,
    /// Settings
    settings: crate::config::Settings,
    // Api
    nostr_keys: Keys,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LNBitsCreateInvoice {
    out: bool,
    amount: u64,
    memo: String,
    webhook: String,
    unit: String,
    internal: bool,
    expiry: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LNBitsCallback {
    pub checking_id: String,
    pub pending: bool,
    pub amount: u64,
    pub memo: String,
    pub time: u64,
    pub bolt11: String,
    pub preimage: String,
    pub payment_hash: String,
    pub wallet_id: String,
    pub webhook: String,
    pub webhook_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, sqlx::Type)]
#[sqlx(type_name = "status")]
pub enum InvoiceStatus {
    Unpaid,
    Paid,
    Expired,
}

impl ToString for InvoiceStatus {
    fn to_string(&self) -> String {
        match self {
            InvoiceStatus::Paid => "Paid".to_string(),
            InvoiceStatus::Unpaid => "Unpaid".to_string(),
            InvoiceStatus::Expired => "Expired".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InvoiceInfo {
    pub pubkey: String,
    pub payment_hash: String,
    pub invoice: String,
    pub amount: u64,
    pub status: InvoiceStatus,
    pub memo: String,
    pub confirmed_at: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPaymentResponse {
    payment_hash: String,
    expires_at: u64,
    bolt11: String,
    payment_secret: String,
}

/// Invoice response for LN bits
#[derive(Debug, Serialize, Deserialize)]
pub struct LNBitsCreateInvoiceResponse {
    payment_hash: String,
    payment_request: String,
    checking_id: String,
    lnurl_response: Option<String>,
}

pub enum PaymentMessage {
    Event(Event),
}

impl Payment {
    pub fn new(
        repo: Arc<dyn NostrRepo>,
        payment_rx: tokio::sync::broadcast::Receiver<Event>,
        event_tx: tokio::sync::broadcast::Sender<Event>,
        settings: crate::config::Settings,
    ) -> Result<Self> {
        info!("Create payment handler");

        let nostr_keys = Keys::from_sk_str(&settings.pay_to_relay.secret_key)?;
        // let nostr_keys = Keys::generate();

        Ok(Payment {
            repo,
            payment_rx,
            event_tx,
            settings,
            nostr_keys,
        })
    }

    /// Perform Payment tasks
    pub async fn run(&mut self) {
        loop {
            tokio::spawn(async move {});

            let res = self.run_internal().await;
            if let Err(e) = res {
                info!("error in payment: {:?}", e);
            }
        }
    }

    /// Internal select loop for preforming payment operatons
    async fn run_internal(&mut self) -> Result<()> {
        tokio::select! {
            m = self.payment_rx.recv() => {
                match m {
                    Ok(e) => {
                        info!("payment event for {:?}", e.pubkey);
                        // REVIEW: This will need to change for cost per event
                        let amount = self.settings.pay_to_relay.admission_cost;
                        let invoice_info = get_invoice_info(&self.repo, &e.pubkey, amount, &self.settings).await?;
                        self.send_admission_message(&e.pubkey, invoice_info).await?;
                    },
                    _ => todo!()
                }
            }
        }

        Ok(())
    }

    pub async fn send_admission_message(
        &mut self,
        pubkey: &str,
        invoice_info: InvoiceInfo,
    ) -> Result<()> {
        // Create lighting address
        // Checks that node url is set other wise returns error
        // REVIEW: should maybe check this in config
        let key = Keys::from_pk_str(pubkey)?;

        let pubkey = key.public_key();

        // Publish dm with terms of service and invoice
        let message_event: NostrEvent = EventBuilder::new_encrypted_direct_msg(
            &self.nostr_keys,
            pubkey,
            &self.settings.pay_to_relay.terms_message,
        )?
        .to_event(&self.nostr_keys)?;

        // Event with invoice
        let invoice_event: NostrEvent = EventBuilder::new_encrypted_direct_msg(
            &self.nostr_keys,
            pubkey,
            &invoice_info.invoice,
        )?
        .to_event(&self.nostr_keys)?;

        // Persist event to DB
        self.repo.write_event(&message_event.clone().into()).await?;
        self.repo.write_event(&invoice_event.clone().into()).await?;

        // Broadcast event
        self.event_tx.send(message_event.clone().into()).ok();
        self.event_tx.send(invoice_event.clone().into()).ok();

        Ok(())
    }
}

pub async fn get_invoice_info(
    repo: &Arc<dyn NostrRepo>,
    pubkey: &str,
    amount: u64,
    settings: &crate::config::Settings,
) -> Result<InvoiceInfo> {
    // If use is already in DB this will be false
    // This avoids resending admission invoices
    // FIXME: It should be more intelligent resend after x time?
    let key = Keys::from_pk_str(pubkey)?;
    if !repo.create_account(&key).await.unwrap() {
        if let Some(invoice_info) = repo.get_unpaid_invoice(&key).await? {
            return Ok(invoice_info);
        } else {
            return Err(Error::UnknownError);
        }
    }
    // If tor proxy is setting is set send request via proxy
    let client = match &settings.pay_to_relay.tor_proxy {
        Some(proxy_url) => {
            // TODO: Implement tor

            debug!("{proxy_url}");
            /*
            let proxy = ureq::Proxy::new(proxy_url).unwrap();
            let agent = ureq::AgentBuilder::new().proxy(proxy).build();

            // This is proxied.
            let resp = agent
                .post(&node_url)
                .set("X-Access", "eUoort9EGSIPVpdmoxqn")
                //.set("Range", "payments=0-99")
                .send_json(ureq::json!({"method": "listsendpays"}));
            debug!("{:?}", resp);
            // resp
            // .unwrap();
            println!("{:?}", resp);
            */
            todo!();
        }
        None => reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?,
    };

    let key = Keys::from_pk_str(pubkey)?;
    let random_number: u16 = rand::thread_rng().gen();

    let memo = format!("{}: {}", random_number, key.public_key());

    let body = LNBitsCreateInvoice {
        out: false,
        amount,
        memo: memo.clone(),
        webhook: format!(
            "{}lnbits",
            &settings
                .info
                .relay_url
                .clone()
                .unwrap()
                .replace("ws", "http")
        ),
        unit: "sat".to_string(),
        internal: false,
        expiry: 3600,
    };

    let res = client
        .post(&settings.pay_to_relay.node_url)
        .header("X-Api-Key", &settings.pay_to_relay.api_secret)
        .json(&body)
        .send()
        .await?;

    let payment_response = res.json::<LNBitsCreateInvoiceResponse>().await?;

    let invoice_info = InvoiceInfo {
        pubkey: key.public_key().to_string(),
        payment_hash: payment_response.payment_hash,
        invoice: payment_response.payment_request,
        amount,
        memo,
        status: InvoiceStatus::Unpaid,
        confirmed_at: None,
    };

    // Persist invoice to DB
    repo.create_invoice_record(&key, invoice_info.clone())
        .await?;

    Ok(invoice_info)
}
