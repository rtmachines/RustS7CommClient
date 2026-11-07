use super::*;
use crate::error::{assert_kind, kind};
use anyerror::AnyError;

#[test]
fn public_bit_helpers_accept_full_byte_range_without_panicking() {
    let mut client = S7Client::new();
    for byte in [8191, 8192, 65535] {
        for bit in [0, 7] {
            client.confirmed = 9;
            assert_kind(
                &client.read_bit(S7_AREA_DB, 1, byte, bit),
                kind::NOT_CONNECTED,
            );
            assert_eq!(client.confirmed, 0);
            client.confirmed = 9;
            assert_kind(
                &client.write_bit(S7_AREA_DB, 1, byte, bit, true),
                kind::NOT_CONNECTED,
            );
            assert_eq!(client.confirmed, 0);
        }
    }
}

#[test]
fn invalid_public_inputs_clear_statistics_and_return_errors() {
    let mut client = S7Client::new();
    client.last_time = 42.0;
    client.chunks = 9;
    client.confirmed = 9;
    assert_kind(
        &client.read_bit(S7_AREA_DB, 1, 0, 8),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_eq!(
        (client.last_time, client.chunks, client.confirmed),
        (0.0, 0, 0)
    );
    assert_kind(
        &client.write_bit(S7_AREA_DB, 1, 0, 8, false),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &client.read_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &mut []),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &client.write_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &[]),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &client.read_area(0, 1, 0, S7_WL_BYTE, &mut [0]),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &client.write_area(S7_AREA_DB, 1, 0, 0, &[0]),
        kind::INVALID_FUN_PARAMETER,
    );
    let buffer = vec![0; 0x200000 - 65535 + 1];
    assert_kind(
        &client.write_db(1, 65535, &buffer),
        kind::INVALID_FUN_PARAMETER,
    );
}

#[test]
fn public_status_fields_cannot_create_a_connection() {
    let mut client = S7Client::new();
    client.connected = true;
    client.pdu_length = 65535;
    assert_kind(&client.read_db(1, 0, &mut [0]), kind::NOT_CONNECTED);
    assert_kind(&client.write_db(1, 0, &[0]), kind::NOT_CONNECTED);
    client.disconnect();
    assert!(!client.connected);
    assert!(client.session.is_none());
}

#[test]
fn invalid_rack_or_slot_is_rejected_before_connecting() {
    let mut client = S7Client::new();
    for (rack, slot) in [(8, 0), (0, 32), (2048, 0)] {
        assert_kind(
            &client.connect_rack_slot("127.0.0.1", rack, slot),
            kind::INVALID_FUN_PARAMETER,
        );
    }
    assert!(!client.connected && client.session.is_none());
}

#[test]
fn only_transport_errors_break_the_session() {
    for e in [
        error::io(std::io::ErrorKind::TimedOut.into()),
        error::connection_closed(),
        error::iso_invalid_header(),
        error::iso_invalid_telegram(),
        error::iso_fragmented_packet(),
    ] {
        assert!(error::breaks_session(&e), "{e}");
    }
    for e in [
        error::s7_invalid_address(),
        error::s7_not_found(),
        error::s7_header_error(0x81, 4),
        error::s7_item_error(3),
        error::invalid_fun_parameter(),
        error::not_connected(),
    ] {
        assert!(!error::breaks_session(&e), "{e}");
    }
}

#[test]
fn finish_preserves_session_only_for_valid_error_ack() {
    for writing in [false, true] {
        for malformed in [false, true] {
            let mut client = S7Client::new();
            client.connected = true;
            client.pdu_length = 125;
            let mut pdu = [0x32, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0x81, 4];
            if malformed {
                pdu[5] = 2; // A PLC error with the wrong reference is a framing error.
            }
            let result = if writing {
                protocol::validate_write_response(&pdu, 1)
            } else {
                protocol::validate_read_response(&pdu, 1, 2, 4).map(|_| ())
            };
            let error = client.finish(result, Instant::now()).unwrap_err();
            if malformed {
                assert_eq!(error.get_type(), Some(kind::ISO_INVALID_TELEGRAM));
                assert!(!client.connected);
                assert_eq!(client.pdu_length, 0);
            } else {
                assert_eq!(error.get_type(), Some(kind::S7_UNSPECIFIED));
                assert!(client.connected);
                assert_eq!(client.pdu_length, 125);
            }
        }
    }
}

