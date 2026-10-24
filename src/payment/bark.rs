//! Bark payment processor
//!
//! Takes payments through a [Bark](https://gitlab.com/ark-bitcoin/bark)
//! wallet daemon (`barkd`) and its REST API. Bark is an Ark wallet. Two
//! payment rails are available, chosen with `pay_to_relay.bark_rails`:
//!
//! * `lightning`: a BOLT11 invoice, received through the Ark server's
//!   Lightning gateway. Paid when barkd reports the receive as `settled`.
//! * `ark`: a fresh Ark address per invoice, paid with an instant
//!   off-chain (arkoor) Ark payment. Paid when barkd's history shows
//!   successful receives on that address adding up to the amount.
//! * `both` (default): a BIP-321 `bitcoin:` URI carrying both the Ark
//!   address and the Lightning invoice. Either rail settles it. A client
//!   can pass the URI unchanged to its wallet with the NWC-321 `pay`
//!   method: the wallet pays `ark=` when the address is on its own Ark
//!   server (the address encodes the server), otherwise `lightning=`.
use std::str::FromStr;

use async_trait::async_trait;
use config::ConfigError;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderValue, Method, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper::{Body, Client};
use hyper_rustls::HttpsConnector;
use nostr::Keys;
use rand::random;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Settings;
use crate::error::{Error, Result};

use super::{InvoiceInfo, InvoiceStatus, PaymentProcessor};

/// Prefix of the invoice id used when there is no Lightning payment hash
/// (Ark-only payments): `ark:<address>`.
const ARK_ID_PREFIX: &str = "ark:";

/// Which payment rails the Bark processor offers
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BarkRails {
    /// Lightning invoices only
    Lightning,
    /// Ark (arkoor) payments only
    Ark,
    /// Both, as a BIP-321 URI
    #[default]
    Both,
}

/// barkd: `InvoiceInfo`
#[derive(Debug, Deserialize)]
struct BarkdInvoice {
    invoice: String,
}

/// barkd: `ArkAddressResponse`
#[derive(Debug, Deserialize)]
struct BarkdAddress {
    address: String,
}

/// barkd: `Bip321UriResponse`
#[derive(Debug, Deserialize)]
struct BarkdBip321 {
    ark: Option<String>,
    bolt11: Option<String>,
    bip321: String,
}

/// barkd: `LightningReceiveInfo` (the fields used here)
#[derive(Debug, Deserialize)]
struct BarkdReceive {
    payment_hash: String,
    state: String,
}

/// barkd: `Movement` (the fields used here)
#[derive(Debug, Deserialize)]
struct BarkdMovement {
    status: String,
    #[serde(default)]
    received_on: Vec<BarkdDestination>,
}

#[derive(Debug, Deserialize)]
struct BarkdDestination {
    destination: BarkdPaymentMethod,
    amount_sat: u64,
}

#[derive(Debug, Deserialize)]
struct BarkdPaymentMethod {
    #[serde(rename = "type")]
    kind: String,
    value: String,
}

#[derive(Clone)]
pub struct BarkPaymentProcessor {
    client: Client<HttpsConnector<HttpConnector>, Body>,
    /// barkd base URL, without a trailing slash
    base_url: String,
    /// `Bearer <token>` header for barkd
    auth_header: HeaderValue,
    rails: BarkRails,
}

impl BarkPaymentProcessor {
    pub fn new(settings: &Settings) -> Result<Self> {
        let p = &settings.pay_to_relay;
        let base_url = p.node_url.trim_end_matches('/').to_owned();
        Uri::from_str(&base_url).map_err(|_| ConfigError::Message("Bad node URL".to_string()))?;
        let mut auth_header = HeaderValue::from_str(&format!("Bearer {}", p.api_secret.trim()))
            .map_err(|_| ConfigError::Message("Invalid barkd token".to_string()))?;
        auth_header.set_sensitive(true);

        // barkd usually listens on localhost, so plain HTTP is allowed.
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder().build::<_, Body>(https);

        Ok(Self {
            client,
            base_url,
            auth_header,
            rails: p.bark_rails,
        })
    }

