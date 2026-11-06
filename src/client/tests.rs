use super::*;
use crate::error::{assert_kind, kind};

#[test]
fn public_bit_helpers_accept_full_byte_range_without_panicking() {
    let mut client = S7Client::new();
    for byte in [8191, 8192, 65535] {
        for bit in [0, 7] {
            assert_kind(
                &client.read_bit(S7_AREA_DB, 1, byte, bit),
                kind::NOT_CONNECTED,
            );
            assert_kind(
                &client.write_bit(S7_AREA_DB, 1, byte, bit, true),
                kind::NOT_CONNECTED,
            );
        }
    }
}

#[test]
fn invalid_public_inputs_clear_statistics_and_return_errors() {
    let mut client = S7Client::new();
    client.last_time = 42.0;
    client.chunks = 9;
    assert_kind(
        &client.read_bit(S7_AREA_DB, 1, 0, 8),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_eq!((client.last_time, client.chunks), (0.0, 0));
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
    assert!(client.stream.is_none());
    assert!(client.limits.is_none());
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
    assert!(!client.connected && client.stream.is_none());
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
            client.limits = Some(protocol::PduLimits::new(125).unwrap());
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
                assert!(client.limits.is_none());
                assert_eq!(client.pdu_length, 0);
            } else {
                assert_eq!(error.get_type(), Some(kind::S7_UNSPECIFIED));
                assert!(client.connected);
                assert_eq!(client.limits.unwrap().pdu_length, 125);
                assert_eq!(client.pdu_length, 125);
            }
        }
    }
}

#[test]
fn invalid_area_keeps_active_session() {
    let mut client = S7Client::new();
    client.connected = true;
    client.pdu_length = 125;
    client.limits = Some(protocol::PduLimits::new(125).unwrap());
    let result = client.read_area(0x99, 1, 0, S7_WL_BYTE, &mut [0; 2]);
    assert_kind(&result, kind::INVALID_FUN_PARAMETER);
    assert!(client.connected);
    assert_eq!(client.pdu_length, 125);
    assert!(client.limits.is_some());
}
