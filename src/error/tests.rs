use super::*;
use std::collections::BTreeSet;
use std::error::Error as _;
use std::fmt;

#[test]
fn constructors_set_known_kind() {
    type Case = (fn() -> AnyError, &'static str, &'static str);
    let table: [Case; 12] = [
        (not_connected, kind::NOT_CONNECTED, "Not connected"),
        (
            tcp_connection_failed,
            kind::TCP_CONNECTION_FAILED,
            "TCP connection failed",
        ),
        (
            connection_closed,
            kind::CONNECTION_CLOSED,
            "TCP connection closed by the peer",
        ),
        (
            iso_connection_failed,
            kind::ISO_CONNECTION_FAILED,
            "ISO-on-TCP connection failed",
        ),
        (
            iso_fragmented_packet,
            kind::ISO_FRAGMENTED_PACKET,
            "Fragmented ISO Packet",
        ),
        (
            iso_invalid_header,
            kind::ISO_INVALID_HEADER,
            "Invalid ISO Header",
        ),
        (
            iso_invalid_telegram,
            kind::ISO_INVALID_TELEGRAM,
            "Invalid ISO Telegram",
        ),
        (
            pdu_negotiation_failed,
            kind::PDU_NEGOTIATION_FAILED,
            "S7 PDU negotiation failed",
        ),
        (
            invalid_fun_parameter,
            kind::INVALID_FUN_PARAMETER,
            "Invalid parameter supplied to the function",
        ),
        (
            s7_not_found,
            kind::S7_NOT_FOUND,
            "S7 Resource not found in the CPU",
        ),
        (
            s7_invalid_address,
            kind::S7_INVALID_ADDRESS,
            "S7 Invalid address",
        ),
        (s7_unspecified, kind::S7_UNSPECIFIED, "S7 unspecified error"),
    ];
    for (make, expected, msg) in table {
        let err = make();
        assert_eq!(err.get_type(), Some(expected));
        assert!(kind::ALL.contains(&expected));
        assert_eq!(err.to_string(), format!("{expected}: {msg}"));
        assert!(err.source().is_none());
    }
    assert_eq!(
        io(std::io::ErrorKind::TimedOut.into()).get_type(),
        Some(kind::IO)
    );
    assert_eq!(kind::ALL.len(), 13);
}

#[test]
fn breaks_session_table() {
    let table: [(&str, bool); 13] = [
        (kind::IO, true),
        (kind::NOT_CONNECTED, false),
        (kind::TCP_CONNECTION_FAILED, false),
        (kind::CONNECTION_CLOSED, true),
        (kind::ISO_CONNECTION_FAILED, false),
        (kind::ISO_FRAGMENTED_PACKET, true),
        (kind::ISO_INVALID_HEADER, true),
        (kind::ISO_INVALID_TELEGRAM, true),
        (kind::PDU_NEGOTIATION_FAILED, false),
        (kind::INVALID_FUN_PARAMETER, false),
        (kind::S7_NOT_FOUND, false),
        (kind::S7_INVALID_ADDRESS, false),
        (kind::S7_UNSPECIFIED, false),
    ];
    let keys: BTreeSet<&str> = table.iter().map(|(k, _)| *k).collect();
    let all: BTreeSet<&str> = kind::ALL.iter().copied().collect();
    assert_eq!(keys, all);
    for (k, expected) in table {
        let err = AnyError::error("x").with_type(Some(k));
        assert_eq!(breaks_session(&err), expected, "{k}");
    }
    assert!(breaks_session(&AnyError::error("x")));
    assert!(breaks_session(
        &AnyError::error("x").with_type(Some("other::Kind"))
    ));
}

#[derive(Debug)]
struct Inner;

impl fmt::Display for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("root cause")
    }
}

impl std::error::Error for Inner {}

#[derive(Debug)]
struct Outer;

impl fmt::Display for Outer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("outer cause")
    }
}

impl std::error::Error for Outer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&Inner)
    }
}

#[test]
fn io_error_round_trip() {
    let err = io(std::io::Error::other(Outer)).add_context(|| "read_db db=7");
    assert_eq!(err.get_type(), Some(kind::IO));
    let text = err.to_string();
    assert!(text.contains("outer cause"), "{text}");
    assert!(text.contains("read_db db=7"), "{text}");
    assert!(err.source().unwrap().to_string().contains("root cause"));

    let json = serde_json::to_string(&err).unwrap();
    let decoded: AnyError = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, err);
    assert_eq!(decoded.get_type(), Some(kind::IO));
    assert_eq!(
        decoded.source().unwrap().to_string(),
        err.source().unwrap().to_string()
    );
}

#[test]
fn kind_values_are_stable() {
    assert_eq!(
        kind::ALL,
        [
            "rust7::Io",
            "rust7::NotConnected",
            "rust7::TcpConnectionFailed",
            "rust7::ConnectionClosed",
            "rust7::IsoConnectionFailed",
            "rust7::IsoFragmentedPacket",
            "rust7::IsoInvalidHeader",
            "rust7::IsoInvalidTelegram",
            "rust7::PduNegotiationFailed",
            "rust7::InvalidFunParameter",
            "rust7::S7NotFound",
            "rust7::S7InvalidAddress",
            "rust7::S7Unspecified",
        ]
    );
}
