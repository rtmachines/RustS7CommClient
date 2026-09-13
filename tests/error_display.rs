//! Regression guard for the thiserror migration:
//! the `Display` text of every `S7Error` variant must stay byte-identical
//! to the hand-written `impl Display` it replaced.

use std::error::Error;
use std::io;

use rust7::S7Error;

#[test]
fn display_text_is_unchanged() {
    let cases: Vec<(S7Error, &str)> = vec![
        (S7Error::NotConnected, "Not connected"),
        (S7Error::TcpConnectionFailed, "TCP connection failed"),
        (S7Error::ConnectionClosed, "TCP connection closed by the peer"),
        (S7Error::IsoConnectionFailed, "ISO-on-TCP connection failed"),
        (S7Error::IsoFragmentedPacket, "Fragmented ISO Packet"),
        (S7Error::IsoInvalidHeader, "Invalid ISO Header"),
        (S7Error::IsoInvalidTelegram, "Invalid ISO Telegram"),
        (S7Error::PduNegotiationFailed, "S7 PDU negotiation failed"),
        (
            S7Error::InvalidFunParameter,
            "Invalid parameter supplied to the function",
        ),
        (S7Error::S7NotFound, "S7 Resource not found in the CPU"),
        (S7Error::S7InvalidAddress, "S7 Invalid address"),
        (S7Error::S7Unspecified, "S7 unspecified error"),
        (S7Error::Timeout, "Operation timed out"),
        (S7Error::S7InvalidResponse, "Invalid S7 response"),
        (
            S7Error::WriteIncomplete { acknowledged: 212, requested: 300, source: Box::new(S7Error::Timeout) },
            "Write incomplete: 212 of 300 bytes acknowledged: Operation timed out",
        ),
    ];

    for (err, expected) in cases {
        assert_eq!(err.to_string(), expected, "Display changed for {err:?}");
    }
}

#[test]
fn io_variant_display_is_unchanged() {
    let err = S7Error::Io(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
    assert_eq!(err.to_string(), "IO error: timed out");
}

#[test]
fn from_io_error_still_converts() {
    let err: S7Error = io::Error::new(io::ErrorKind::ConnectionRefused, "refused").into();
    assert!(matches!(err, S7Error::Io(_)));
}

#[test]
fn from_io_error_normalises_timeouts_and_disconnects() {
    let timeout: S7Error = io::Error::from(io::ErrorKind::WouldBlock).into();
    assert!(matches!(timeout, S7Error::Timeout));
    let eof: S7Error = io::Error::from(io::ErrorKind::UnexpectedEof).into();
    assert!(matches!(eof, S7Error::ConnectionClosed));
}

#[test]
fn write_incomplete_exposes_its_source() {
    let err = S7Error::WriteIncomplete { acknowledged: 0, requested: 1, source: Box::new(S7Error::S7NotFound) };
    assert_eq!(err.source().unwrap().to_string(), "S7 Resource not found in the CPU");
    assert!(err.is_plc_error());
    assert!(!err.requires_reconnect());
}

#[test]
fn implements_std_error_with_source() {
    // New capability: `S7Error` now implements `std::error::Error`,
    // so it composes with `?` into `Box<dyn Error>` / `anyhow`.
    let err = S7Error::Io(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
    assert!(err.source().is_some(), "Io variant should expose its source");
    assert_eq!(err.source().unwrap().to_string(), "timed out");

    assert!(S7Error::NotConnected.source().is_none());

    let boxed: Box<dyn Error> = Box::new(S7Error::NotConnected);
    assert_eq!(boxed.to_string(), "Not connected");
}
