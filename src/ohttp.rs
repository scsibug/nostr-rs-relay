use ohttp::hpke::{Aead, Kdf, Kem};
use ohttp::SymmetricSuite;

const KEY_ID: u8 = 1;
const KEM: Kem = Kem::K256Sha256;
const SYMMETRIC: &[SymmetricSuite] =
    &[SymmetricSuite::new(Kdf::HkdfSha256, Aead::ChaCha20Poly1305)];

#[derive(Debug, thiserror::Error)]
pub enum OhttpError {
    #[error("ohttp error: {0}")]
    Ohttp(ohttp::Error),
}

#[derive(Debug, Clone)]
pub struct ServerKeyConfig {
    pub(crate) server: ohttp::Server,
}

impl From<ServerKeyConfig> for ohttp::Server {
    fn from(value: ServerKeyConfig) -> Self {
        value.server
    }
}

/// Generate a new OHTTP server key configuration
pub fn gen_ohttp_server_config() -> Result<ServerKeyConfig, OhttpError> {
    let config =
        ohttp::KeyConfig::new(KEY_ID, KEM, Vec::from(SYMMETRIC)).map_err(OhttpError::Ohttp)?;
    Ok(ServerKeyConfig {
        server: ohttp::Server::new(config).map_err(OhttpError::Ohttp)?,
    })
}
