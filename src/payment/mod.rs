use crate::error::{Error, Result};
use crate::event::Event;
use crate::payment::cln_rest::ClnRestPaymentProcessor;
use crate::payment::lnbits::LNBitsPaymentProcessor;
use crate::repo::NostrRepo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

use async_trait::async_trait;
use nostr::key::{FromPkStr, FromSkStr};
use nostr::{key::Keys, Event as NostrEvent, EventBuilder};

pub mod cln_rest;
pub mod lnbits;

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
    nostr_keys: Option<Keys>,
    /// Payment Processor
    processor: Arc<dyn PaymentProcessor>,
}

#[async_trait]
pub trait PaymentProcessor: Send + Sync {
    /// Get invoice from processor
    async fn get_invoice(&self, keys: &Keys, amount: u64) -> Result<InvoiceInfo, Error>;
    /// Check payment status of an invoice
    async fn check_invoice(&self, payment_hash: &str) -> Result<InvoiceStatus, Error>;
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum Processor {
    LNBits,
    ClnRest,
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
    pub bolt11: String,
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
    /// Check account,
    CheckAccount(String),
    /// Account Admitted
    AccountAdmitted(String),
    /// Invoice generated
    Invoice(String, InvoiceInfo),
    /// Invoice call back
    /// Payment hash is passed
    // This may have to be changed to better support other processors
    InvoicePaid(String),
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
        let nostr_keys = if let Some(secret_key) = &settings.pay_to_relay.secret_key {
            Some(Keys::from_sk_str(secret_key)?)
        } else {
            None
        };

        // Create processor kind defined in settings
        let processor: Arc<dyn PaymentProcessor> = match &settings.pay_to_relay.processor {
            Processor::LNBits => Arc::new(LNBitsPaymentProcessor::new(&settings)),
            Processor::ClnRest => Arc::new(ClnRestPaymentProcessor::new(&settings)?),
        };

        Ok(Payment {
            repo,
            payment_tx,
            payment_rx,
            event_tx,
            settings,
            nostr_keys,
            processor,
        })
    }

    /// Perform Payment tasks
    pub async fn run(&mut self) {
        loop {
            let res = self.run_internal().await;
            if let Err(e) = res {
                info!("error in payment: {:?}", e);
            }
        }
    }

    /// Internal select loop for preforming payment operations
    async fn run_internal(&mut self) -> Result<()> {
        tokio::select! {
            m = self.payment_rx.recv() => {
                match m {
                    Ok(PaymentMessage::NewAccount(pubkey)) => {
                        info!("payment event for {:?}", pubkey);
                        // REVIEW: This will need to change for cost per event
                        let amount = self.settings.pay_to_relay.admission_cost;
                        let invoice_info = self.get_invoice_info(&pubkey, amount).await?;
                        // TODO: should handle this error
                        self.payment_tx.send(PaymentMessage::Invoice(pubkey, invoice_info)).ok();
                    },
                    // Gets the most recent unpaid invoice from database
                    // Checks LNbits to verify if paid/unpaid
                    Ok(PaymentMessage::CheckAccount(pubkey)) => {
                        let keys = Keys::from_pk_str(&pubkey)?;

                        if let Ok(Some(invoice_info)) = self.repo.get_unpaid_invoice(&keys).await {
                            match self.check_invoice_status(&invoice_info.payment_hash).await? {
                                InvoiceStatus::Paid => {
                                    self.repo.admit_account(&keys, self.settings.pay_to_relay.admission_cost).await?;
                                    self.payment_tx.send(PaymentMessage::AccountAdmitted(pubkey)).ok();
                                }
                                _ => {
                                    self.payment_tx.send(PaymentMessage::Invoice(pubkey, invoice_info)).ok();
                                }
                            }
                        } else {
                        let amount = self.settings.pay_to_relay.admission_cost;
                            let invoice_info = self.get_invoice_info(&pubkey, amount).await?;
                            self.payment_tx.send(PaymentMessage::Invoice(pubkey, invoice_info)).ok();
                        }
                    }
                    Ok(PaymentMessage::InvoicePaid(payment_hash)) => {
                        if self.check_invoice_status(&payment_hash).await?.eq(&InvoiceStatus::Paid) {
                            let pubkey = self.repo
                                .update_invoice(&payment_hash, InvoiceStatus::Paid)
                                .await?;

                            let key = Keys::from_pk_str(&pubkey)?;
                            self.repo.admit_account(&key, self.settings.pay_to_relay.admission_cost).await?;
                        }
                    }
                    Ok(_) => {
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
        let nostr_keys = match &self.nostr_keys {
            Some(key) => key,
            None => return Err(Error::CustomError("Nostr key not defined".to_string())),
        };

        // Create Nostr key from pk
        let key = Keys::from_pk_str(pubkey)?;

        let pubkey = key.public_key();

        // Event DM with terms of service
        let message_event: NostrEvent = EventBuilder::new_encrypted_direct_msg(
            nostr_keys,
            pubkey,
            &self.settings.pay_to_relay.terms_message,
        )?
        .to_event(nostr_keys)?;

        // Event DM with invoice
        let invoice_event: NostrEvent =
            EventBuilder::new_encrypted_direct_msg(nostr_keys, pubkey, &invoice_info.bolt11)?
                .to_event(nostr_keys)?;

        // Persist DM events to DB
        self.repo.write_event(&message_event.clone().into()).await?;
        self.repo.write_event(&invoice_event.clone().into()).await?;

        // Broadcast DM events
        self.event_tx.send(message_event.clone().into()).ok();
        self.event_tx.send(invoice_event.clone().into()).ok();

        Ok(())
    }

    /// Get Invoice Info
    /// If the has an active invoice that will be return
    /// Otherwise a new invoice will be generated by the payment processor
    pub async fn get_invoice_info(&self, pubkey: &str, amount: u64) -> Result<InvoiceInfo> {
        // If user is already in DB this will be false
        // This avoids recreating admission invoices
        // I think it will continue to send DMs with the invoice
        // If client continues to try and write to the relay (will be same invoice)
        let key = Keys::from_pk_str(pubkey)?;
        if !self.repo.create_account(&key).await? {
            if let Ok(Some(invoice_info)) = self.repo.get_unpaid_invoice(&key).await {
                return Ok(invoice_info);
            }
        }

        let key = Keys::from_pk_str(pubkey)?;

        let invoice_info = self.processor.get_invoice(&key, amount).await?;

        // Persist invoice to DB
        self.repo
            .create_invoice_record(&key, invoice_info.clone())
            .await?;

        if self.settings.pay_to_relay.direct_message {
            // Admission event invoice and terms to pubkey that is joining
            self.send_admission_message(pubkey, &invoice_info).await?;
        }

        Ok(invoice_info)
    }

    /// Check paid status of invoice with LNbits
    pub async fn check_invoice_status(&self, payment_hash: &str) -> Result<InvoiceStatus, Error> {
        // Check base if passed expiry time
        let status = self.processor.check_invoice(payment_hash).await?;
        self.repo
            .update_invoice(payment_hash, status.clone())
            .await?;

        Ok(status)
    }
}
