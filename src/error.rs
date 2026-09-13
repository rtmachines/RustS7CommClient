// Rust7 - Native Rust S7 client (Snap7‑style) for Siemens PLCs.
// Copyright 2025 - Davide Nardella

use std::io;

use thiserror::Error;

/// Every failure `S7Client` can report.
///
/// The variants fall into three groups; use [`S7Error::requires_reconnect`]
/// and [`S7Error::is_plc_error`] instead of matching on them by hand:
///
/// | Group | Variants | Connection afterwards | Retry |
/// | :-- | :-- | :-- | :-- |
/// | **Parameter** | `InvalidFunParameter`, `NotConnected` | unchanged | fix the call |
/// | **PLC rejected the job** | `S7NotFound`, `S7InvalidAddress`, `S7Unspecified` | still connected, in sync | same request will fail again |
/// | **Transport / protocol** | `Io`, `Timeout`, `ConnectionClosed`, `TcpConnectionFailed`, `IsoConnectionFailed`, `Iso*`, `S7InvalidResponse`, `PduNegotiationFailed` | **dropped** by the client | reconnect first |
///
/// `WriteIncomplete` wraps any of the above raised in the middle of a
/// `write_area` and reports how many bytes the CPU had already acknowledged.
#[derive(Debug, Error)]
pub enum S7Error {
    /// Network I/O failure other than a timeout or a closed peer.
    #[error("IO error: {0}")]
    Io(#[source] io::Error),
    /// The socket read or write timed out (`TimedOut` / `WouldBlock`).
    ///
    /// For a write this means the request may or may not have been applied.
    #[error("Operation timed out")]
    Timeout,
    #[error("Not connected")]
    NotConnected,
    #[error("TCP connection failed")]
    TcpConnectionFailed,
    #[error("TCP connection closed by the peer")]
    ConnectionClosed,
    #[error("ISO-on-TCP connection failed")]
    IsoConnectionFailed,
    #[error("Fragmented ISO Packet")]
    IsoFragmentedPacket,
    #[error("Invalid ISO Header")]
    IsoInvalidHeader,
    #[error("Invalid ISO Telegram")]
    IsoInvalidTelegram,
    /// A well-framed telegram that is not the acknowledgement of the request
    /// just sent (wrong PDU reference, message type, function or item size).
    #[error("Invalid S7 response")]
    S7InvalidResponse,
    #[error("S7 PDU negotiation failed")]
    PduNegotiationFailed,
    #[error("Invalid parameter supplied to the function")]
    InvalidFunParameter,
    #[error("S7 Resource not found in the CPU")]
    S7NotFound,
    #[error("S7 Invalid address")]
    S7InvalidAddress,
    #[error("S7 unspecified error")]
    S7Unspecified,
    /// A write failed after some of its data had already been sent.
    ///
    /// `acknowledged` bytes (a prefix of the buffer) were confirmed by the
    /// CPU. Whether the chunk that failed was applied is unknown when
    /// `source` is a transport error; it was **not** applied when `source`
    /// is a PLC rejection.
    #[error("Write incomplete: {acknowledged} of {requested} bytes acknowledged: {source}")]
    WriteIncomplete {
        acknowledged: usize,
        requested: usize,
        #[source]
        source: Box<S7Error>,
    },
}

impl S7Error {
    /// `true` when the client has dropped the connection because of this
    /// error; call one of the `connect_*` methods before the next operation.
    pub fn requires_reconnect(&self) -> bool {
        match self {
            S7Error::Io(_)
            | S7Error::Timeout
            | S7Error::TcpConnectionFailed
            | S7Error::ConnectionClosed
            | S7Error::IsoConnectionFailed
            | S7Error::IsoFragmentedPacket
            | S7Error::IsoInvalidHeader
            | S7Error::IsoInvalidTelegram
            | S7Error::S7InvalidResponse
            | S7Error::PduNegotiationFailed => true,
            S7Error::NotConnected
            | S7Error::InvalidFunParameter
            | S7Error::S7NotFound
            | S7Error::S7InvalidAddress
            | S7Error::S7Unspecified => false,
            S7Error::WriteIncomplete { source, .. } => source.requires_reconnect(),
        }
    }

    /// `true` when the CPU received the request and rejected it: the
    /// connection is intact and repeating the same request is pointless.
    pub fn is_plc_error(&self) -> bool {
        match self {
            S7Error::S7NotFound | S7Error::S7InvalidAddress | S7Error::S7Unspecified => true,
            S7Error::WriteIncomplete { source, .. } => source.is_plc_error(),
            _ => false,
        }
    }
}

impl From<io::Error> for S7Error {
    /// Normalises the platform-dependent timeout and disconnect kinds so
    /// callers never have to match `WouldBlock` vs `TimedOut` themselves.
    fn from(e: io::Error) -> Self {
        use io::ErrorKind::*;
        match e.kind() {
            TimedOut | WouldBlock => S7Error::Timeout,
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe => {
                S7Error::ConnectionClosed
            }
            _ => S7Error::Io(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_timeouts_normalise_to_timeout() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::WouldBlock] {
            assert!(
                matches!(S7Error::from(io::Error::from(kind)), S7Error::Timeout),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn io_disconnects_normalise_to_connection_closed() {
        use io::ErrorKind::*;
        for kind in [
            UnexpectedEof,
            ConnectionReset,
            ConnectionAborted,
            BrokenPipe,
        ] {
            assert!(
                matches!(
                    S7Error::from(io::Error::from(kind)),
                    S7Error::ConnectionClosed
                ),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn other_io_kinds_stay_io() {
        let e = S7Error::from(io::Error::from(io::ErrorKind::ConnectionRefused));
        assert!(
            matches!(e, S7Error::Io(ref inner) if inner.kind() == io::ErrorKind::ConnectionRefused)
        );
    }

    #[test]
    fn write_incomplete_inherits_classification_from_its_source() {
        let transport = S7Error::WriteIncomplete {
            acknowledged: 1,
            requested: 2,
            source: Box::new(S7Error::Timeout),
        };
        assert!(transport.requires_reconnect());
        assert!(!transport.is_plc_error());

        let plc = S7Error::WriteIncomplete {
            acknowledged: 1,
            requested: 2,
            source: Box::new(S7Error::S7NotFound),
        };
        assert!(!plc.requires_reconnect());
        assert!(plc.is_plc_error());
        assert_eq!(
            plc.to_string(),
            "Write incomplete: 1 of 2 bytes acknowledged: S7 Resource not found in the CPU"
        );
    }
}