#[test]
fn finish_applies_the_session_rule_of_every_kind() {
    let table = [
        (kind::IO, error::io(std::io::Error::other("x")), true),
        (kind::NOT_CONNECTED, error::not_connected(), false),
        (
            kind::TCP_CONNECTION_FAILED,
            error::tcp_connection_failed(),
            false,
        ),
        (kind::CONNECTION_CLOSED, error::connection_closed(), true),
        (
            kind::ISO_CONNECTION_FAILED,
            error::iso_connection_failed(),
            false,
        ),
        (
            kind::ISO_FRAGMENTED_PACKET,
            error::iso_fragmented_packet(),
            true,
        ),
        (kind::ISO_INVALID_HEADER, error::iso_invalid_header(), true),
        (
            kind::ISO_INVALID_TELEGRAM,
            error::iso_invalid_telegram(),
            true,
        ),
        (
            kind::PDU_NEGOTIATION_FAILED,
            error::pdu_negotiation_failed(),
            false,
        ),
        (
            kind::INVALID_FUN_PARAMETER,
            error::invalid_fun_parameter(),
            false,
        ),
        (kind::S7_NOT_FOUND, error::s7_not_found(), false),
        (kind::S7_INVALID_ADDRESS, error::s7_invalid_address(), false),
        (kind::S7_UNSPECIFIED, error::s7_header_error(0x81, 4), false),
    ];
    let mut kinds: Vec<&str> = table.iter().map(|(kind, _, _)| *kind).collect();
    kinds.sort_unstable();
    let mut all = kind::ALL.to_vec();
    all.sort_unstable();
    assert_eq!(kinds, all);

    let connected_client = || {
        let mut client = S7Client::new();
        client.connected = true;
        client.pdu_length = 125;
        client
    };
    for (expected, err, breaks) in table {
        let mut client = connected_client();
        let e = client.finish(Err(err), Instant::now()).unwrap_err();
        assert_eq!(e.get_type(), Some(expected));
        if breaks {
            assert!(!client.connected, "{expected} must disconnect");
            assert_eq!(client.pdu_length, 0);
        } else {
            assert!(client.connected, "{expected} must keep the session");
            assert_eq!(client.pdu_length, 125);
        }
    }
    for err in [
        AnyError::error("x"),
        AnyError::error("x").with_type(Some("other::X")),
    ] {
        let mut client = connected_client();
        client.finish(Err(err), Instant::now()).unwrap_err();
        assert!(!client.connected);
        assert_eq!(client.pdu_length, 0);
    }
}

#[test]
fn invalid_area_keeps_active_session() {
    let mut client = S7Client::new();
    client.connected = true;
    client.pdu_length = 125;
    let result = client.read_area(0x99, 1, 0, S7_WL_BYTE, &mut [0; 2]);
    assert_kind(&result, kind::INVALID_FUN_PARAMETER);
    assert!(client.connected);
    assert_eq!(client.pdu_length, 125);
}

#[test]
fn default_client_has_documented_settings() {
    let client = S7Client::default();
    assert_eq!(client.port, 102);
    assert_eq!(
        (
            client.co_timeout_ms,
            client.rd_timeout_ms,
            client.wr_timeout_ms
        ),
        (3000, 1000, 500)
    );
    assert_eq!(client.conn_type, CT_PG);
    assert!(!client.connected && client.session.is_none());
    assert_eq!(
        (
            client.pdu_length,
            client.reference,
            client.last_time,
            client.chunks,
            client.confirmed
        ),
        (0, 0, 0.0, 0, 0)
    );
}