    /// Call barkd. `Ok(None)` means 404 Not Found.
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Option<T>> {
        let uri = Uri::from_str(&format!("{}{}", self.base_url, path))
            .map_err(|_| ConfigError::Message("Bad node URL".to_string()))?;
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(uri)
            .header(AUTHORIZATION, self.auth_header.clone());
        let body = match body {
            Some(b) => {
                req = req.header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                Body::from(serde_json::to_string(&b)?)
            }
            None => Body::empty(),
        };
        let req = req.body(body).expect("request builder");

        let res = self.client.request(req).await?;
        let status = res.status();
        let bytes = hyper::body::to_bytes(res.into_body()).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(Error::CustomError(format!(
                "barkd {} returned {}: {}",
                path,
                status,
                String::from_utf8_lossy(&bytes)
            )));
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    async fn call_required<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        self.call(method, path, body)
            .await?
            .ok_or_else(|| Error::CustomError(format!("barkd {path} not found")))
    }

    /// Payment hash of a Lightning invoice barkd created
    async fn payment_hash(&self, invoice: &str) -> Result<String> {
        let receive: BarkdReceive = self
            .call_required(
                Method::GET,
                &format!("/api/v1/lightning/receives/{invoice}"),
                None,
            )
            .await?;
        Ok(receive.payment_hash)
    }

    /// Lightning receive state, `None` if barkd does not know it
    async fn lightning_status(&self, payment_hash: &str) -> Result<Option<InvoiceStatus>> {
        let receive: Option<BarkdReceive> = self
            .call(
                Method::GET,
                &format!("/api/v1/lightning/receives/{payment_hash}"),
                None,
            )
            .await?;
        Ok(receive.map(|r| {
            if r.state == "settled" {
                InvoiceStatus::Paid
            } else {
                InvoiceStatus::Unpaid
            }
        }))
    }

    /// Total sats received over Ark on `address`, from successful movements
    async fn ark_received(&self, address: &str) -> Result<u64> {
        let movements: Vec<BarkdMovement> = self
            .call_required(Method::GET, "/api/v1/wallet/history", None)
            .await?;
        Ok(movements
            .iter()
            .filter(|m| m.status == "successful")
            .flat_map(|m| m.received_on.iter())
            .filter(|d| d.destination.kind == "ark" && d.destination.value == address)
            .map(|d| d.amount_sat)
            .sum())
    }
}

/// Format sats as a BTC amount for a BIP-321 URI
fn btc_amount(sats: u64) -> String {
    format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000)
}

/// The Ark address in an invoice record: either the id (`ark:<address>`)
/// or the `ark=` parameter of a BIP-321 payment request.
fn ark_address(invoice: &InvoiceInfo) -> Option<String> {
    if let Some(address) = invoice.payment_hash.strip_prefix(ARK_ID_PREFIX) {
        return Some(address.to_owned());
    }
    let request = &invoice.bolt11;
    if !request.to_ascii_lowercase().starts_with("bitcoin:") {
        return None;
    }
    let query = request.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k.eq_ignore_ascii_case("ark"))
        .map(|(_, v)| v.into_owned())
}

