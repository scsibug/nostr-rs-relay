//! Error handling.

use std::result;
use thiserror::Error;

pub type Result<T, E = Error> = result::Result<T, E>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("command from client not recognized")]
    CommandNotFound,
    #[error("parsing JSON->Event failed")]
    EventParseFailed,
    #[error("parsing JSON->Req failed")]
    ReqParseFailed,
    #[error("parsing JSON->Close failed")]
    CloseParseFailed,
}
