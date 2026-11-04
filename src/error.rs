//! Error type of the crate: [`AnyError`] tagged with a stable category from [`kind`].
//!
//! Compare the category with the constants in [`kind`]; the strings are a stable
//! contract. Every public client method adds one context (method name and
//! parameters, never buffer contents), so `{e}` prints the category, message,
//! context and causes on one line and `{e:#}` prints them on separate lines.
//!
//! ```
//! fn configure(c: &mut rust7::S7Client) -> rust7::Result<()> {
//!     c.set_connection_port(102)?;
//!     Ok(())
//! }
//!
//! let mut client = rust7::S7Client::new();
//! assert!(configure(&mut client).is_ok());
//! ```
//!
//! ```
//! let mut client = rust7::S7Client::new();
//! let e = client.set_connection_port(0).unwrap_err();
//! assert_eq!(e.get_type(), Some(rust7::kind::INVALID_FUN_PARAMETER));
//! ```
//!
//! `S7Error` from 0.1 no longer exists:
//!
//! ```compile_fail
//! use rust7::S7Error;
//! ```
//!
//! ```compile_fail
//! use rust7::client::S7Error;
//! ```

pub use anyerror::AnyError;

/// Result alias with [`AnyError`] as the default error type; the second parameter keeps
/// `Result<T, E>` usable after `use rust7::*;`.
pub type Result<T, E = AnyError> = std::result::Result<T, E>;

/// Stable category strings returned by [`AnyError::get_type`](crate::AnyError::get_type).
pub mod kind {
    /// Socket or name-resolution I/O failure. Breaks the session.
    pub const IO: &str = "rust7::Io";
    /// The operation needs a connected client. No session to break.
    pub const NOT_CONNECTED: &str = "rust7::NotConnected";
    /// The address resolves to no socket address. Raised while connecting; a failed connect leaves the client disconnected (any previous session was already closed).
    pub const TCP_CONNECTION_FAILED: &str = "rust7::TcpConnectionFailed";
    /// The peer closed the TCP connection. Breaks the session.
    pub const CONNECTION_CLOSED: &str = "rust7::ConnectionClosed";
    /// ISO-on-TCP connection request was rejected or malformed. Raised while connecting; a failed connect leaves the client disconnected (any previous session was already closed).
    pub const ISO_CONNECTION_FAILED: &str = "rust7::IsoConnectionFailed";
    /// Fragmented ISO packet (COTP fragmentation is unsupported). Breaks the session.
    pub const ISO_FRAGMENTED_PACKET: &str = "rust7::IsoFragmentedPacket";
    /// Invalid TPKT/ISO header. Breaks the session.
    pub const ISO_INVALID_HEADER: &str = "rust7::IsoInvalidHeader";
    /// Inconsistent or malformed telegram. Breaks the session.
    pub const ISO_INVALID_TELEGRAM: &str = "rust7::IsoInvalidTelegram";
    /// S7 PDU size negotiation failed. Raised while connecting; a failed connect leaves the client disconnected (any previous session was already closed).
    pub const PDU_NEGOTIATION_FAILED: &str = "rust7::PduNegotiationFailed";
    /// A parameter was rejected before any I/O. Keeps the session.
    pub const INVALID_FUN_PARAMETER: &str = "rust7::InvalidFunParameter";
    /// The PLC reports that the resource (for example a DB) does not exist. Keeps the session.
    pub const S7_NOT_FOUND: &str = "rust7::S7NotFound";
    /// The PLC rejected the address (out of range or optimized DB). Keeps the session.
    pub const S7_INVALID_ADDRESS: &str = "rust7::S7InvalidAddress";
    /// Other error reported by the PLC. Keeps the session.
    pub const S7_UNSPECIFIED: &str = "rust7::S7Unspecified";
    /// All 13 categories.
    pub const ALL: &[&str] = &[
        IO,
        NOT_CONNECTED,
        TCP_CONNECTION_FAILED,
        CONNECTION_CLOSED,
        ISO_CONNECTION_FAILED,
        ISO_FRAGMENTED_PACKET,
        ISO_INVALID_HEADER,
        ISO_INVALID_TELEGRAM,
        PDU_NEGOTIATION_FAILED,
        INVALID_FUN_PARAMETER,
        S7_NOT_FOUND,
        S7_INVALID_ADDRESS,
        S7_UNSPECIFIED,
    ];
}

fn domain(kind: &'static str, msg: &'static str) -> AnyError {
    AnyError::error(msg).with_type(Some(kind))
}

pub(crate) fn io(e: std::io::Error) -> AnyError {
    AnyError::new(&e).with_type(Some(kind::IO))
}

pub(crate) fn not_connected() -> AnyError {
    domain(kind::NOT_CONNECTED, "Not connected")
}

pub(crate) fn tcp_connection_failed() -> AnyError {
    domain(kind::TCP_CONNECTION_FAILED, "TCP connection failed")
}

pub(crate) fn connection_closed() -> AnyError {
    domain(kind::CONNECTION_CLOSED, "TCP connection closed by the peer")
}

pub(crate) fn iso_connection_failed() -> AnyError {
    domain(kind::ISO_CONNECTION_FAILED, "ISO-on-TCP connection failed")
}

pub(crate) fn iso_fragmented_packet() -> AnyError {
    domain(kind::ISO_FRAGMENTED_PACKET, "Fragmented ISO Packet")
}

pub(crate) fn iso_invalid_header() -> AnyError {
    domain(kind::ISO_INVALID_HEADER, "Invalid ISO Header")
}

pub(crate) fn iso_invalid_telegram() -> AnyError {
    domain(kind::ISO_INVALID_TELEGRAM, "Invalid ISO Telegram")
}

pub(crate) fn pdu_negotiation_failed() -> AnyError {
    domain(kind::PDU_NEGOTIATION_FAILED, "S7 PDU negotiation failed")
}

pub(crate) fn invalid_fun_parameter() -> AnyError {
    domain(
        kind::INVALID_FUN_PARAMETER,
        "Invalid parameter supplied to the function",
    )
}

pub(crate) fn s7_not_found() -> AnyError {
    domain(kind::S7_NOT_FOUND, "S7 Resource not found in the CPU")
}

pub(crate) fn s7_invalid_address() -> AnyError {
    domain(kind::S7_INVALID_ADDRESS, "S7 Invalid address")
}

pub(crate) fn s7_unspecified() -> AnyError {
    domain(kind::S7_UNSPECIFIED, "S7 unspecified error")
}

/// Whether the error leaves the connection in an unusable state.
/// Errors without a known category are treated as session-breaking.
pub(crate) fn breaks_session(e: &AnyError) -> bool {
    match e.get_type() {
        Some(k) if kind::ALL.contains(&k) => matches!(
            k,
            kind::IO
                | kind::CONNECTION_CLOSED
                | kind::ISO_INVALID_HEADER
                | kind::ISO_INVALID_TELEGRAM
                | kind::ISO_FRAGMENTED_PACKET
        ),
        _ => true,
    }
}

#[cfg(test)]
#[track_caller]
pub(crate) fn assert_kind<T>(result: &Result<T>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error of kind {expected}, got Ok"),
        Err(e) if e.get_type() == Some(expected) => {}
        Err(e) => panic!(
            "expected error of kind {expected}, got {:?}: {e}",
            e.get_type()
        ),
    }
}

#[cfg(test)]
mod tests;