#[async_trait]
impl PaymentProcessor for BarkPaymentProcessor {
    async fn get_invoice(&self, key: &Keys, amount: u64) -> Result<InvoiceInfo, Error> {
        let random_number: u16 = random();
        let memo = format!("{}: {}", random_number, key.public_key());

        let (payment_hash, request) = match self.rails {
            BarkRails::Lightning => {
                let invoice: BarkdInvoice = self
                    .call_required(
                        Method::POST,
                        "/api/v1/lightning/receives/invoice",
                        Some(json!({ "amount_sat": amount, "description": memo })),
                    )
                    .await?;
                (self.payment_hash(&invoice.invoice).await?, invoice.invoice)
            }
            BarkRails::Ark => {
                let address: BarkdAddress = self
                    .call_required(Method::POST, "/api/v1/wallet/addresses/next", None)
                    .await?;
                let request = format!(
                    "bitcoin:?ark={}&amount={}",
                    address.address,
                    btc_amount(amount)
                );
                (format!("{ARK_ID_PREFIX}{}", address.address), request)
            }
            BarkRails::Both => {
                let uri: BarkdBip321 = self
                    .call_required(
                        Method::POST,
                        "/api/v1/wallet/bip321",
                        Some(json!({ "amount_sat": amount })),
                    )
                    .await?;
                if uri.ark.is_none() {
                    return Err(Error::CustomError(
                        "barkd returned no Ark address".to_string(),
                    ));
                }
                let bolt11 = uri.bolt11.ok_or_else(|| {
                    Error::CustomError("barkd returned no Lightning invoice".to_string())
                })?;
                (self.payment_hash(&bolt11).await?, uri.bip321)
            }
        };

        Ok(InvoiceInfo {
            pubkey: key.public_key().to_string(),
            payment_hash,
            bolt11: request,
            amount,
            memo,
            status: InvoiceStatus::Unpaid,
            confirmed_at: None,
        })
    }

    /// Check by id alone. Lightning ids are checked with barkd; an Ark-only
    /// id has no amount to compare against, so it stays unpaid here. The
    /// relay checks stored invoices with `check_invoice_info`.
    async fn check_invoice(&self, payment_hash: &str) -> Result<InvoiceStatus, Error> {
        if payment_hash.starts_with(ARK_ID_PREFIX) {
            return Ok(InvoiceStatus::Unpaid);
        }
        Ok(self
            .lightning_status(payment_hash)
            .await?
            .unwrap_or(InvoiceStatus::Expired))
    }

