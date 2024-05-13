use pyo3::exceptions::PyOSError;
use pyo3::prelude::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("No test suite is active, check the call to 'On Suite Setup'")]
    NoSuiteActive,

    #[error(transparent)]
    AttrKeyVal(#[from] auxon_sdk::reflector_config::AttrKeyValuePairParseError),

    #[error("Encountered an ingest client initialization error. {0}")]
    IngestClientInitialization(#[from] auxon_sdk::ingest_client::IngestClientInitializationError),

    #[error("Encountered an ingest client error. {0}")]
    Ingest(#[from] auxon_sdk::ingest_client::IngestError),

    #[error("Encountered an ingest client error. {0}")]
    DynamicIngest(#[from] auxon_sdk::ingest_client::dynamic::DynamicIngestError),

    #[error(transparent)]
    AuthDes(#[from] auxon_sdk::auth_token::AuthTokenStringDeserializationError),

    #[error(transparent)]
    AuthLoad(#[from] auxon_sdk::auth_token::LoadAuthTokenError),

    #[error(
        "Encountered and IO error while reading the input stream ({})",
        .0.kind()
    )]
    Io(#[from] std::io::Error),
}

impl From<Error> for PyErr {
    fn from(err: Error) -> PyErr {
        PyOSError::new_err(err.to_string())
    }
}
