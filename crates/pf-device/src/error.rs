use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeviceError {
    #[error("could not list serial ports: {0}")]
    ListPorts(String),
    #[error("could not open {port}: {reason}")]
    Open { port: String, reason: String },
    #[error("could not start the connection thread: {0}")]
    Thread(String),
}