    async fn check_invoice_info(&self, invoice: &InvoiceInfo) -> Result<InvoiceStatus, Error> {
        let address = ark_address(invoice);

        let mut lightning_known = false;
        if !invoice.payment_hash.starts_with(ARK_ID_PREFIX) {
            match self.lightning_status(&invoice.payment_hash).await? {
                Some(InvoiceStatus::Paid) => return Ok(InvoiceStatus::Paid),
                Some(_) => lightning_known = true,
                None => {}
            }
        }

        if let Some(address) = address {
            if self.ark_received(&address).await? >= invoice.amount {
                return Ok(InvoiceStatus::Paid);
            }
            return Ok(InvoiceStatus::Unpaid);
        }

        if lightning_known {
            Ok(InvoiceStatus::Unpaid)
        } else {
            // barkd no longer knows this Lightning receive (canceled)
            Ok(InvoiceStatus::Expired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Request, Response, Server};
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "test-token";
    const HASH: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    /// What the mock barkd knows
    #[derive(Default)]
    struct MockState {
        /// state of the one Lightning receive (`None`: unknown / 404)
        receive_state: Option<String>,
        /// successful Ark receives: (address, sats)
        ark_receives: Vec<(String, u64)>,
        /// a pending (not yet successful) Ark receive
        pending_ark: Option<(String, u64)>,
        /// "METHOD path" of each request
        calls: Vec<String>,
        /// request bodies
        bodies: Vec<serde_json::Value>,
    }

    type Shared = Arc<Mutex<MockState>>;

    const INVOICE: &str = "lntbs42u1mockinvoice";
    const ADDRESS: &str = "tark1mockaddress0";

    fn movement(status: &str, address: &str, sats: u64) -> serde_json::Value {
        json!({
            "id": 1,
            "status": status,
            "subsystem": { "name": "bark.arkoor", "kind": "receive" },
            "intended_balance_sat": sats,
            "effective_balance_sat": sats,
            "offchain_fee_sat": 0,
            "sent_to": [],
            "received_on": [{ "destination": { "type": "ark", "value": address }, "amount_sat": sats }],
            "input_vtxos": [],
            "output_vtxos": [],
            "exited_vtxos": [],
        })
    }

    async fn handle(state: Shared, req: Request<Body>) -> Result<Response<Body>, Infallible> {
        let method = req.method().clone();
        let path = req.uri().path().to_owned();
        let authed = req
            .headers()
            .get(AUTHORIZATION)
            .is_some_and(|h| h == format!("Bearer {TOKEN}").as_str());
        let bytes = hyper::body::to_bytes(req.into_body()).await.unwrap();
        let mut s = state.lock().unwrap();
        s.calls.push(format!("{method} {path}"));
        if !bytes.is_empty() {
            s.bodies.push(serde_json::from_slice(&bytes).unwrap());
        }
        let reply = |code: u16, body: serde_json::Value| {
            Ok(Response::builder()
                .status(code)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap())
        };
        if !authed {
            return reply(401, json!({ "message": "unauthorized" }));
        }
        let receive = |s: &MockState| {
            s.receive_state.as_ref().map(|st| {
                json!({ "payment_hash": HASH, "state": st, "invoice": INVOICE,
                        "amount_sat": 4200, "htlc_vtxo_ids": [], "settled_at": null })
            })
        };
        match (method.as_str(), path.as_str()) {
            ("POST", "/api/v1/lightning/receives/invoice") => {
                s.receive_state = Some("awaiting-payment".to_owned());
                reply(200, json!({ "invoice": INVOICE }))
            }
            ("POST", "/api/v1/wallet/addresses/next") => reply(200, json!({ "address": ADDRESS })),
            ("POST", "/api/v1/wallet/bip321") => {
                s.receive_state = Some("awaiting-payment".to_owned());
                reply(
                    200,
                    json!({ "ark": ADDRESS, "bolt11": INVOICE,
                            "bip321": format!("bitcoin:?ark={ADDRESS}&lightning={INVOICE}&amount=0.00004200") }),
                )
            }
            ("GET", "/api/v1/wallet/history") => {
                let mut movements: Vec<_> = s
                    .ark_receives
                    .iter()
                    .map(|(a, sats)| movement("successful", a, *sats))
                    .collect();
                if let Some((a, sats)) = &s.pending_ark {
                    movements.push(movement("pending", a, *sats));
                }
                reply(200, json!(movements))
            }
            ("GET", p) if p.starts_with("/api/v1/lightning/receives/") => {
                let id = p.trim_start_matches("/api/v1/lightning/receives/");
                match receive(&s) {
                    Some(r) if id == HASH || id == INVOICE => reply(200, r),
                    _ => reply(404, json!({ "message": "not found" })),
                }
            }
            _ => reply(404, json!({ "message": "not found" })),
        }
    }

    /// Start a mock barkd and a processor pointed at it
    async fn setup(rails: BarkRails) -> (BarkPaymentProcessor, Shared) {
        let state: Shared = Arc::default();
        let st = state.clone();
        let make = make_service_fn(move |_| {
            let st = st.clone();
            async move { Ok::<_, Infallible>(service_fn(move |req| handle(st.clone(), req))) }
        });
        let server = Server::bind(&([127, 0, 0, 1], 0).into()).serve(make);
        let addr = server.local_addr();
        tokio::spawn(server);

        let mut settings = Settings::default();
        settings.pay_to_relay.processor = super::super::Processor::Bark;
        settings.pay_to_relay.node_url = format!("http://{addr}/");
        settings.pay_to_relay.api_secret = TOKEN.to_owned();
        settings.pay_to_relay.bark_rails = rails;
        (BarkPaymentProcessor::new(&settings).unwrap(), state)
    }

    fn keys() -> Keys {
        Keys::generate()
    }

    #[tokio::test]
    async fn lightning_invoice_and_settlement() {
        let (p, state) = setup(BarkRails::Lightning).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        assert_eq!(invoice.bolt11, INVOICE);
        assert_eq!(invoice.payment_hash, HASH);
        assert_eq!(invoice.amount, 4200);
        assert_eq!(state.lock().unwrap().bodies[0]["amount_sat"], 4200);

        assert_eq!(p.check_invoice(HASH).await.unwrap(), InvoiceStatus::Unpaid);
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );

        state.lock().unwrap().receive_state = Some("settled".to_owned());
        assert_eq!(p.check_invoice(HASH).await.unwrap(), InvoiceStatus::Paid);
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Paid
        );
    }

