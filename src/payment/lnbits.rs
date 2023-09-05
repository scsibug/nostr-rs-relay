//! LNBits payment processor
use http::Uri;
use hyper::client::connect::HttpConnector;
use hyper::Client;
use hyper_rustls::HttpsConnector;
use nostr::Keys;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use async_trait::async_trait;
use rand::Rng;

use std::str::FromStr;
use url::Url;

use crate::{config::Settings, error::Error};

use super::{InvoiceInfo, InvoiceStatus, PaymentProcessor};

const APIPATH: &str = "/api/v1/payments/";

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
    /// HTTP client
    client: hyper::Client<HttpsConnector<HttpConnector>, hyper::Body>,
    settings: Settings,
}

impl LNBitsPaymentProcessor {
    pub fn new(settings: &Settings) -> Self {
        // setup hyper client
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder().build::<_, hyper::Body>(https);

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

        let callback_url = Url::parse(
            &self
                .settings
                .info
                .relay_url
                .clone()
                .unwrap()
                .replace("ws", "http"),
        )?
        .join("lnbits")?;

        let body = LNBitsCreateInvoice {
            out: false,
            amount,
            memo: memo.clone(),
            webhook: callback_url.to_string(),
            unit: "sat".to_string(),
            internal: false,
            expiry: 3600,
        };
        let url = Url::parse(&self.settings.pay_to_relay.node_url)?.join(APIPATH)?;
        let uri = Uri::from_str(url.as_str().strip_suffix('/').unwrap_or(url.as_str())).unwrap();

        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("X-Api-Key", &self.settings.pay_to_relay.api_secret)
            .body(hyper::Body::from(serde_json::to_string(&body)?))
            .expect("request builder");

        let res = self.client.request(req).await?;

        // Json to Struct of LNbits callback
        let body = hyper::body::to_bytes(res.into_body()).await?;
        let invoice_response: LNBitsCreateInvoiceResponse = serde_json::from_slice(&body)?;

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
        let url = Url::parse(&self.settings.pay_to_relay.node_url)?
            .join(APIPATH)?
            .join(payment_hash)?;
        let uri = Uri::from_str(url.as_str()).unwrap();

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("X-Api-Key", &self.settings.pay_to_relay.api_secret)
            .body(hyper::Body::empty())
            .expect("request builder");

        let res = self.client.request(req).await?;
        // Json to Struct of LNbits callback
        let body = hyper::body::to_bytes(res.into_body()).await?;
        let invoice_response: Value = serde_json::from_slice(&body)?;

        let status = if let Ok(invoice_response) =
            serde_json::from_value::<LNBitsCheckInvoiceResponse>(invoice_response)
        {
            if invoice_response.paid {
                InvoiceStatus::Paid
            } else {
                InvoiceStatus::Unpaid
            }
        } else {
            InvoiceStatus::Expired
        };

        Ok(status)
    }
}
