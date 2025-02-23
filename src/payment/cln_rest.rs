use std::{fs, str::FromStr};

use async_trait::async_trait;
use cln_rpc::{
    model::{
        requests::InvoiceRequest,
        responses::{InvoiceResponse, ListinvoicesInvoicesStatus, ListinvoicesResponse},
    },
    primitives::{Amount, AmountOrAny},
};
use config::ConfigError;
use http::{header::CONTENT_TYPE, HeaderValue, Uri};
use hyper::{client::HttpConnector, Client};
use hyper_rustls::HttpsConnector;
use nostr::Keys;
use rand::random;

use crate::{
    config::Settings,
    error::{Error, Result},
};

use super::{InvoiceInfo, InvoiceStatus, PaymentProcessor};

#[derive(Clone)]
pub struct ClnRestPaymentProcessor {
    client: hyper::Client<HttpsConnector<HttpConnector>, hyper::Body>,
    settings: Settings,
    rune_header: HeaderValue,
}

impl ClnRestPaymentProcessor {
    pub fn new(settings: &Settings) -> Result<Self> {
        let rune_path = settings
            .pay_to_relay
            .rune_path
            .clone()
            .ok_or(ConfigError::NotFound("rune_path".to_string()))?;
        let rune = String::from_utf8(fs::read(rune_path)?)
            .map_err(|_| ConfigError::Message("Rune should be UTF8".to_string()))?;
        let mut rune_header = HeaderValue::from_str(rune.trim())
            .map_err(|_| ConfigError::Message("Invalid Rune header".to_string()))?;
        rune_header.set_sensitive(true);

        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder().build::<_, hyper::Body>(https);

        Ok(Self {
            client,
            settings: settings.clone(),
            rune_header,
        })
    }
}

#[async_trait]
impl PaymentProcessor for ClnRestPaymentProcessor {
    async fn get_invoice(&self, key: &Keys, amount: u64) -> Result<InvoiceInfo, Error> {
        let random_number: u16 = random();
        let memo = format!("{}: {}", random_number, key.public_key());

        let body = InvoiceRequest {
            cltv: None,
            deschashonly: None,
            expiry: None,
            preimage: None,
            exposeprivatechannels: None,
            fallbacks: None,
            amount_msat: AmountOrAny::Amount(Amount::from_sat(amount)),
            description: memo.clone(),
            label: "Nostr".to_string(),
        };
        let uri = Uri::from_str(&format!(
            "{}/v1/invoice",
            &self.settings.pay_to_relay.node_url
        ))
        .map_err(|_| ConfigError::Message("Bad node URL".to_string()))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header("Rune", self.rune_header.clone())
            .body(hyper::Body::from(serde_json::to_string(&body)?))
            .expect("request builder");

        let res = self.client.request(req).await?;

        let body = hyper::body::to_bytes(res.into_body()).await?;
        let invoice_response: InvoiceResponse = serde_json::from_slice(&body)?;

        Ok(InvoiceInfo {
            pubkey: key.public_key().to_string(),
            payment_hash: invoice_response.payment_hash.to_string(),
            bolt11: invoice_response.bolt11,
            amount,
            memo,
            status: InvoiceStatus::Unpaid,
            confirmed_at: None,
        })
    }

    async fn check_invoice(&self, payment_hash: &str) -> Result<InvoiceStatus, Error> {
        let uri = Uri::from_str(&format!(
            "{}/v1/listinvoices?payment_hash={}",
            &self.settings.pay_to_relay.node_url, payment_hash
        ))
        .map_err(|_| ConfigError::Message("Bad node URL".to_string()))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header("Rune", self.rune_header.clone())
            .body(hyper::Body::empty())
            .expect("request builder");

        let res = self.client.request(req).await?;

        let body = hyper::body::to_bytes(res.into_body()).await?;
        let invoice_response: ListinvoicesResponse = serde_json::from_slice(&body)?;
        let invoice = invoice_response
            .invoices
            .first()
            .ok_or(Error::CustomError("Invoice not found".to_string()))?;
        let status = match invoice.status {
            ListinvoicesInvoicesStatus::PAID => InvoiceStatus::Paid,
            ListinvoicesInvoicesStatus::UNPAID => InvoiceStatus::Unpaid,
            ListinvoicesInvoicesStatus::EXPIRED => InvoiceStatus::Expired,
        };
        Ok(status)
    }
}
