//! Error type of the crate: `AnyError` tagged with a stable category from [`kind`].

// removed in Task 2
#![cfg_attr(not(test), allow(dead_code))]

pub use anyerror::AnyError;

pub type Result<T> = std::result::Result<T, AnyError>;

pub mod kind {
    pub const IO: &str = "rust7::Io";
    pub const NOT_CONNECTED: &str = "rust7::NotConnected";
    pub const TCP_CONNECTION_FAILED: &str = "rust7::TcpConnectionFailed";
    pub const CONNECTION_CLOSED: &str = "rust7::ConnectionClosed";
    pub const ISO_CONNECTION_FAILED: &str = "rust7::IsoConnectionFailed";
    pub const ISO_FRAGMENTED_PACKET: &str = "rust7::IsoFragmentedPacket";
    pub const ISO_INVALID_HEADER: &str = "rust7::IsoInvalidHeader";
    pub const ISO_INVALID_TELEGRAM: &str = "rust7::IsoInvalidTelegram";
    pub const PDU_NEGOTIATION_FAILED: &str = "rust7::PduNegotiationFailed";
    pub const INVALID_FUN_PARAMETER: &str = "rust7::InvalidFunParameter";
    pub const S7_NOT_FOUND: &str = "rust7::S7NotFound";
    pub const S7_INVALID_ADDRESS: &str = "rust7::S7InvalidAddress";
    pub const S7_UNSPECIFIED: &str = "rust7::S7Unspecified";
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
mod tests;
