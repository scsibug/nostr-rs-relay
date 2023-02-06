use crate::error::{Error, Result};
use crate::event::Event;
use crate::repo::NostrRepo;
use nostr::secp256k1::XOnlyPublicKey;
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info, warn};

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
    // HTTP client
    client: Client,

    /// Node Url
    node_url: String,
    // Api
    api_secret: String,
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

        let nostr_keys = Keys::generate();
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let api_secret = settings.clone().pay_to_relay.api_secret.unwrap();

        let node_url = match &settings.pay_to_relay.cln_node_url {
            Some(url) => url.clone(),
            None => return Err(Error::CustomError("Relay url not set".to_string())),
        };

        Ok(Payment {
            repo,
            payment_rx,
            event_tx,
            settings,
            nostr_keys,
            client,
            api_secret: api_secret.clone(),
            node_url,
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
                        self.send_admission_message(&e.pubkey).await?;
                    },
                    _ => todo!()
                }
            }
        }

        Ok(())
    }

    pub async fn send_admission_message(&mut self, pubkey: &str) -> Result<()> {
        // Create lighting address
        // Checks that node url is set other wise returns error
        // REVIEW: should maybe check this in config

        let invoice_info: InvoiceInfo;

        // If use is already in DB this will be false
        // This avoids resending admission invoices
        // FIXME: It should be more intelligent resend after x time?
        if !self.repo.create_account(pubkey).await? {
            return Err(Error::UnknownError);
        }

        // If tor proxy is setting is set send request via proxy
        match &self.settings.pay_to_relay.tor_proxy {
            Some(proxy_url) => {
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
            None => {
                let random_number: u16 = rand::thread_rng().gen();

                let memo = format!("{}: {}", random_number, pubkey);
                let amount = self.settings.pay_to_relay.admission_cost;

                let body = LNBitsCreateInvoice {
                    out: false,
                    amount,
                    memo: memo.clone(),
                    webhook: format!(
                        "{}lnbits",
                        &self
                            .settings
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

                let res = self
                    .client
                    .post(&self.node_url)
                    .header("X-Api-Key", &self.api_secret)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();

                let payment_response = res.json::<LNBitsCreateInvoiceResponse>().await.unwrap();

                invoice_info = InvoiceInfo {
                    pubkey: pubkey.to_string(),
                    payment_hash: payment_response.payment_hash,
                    invoice: payment_response.payment_request,
                    amount,
                    memo,
                    status: InvoiceStatus::Unpaid,
                    confirmed_at: None,
                };

                println!("{:?}", &invoice_info);
            }
        }

        let pubkey = XOnlyPublicKey::from_str(pubkey).unwrap();

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
        
        // Persist invoice to DB
        self.repo
            .create_invoice_record(&pubkey.to_string(), invoice_info.clone())
            .await?;

        // Broadcast event
        // FIXME: events are not getting broadcasted
        self.event_tx.send(message_event.clone().into()).ok();
        self.event_tx.send(invoice_event.clone().into()).ok();


        Ok(())
    }
}