#[test]
fn connection_type_accepts_only_pg_op_and_s7() {
    let mut client = S7Client::new();
    for connection_type in [CT_OP, CT_S7, CT_PG, CT_S7] {
        client.set_connection_type(connection_type).unwrap();
        assert_eq!(client.conn_type, connection_type);
    }
    for connection_type in [0, CT_S7 + 1, u16::MAX] {
        assert_kind(
            &client.set_connection_type(connection_type),
            kind::INVALID_FUN_PARAMETER,
        );
        assert_eq!(client.conn_type, CT_S7, "a rejected value is not stored");
    }
}

#[test]
fn every_timeout_must_be_positive() {
    let mut client = S7Client::new();
    client.set_timeout(1, 1, 1).unwrap();
    for (co, rd, wr) in [(0, 7, 7), (7, 0, 7), (7, 7, 0), (0, 0, 0)] {
        assert_kind(&client.set_timeout(co, rd, wr), kind::INVALID_FUN_PARAMETER);
        assert_eq!(
            (
                client.co_timeout_ms,
                client.rd_timeout_ms,
                client.wr_timeout_ms
            ),
            (1, 1, 1),
            "({co}, {rd}, {wr}) must not be stored"
        );
    }
    client.set_timeout(u64::MAX, 2, 3).unwrap();
    assert_eq!(
        (
            client.co_timeout_ms,
            client.rd_timeout_ms,
            client.wr_timeout_ms
        ),
        (u64::MAX, 2, 3)
    );
}

#[test]
fn connection_port_must_be_nonzero() {
    let mut client = S7Client::new();
    assert_kind(&client.set_connection_port(0), kind::INVALID_FUN_PARAMETER);
    assert_eq!(client.port, 102);
    for port in [1, 65535] {
        client.set_connection_port(port).unwrap();
        assert_eq!(client.port, port);
    }
}

#[test]
fn invalid_write_area_keeps_active_session() {
    let mut client = S7Client::new();
    client.connected = true;
    client.pdu_length = 125;
    for area in [0x80, 0x85] {
        let result = client.write_area(area, 1, 0, S7_WL_BYTE, &[0; 2]);
        assert_kind(&result, kind::INVALID_FUN_PARAMETER);
        assert!(client.connected);
        assert_eq!(client.pdu_length, 125);
    }
}

#[test]
fn public_arguments_map_to_transfer() {
    use protocol::{bit_payload, bit_value, Transfer};

    for area in [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB] {
        assert_eq!(
            Transfer::area(area, 9, 0x1234, S7_WL_BYTE),
            Transfer {
                area,
                db_number: 9,
                start: 0x1234,
                wordlen: S7_WL_BYTE,
            }
        );
    }
    assert_eq!(
        Transfer::db(7, 3),
        Transfer {
            area: S7_AREA_DB,
            db_number: 7,
            start: 3,
            wordlen: S7_WL_BYTE,
        }
    );
    let edge = Transfer::db(u16::MAX, u16::MAX);
    assert_eq!(edge.db_number, 65535);
    assert_eq!(edge.start, 65535);

    assert_eq!(
        Transfer::bit(S7_AREA_MK, 5, 10, 0).unwrap(),
        Transfer {
            area: S7_AREA_MK,
            db_number: 5,
            start: 80,
            wordlen: S7_WL_BIT,
        }
    );
    let bit = Transfer::bit(S7_AREA_DB, 12, 10, 3).unwrap();
    assert_eq!(bit.start, 83);
    assert_eq!(bit.db_number, 12);
    assert_eq!(
        Transfer::bit(S7_AREA_DB, 1, 65535, 7).unwrap().start,
        524287
    );
    assert_kind(
        &Transfer::bit(S7_AREA_DB, 1, 0, 8),
        kind::INVALID_FUN_PARAMETER,
    );

    assert_eq!(bit_payload(false), [0]);
    assert_eq!(bit_payload(true), [1]);
    assert!(!bit_value(0));
    assert!(bit_value(1));
    assert!(bit_value(5));
}
