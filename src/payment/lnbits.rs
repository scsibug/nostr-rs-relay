use nostr::Keys;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use async_trait::async_trait;
use rand::Rng;
use tracing::debug;

use crate::{config::Settings, error::Error};

use super::{InvoiceInfo, InvoiceStatus, PaymentProcessor};

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

/// LN Bits repose for check invoice endpoint
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LNBitsCheckInvoiceResponse {
    paid: bool,
}

#[derive(Clone)]
pub struct LNBitsPaymentProcessor {
    client: Client,
    settings: Settings,
}

impl LNBitsPaymentProcessor {
    pub fn new(settings: &Settings) -> Self {
        let client = reqwest::Client::builder()
            // Not ideal to accept all certs
            // But enforcing certs may be too much of a burden on users
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        Self {
            client,
            settings: settings.clone(),
        }
    }
}

#[async_trait]
impl PaymentProcessor for LNBitsPaymentProcessor {
    /// Calls LNBits api to ger new invoice
    async fn get_invoice(&self, key: &Keys, amount: u64) -> Result<InvoiceInfo, Error> {
        let random_number: u16 = rand::thread_rng().gen();
        let memo = format!("{}: {}", random_number, key.public_key());
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
            .post(&self.settings.pay_to_relay.node_url)
            .header("X-Api-Key", &self.settings.pay_to_relay.api_secret)
            .json(&body)
            .send()
            .await?;

        debug!("{res:?}");

        // Json to Struct of LNbits callback
        let invoice_response = res.json::<LNBitsCreateInvoiceResponse>().await.unwrap();

        debug!("{:?}", invoice_response);

        Ok(InvoiceInfo {
            pubkey: key.public_key().to_string(),
            payment_hash: invoice_response.payment_hash,
            bolt11: invoice_response.payment_request,
            amount,
            memo,
            status: InvoiceStatus::Unpaid,
            confirmed_at: None,
        })
    }

    /// Calls LNBits Api to check the payment status of invoice
    async fn check_invoice(&self, payment_hash: &str) -> Result<InvoiceStatus, Error> {
        let res = self
            .client
            .get(format!(
                "{}/{}",
                &self.settings.pay_to_relay.node_url, payment_hash
            ))
            .header("X-Api-Key", &self.settings.pay_to_relay.api_secret)
            .send()
            .await?;

        let invoice_response = res.json::<LNBitsCheckInvoiceResponse>().await.unwrap();

        let status = if invoice_response.paid {
            InvoiceStatus::Paid
        } else {
            InvoiceStatus::Unpaid
        };

        Ok(status)
    }
}
