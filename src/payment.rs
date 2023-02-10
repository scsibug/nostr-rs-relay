use crate::error::{Error, Result};
use crate::event::Event;
use crate::repo::NostrRepo;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

use nostr::key::{FromPkStr, FromSkStr};
use nostr::{key::Keys, Event as NostrEvent, EventBuilder};

/// Payment handler
pub struct Payment {
    /// Repository for saving/retrieving events and events
    repo: Arc<dyn NostrRepo>,
    /// Newly validated events get written and then broadcast on this channel to subscribers
    event_tx: tokio::sync::broadcast::Sender<Event>,
    /// Payment message sender
    payment_tx: tokio::sync::broadcast::Sender<PaymentMessage>,
    /// Payment message receiver
    payment_rx: tokio::sync::broadcast::Receiver<PaymentMessage>,
    /// Settings
    settings: crate::config::Settings,
    // Nostr Keys
    nostr_keys: Keys,
}

/// Info LNBits expects in create invoice request
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

/// Invoice response for LN bits
#[derive(Debug, Serialize, Deserialize)]
pub struct LNBitsCreateInvoiceResponse {
    payment_hash: String,
    payment_request: String,
    checking_id: String,
    lnurl_response: Option<String>,
}

/// LNBits call back response
/// Used when an invoice is paid
/// lnbits to post the status change to relay
#[derive(Serialize, Deserialize, Debug, Clone)]
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

/// Possible states of an invoice
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

/// Invoice information
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

/// Message variants for the payment channel
#[derive(Debug, Clone)]
pub enum PaymentMessage {
    /// New account
    NewAccount(String),
    /// Invoice generated
    Invoice(String, InvoiceInfo),
    /// Invoice call back
    InvoiceUpdate(LNBitsCallback),
}

impl Payment {
    pub fn new(
        repo: Arc<dyn NostrRepo>,
        payment_tx: tokio::sync::broadcast::Sender<PaymentMessage>,
        payment_rx: tokio::sync::broadcast::Receiver<PaymentMessage>,
        event_tx: tokio::sync::broadcast::Sender<Event>,
        settings: crate::config::Settings,
    ) -> Result<Self> {
        info!("Create payment handler");

        // Create nostr key from sk string
        let nostr_keys = Keys::from_sk_str(&settings.pay_to_relay.secret_key)?;

        Ok(Payment {
            repo,
            payment_tx,
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
                    Ok(PaymentMessage::NewAccount(pubkey)) => {
                        info!("payment event for {:?}", pubkey);
                        // REVIEW: This will need to change for cost per event
                        let amount = self.settings.pay_to_relay.admission_cost;
                        let invoice_info = self.get_invoice_info(&pubkey, amount, &self.settings).await?;
                        // TODO: should handle this error
                        self.payment_tx.send(PaymentMessage::Invoice(pubkey, invoice_info)).ok();
                    },
                    Ok(PaymentMessage::InvoiceUpdate(callback)) => {
                        let pubkey = self.repo
                            .update_invoice(&callback.payment_hash, InvoiceStatus::Paid)
                            .await
                            .unwrap();

                         let key = Keys::from_pk_str(&pubkey).unwrap();
                         self.repo.admit_account(&key, self.settings.pay_to_relay.admission_cost).await.unwrap();
                    }
                    Ok(PaymentMessage::Invoice(_pubkey, _invoice)) => {
                        // For this variant nothing need to be done here
                        // it is used by `server`
                    }
                    Err(err) => warn!("Payment RX: {err}")
                }
            }
        }

        Ok(())
    }

    /// Sends Nostr DM to pubkey that requested invoice
    /// Two events the terms followed by the bolt11 invoice
    pub async fn send_admission_message(
        &self,
        pubkey: &str,
        invoice_info: &InvoiceInfo,
    ) -> Result<()> {
        // Create Nostr key from pk
        let key = Keys::from_pk_str(pubkey)?;

        let pubkey = key.public_key();

        // Event DM with terms of service
        let message_event: NostrEvent = EventBuilder::new_encrypted_direct_msg(
            &self.nostr_keys,
            pubkey,
            &self.settings.pay_to_relay.terms_message,
        )?
        .to_event(&self.nostr_keys)?;

        // Event DM with invoice
        let invoice_event: NostrEvent = EventBuilder::new_encrypted_direct_msg(
            &self.nostr_keys,
            pubkey,
            &invoice_info.invoice,
        )?
        .to_event(&self.nostr_keys)?;

        // Persist DM events to DB
        self.repo.write_event(&message_event.clone().into()).await?;
        self.repo.write_event(&invoice_event.clone().into()).await?;

        // Broadcast DM events
        self.event_tx.send(message_event.clone().into()).ok();
        self.event_tx.send(invoice_event.clone().into()).ok();

        Ok(())
    }

    pub async fn get_invoice_info(
        &self,
        pubkey: &str,
        amount: u64,
        settings: &crate::config::Settings,
    ) -> Result<InvoiceInfo> {
        // If use is already in DB this will be false
        // This avoids resending admission invoices
        // FIXME: It should be more intelligent resend after x time?
        // I think it will continue to send DMs with an invoice
        // If client continues to try and write to the event (will be same invoice)
        let key = Keys::from_pk_str(pubkey)?;
        if !self.repo.create_account(&key).await.unwrap() {
            if let Some(invoice_info) = self.repo.get_unpaid_invoice(&key).await? {
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

        // Json to Struct of LNbits callback
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
        self.repo
            .create_invoice_record(&key, invoice_info.clone())
            .await?;

        // Admission event invoice and terms to pubkey that is joining
        self.send_admission_message(pubkey, &invoice_info).await?;

        Ok(invoice_info)
    }
}