    #[tokio::test]
    async fn lightning_receive_in_progress_is_unpaid() {
        let (p, state) = setup(BarkRails::Lightning).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        state.lock().unwrap().receive_state = Some("htlcs-ready".to_owned());
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );
    }

    #[tokio::test]
    async fn lightning_unknown_receive_is_expired() {
        let (p, state) = setup(BarkRails::Lightning).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        state.lock().unwrap().receive_state = None;
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Expired
        );
        assert_eq!(p.check_invoice(HASH).await.unwrap(), InvoiceStatus::Expired);
    }

    #[tokio::test]
    async fn ark_address_and_settlement() {
        let (p, state) = setup(BarkRails::Ark).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        assert_eq!(invoice.payment_hash, format!("ark:{ADDRESS}"));
        assert_eq!(
            invoice.bolt11,
            format!("bitcoin:?ark={ADDRESS}&amount=0.00004200")
        );
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );

        // a pending receive does not count
        state.lock().unwrap().pending_ark = Some((ADDRESS.to_owned(), 4200));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );

        // payments to other addresses do not count
        state
            .lock()
            .unwrap()
            .ark_receives
            .push(("tark1someoneelse".to_owned(), 10_000));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );

        // partial payments add up
        state
            .lock()
            .unwrap()
            .ark_receives
            .push((ADDRESS.to_owned(), 2000));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );
        state
            .lock()
            .unwrap()
            .ark_receives
            .push((ADDRESS.to_owned(), 2200));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Paid
        );
        // by id alone there is no amount to check against
        assert_eq!(
            p.check_invoice(&invoice.payment_hash).await.unwrap(),
            InvoiceStatus::Unpaid
        );
    }

    #[tokio::test]
    async fn both_rails_bip321_paid_over_lightning() {
        let (p, state) = setup(BarkRails::Both).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        assert_eq!(invoice.payment_hash, HASH);
        assert!(invoice.bolt11.starts_with("bitcoin:?ark="));
        assert!(invoice.bolt11.contains(INVOICE));
        assert_eq!(ark_address(&invoice).as_deref(), Some(ADDRESS));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Unpaid
        );
        state.lock().unwrap().receive_state = Some("settled".to_owned());
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Paid
        );
    }

    #[tokio::test]
    async fn both_rails_bip321_paid_over_ark() {
        let (p, state) = setup(BarkRails::Both).await;
        let invoice = p.get_invoice(&keys(), 4200).await.unwrap();
        state
            .lock()
            .unwrap()
            .ark_receives
            .push((ADDRESS.to_owned(), 4200));
        assert_eq!(
            p.check_invoice_info(&invoice).await.unwrap(),
            InvoiceStatus::Paid
        );
    }

    #[tokio::test]
    async fn sends_bearer_token_and_reports_errors() {
        let (mut p, state) = setup(BarkRails::Lightning).await;
        p.get_invoice(&keys(), 1).await.unwrap();
        assert_eq!(
            state.lock().unwrap().calls,
            vec![
                "POST /api/v1/lightning/receives/invoice",
                &format!("GET /api/v1/lightning/receives/{INVOICE}"),
            ]
        );

        p.auth_header = HeaderValue::from_static("Bearer wrong");
        let err = p.get_invoice(&keys(), 1).await.unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
    }

    #[test]
    fn btc_amounts() {
        assert_eq!(btc_amount(4200), "0.00004200");
        assert_eq!(btc_amount(150_000_000), "1.50000000");
    }

    #[test]
    fn rails_config_names() {
        let r: BarkRails = serde_json::from_str("\"lightning\"").unwrap();
        assert_eq!(r, BarkRails::Lightning);
        let r: BarkRails = serde_json::from_str("\"ark\"").unwrap();
        assert_eq!(r, BarkRails::Ark);
        assert_eq!(BarkRails::default(), BarkRails::Both);
    }
}
