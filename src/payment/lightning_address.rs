//! LightningAddress payment processor
use http::Uri;
use hyper::client::connect::HttpConnector;
use hyper::Client;
use hyper_tls::HttpsConnector;
use nostr::Keys;
use serde::{Deserialize, Serialize};

use async_trait::async_trait;
use rand::Rng;
use tracing::debug;

use config::ConfigError;
use std::str::FromStr;
use url::Url;

use crate::{config::Settings, error::Error};

use super::{InvoiceInfo, InvoiceStatus, PaymentProcessor};

/// Lnurl pay request response
#[derive(Debug, Serialize, Deserialize)]
struct LnUrlPayResponse {
    callback: String,
    #[serde(rename = "maxSendable")]
    max_sendable: u64,
    #[serde(rename = "minSendable")]
    min_sendable: u64,
    metadata: String,
    tag: String,
}

/// Invoice response
#[derive(Debug, Serialize, Deserialize)]
pub struct InvoiceResponse {
    payment_hash: String,
    #[serde(alias = "pr")]
    payment_request: String,
}

/// Check payment response
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckPaymentResponse {
    paid: bool,
}

#[derive(Clone)]
pub struct LightningAddressPaymentProcessor {
    /// HTTP client
    client: hyper::Client<HttpsConnector<HttpConnector>, hyper::Body>,
    settings: Settings,
}

impl LightningAddressPaymentProcessor {
    pub fn new(settings: &Settings) -> Self {
        // setup hyper client
        let https = HttpsConnector::new();
        let client = Client::builder().build::<_, hyper::Body>(https);

        Self {
            client,
            settings: settings.clone(),
        }
    }
}

#[async_trait]
impl PaymentProcessor for LightningAddressPaymentProcessor {
    /// Uses lightning address to get a new invoice
    async fn get_invoice(&self, key: &Keys, amount: u64) -> Result<InvoiceInfo, Error> {
        let random_number: u16 = rand::thread_rng().gen();
        let memo = format!("{}: {}", random_number, key.public_key());

        let lightning_address = self
            .settings
            .pay_to_relay
            .lightning_address
            .as_ref()
            .ok_or(Error::ConfigError(ConfigError::NotFound(String::from(
                "lightning_address",
            ))))?;

        let (local_part, domain) = lightning_address.split_once('@').ok_or(Error::ConnError)?;

        let base = format!("https://{domain}");
        let base_url = Url::parse(&base)?;

        let api_path = format!("/.well-known/lnurlp/{local_part}");
        let lnurl_payment_request_url = base_url.join(&api_path)?;
        let lnurl_pay_uri = Uri::from_str(lnurl_payment_request_url.as_str()).unwrap();
        debug!("{lnurl_pay_uri}");

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(lnurl_pay_uri)
            .body(hyper::Body::empty())
            .expect("request builder");

        let res = self.client.request(req).await?;

        debug!("{res:?}");

        let pay_resp_body = hyper::body::to_bytes(res.into_body()).await?;
        let pay_request_response: LnUrlPayResponse = serde_json::from_slice(&pay_resp_body)?;

        debug!("{:?}", pay_request_response);

        let amount_msat = 1000 * amount;
        if amount_msat < pay_request_response.min_sendable
            || amount_msat > pay_request_response.max_sendable
        {
            return Err(Error::ConnError);
        }

        let amount_param = format!("amount={amount_msat}");
        let mut invoice_request_url = Url::parse(&pay_request_response.callback)?;
        invoice_request_url.set_query(Some(amount_param.as_str()));
        let invoice_request_uri = Uri::from_str(invoice_request_url.as_str()).unwrap();
        debug!("{invoice_request_uri}");

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(invoice_request_uri)
            .body(hyper::Body::empty())
            .expect("request builder");

        let res = self.client.request(req).await?;

        debug!("{res:?}");

        let invoice_resp_body = hyper::body::to_bytes(res.into_body()).await?;
        let invoice_response: InvoiceResponse = serde_json::from_slice(&invoice_resp_body)?;

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

    /// Calls an external Api to check the payment status of invoice
    async fn check_invoice(&self, payment_hash: &str) -> Result<InvoiceStatus, Error> {
        let lightning_address = self
            .settings
            .pay_to_relay
            .lightning_address
            .as_ref()
            .ok_or(Error::ConfigError(ConfigError::NotFound(String::from(
                "lightning_address",
            ))))?;

        let check_invoice_endpoint = self
            .settings
            .pay_to_relay
            .check_invoice_endpoint
            .as_ref()
            .ok_or(Error::ConfigError(ConfigError::NotFound(String::from(
                "check_invoice_endpoint",
            ))))?;

        let (_local_part, domain) = lightning_address.split_once('@').ok_or(Error::ConnError)?;

        let base = format!("https://{domain}");
        let base_url = Url::parse(&base)?;

        let mut check_invoice_endpoint_url = base_url.join(&check_invoice_endpoint)?;

        let query_param = format!("payment_hash={payment_hash}");
        check_invoice_endpoint_url.set_query(Some(query_param.as_str()));
        let uri = Uri::from_str(check_invoice_endpoint_url.as_str()).unwrap();
        debug!("{uri}");

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .body(hyper::Body::empty())
            .expect("request builder");

        let res = self.client.request(req).await?;

        let body = hyper::body::to_bytes(res.into_body()).await?;
        debug!("check invoice: {body:?}");
        let invoice_response: CheckPaymentResponse = serde_json::from_slice(&body)?;

        let status = if invoice_response.paid {
            InvoiceStatus::Paid
        } else {
            InvoiceStatus::Unpaid
        };

        Ok(status)
    }
}
