use super::*;

#[test]
fn public_bit_helpers_accept_full_byte_range_without_panicking() {
    let mut client = S7Client::new();
    for byte in [8191, 8192, 65535] {
        for bit in [0, 7] {
            assert!(matches!(
                client.read_bit(S7_AREA_DB, 1, byte, bit),
                Err(S7Error::NotConnected)
            ));
            assert!(matches!(
                client.write_bit(S7_AREA_DB, 1, byte, bit, true),
                Err(S7Error::NotConnected)
            ));
        }
    }
}

#[test]
fn invalid_public_inputs_clear_statistics_and_return_errors() {
    let mut client = S7Client::new();
    client.last_time = 42.0;
    client.chunks = 9;
    assert!(matches!(
        client.read_bit(S7_AREA_DB, 1, 0, 8),
        Err(S7Error::InvalidFunParameter)
    ));
    assert_eq!((client.last_time, client.chunks), (0.0, 0));
    assert!(matches!(
        client.write_bit(S7_AREA_DB, 1, 0, 8, false),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.read_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &mut []),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.write_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &[]),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.read_area(0, 1, 0, S7_WL_BYTE, &mut [0]),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.write_area(S7_AREA_DB, 1, 0, 0, &[0]),
        Err(S7Error::InvalidFunParameter)
    ));
    let buffer = vec![0; 0x200000 - 65535 + 1];
    assert!(matches!(
        client.write_db(1, 65535, &buffer),
        Err(S7Error::InvalidFunParameter)
    ));
}

#[test]
fn public_status_fields_cannot_create_a_connection() {
    let mut client = S7Client::new();
    client.connected = true;
    client.pdu_length = 65535;
    assert!(matches!(
        client.read_db(1, 0, &mut [0]),
        Err(S7Error::NotConnected)
    ));
    assert!(matches!(
        client.write_db(1, 0, &[0]),
        Err(S7Error::NotConnected)
    ));
    client.disconnect();
    assert!(!client.connected);
    assert!(client.stream.is_none());
    assert!(client.limits.is_none());
}

#[test]
fn invalid_rack_or_slot_is_rejected_before_connecting() {
    let mut client = S7Client::new();
    for (rack, slot) in [(8, 0), (0, 32), (2048, 0)] {
        assert!(matches!(
            client.connect_rack_slot("127.0.0.1", rack, slot),
            Err(S7Error::InvalidFunParameter)
        ));
    }
    assert!(!client.connected && client.stream.is_none());
}

#[test]
fn only_transport_errors_break_the_session() {
    for e in [
        S7Error::Io(std::io::ErrorKind::TimedOut.into()),
        S7Error::ConnectionClosed,
        S7Error::IsoInvalidHeader,
        S7Error::IsoInvalidTelegram,
        S7Error::IsoFragmentedPacket,
    ] {
        assert!(e.breaks_session(), "{e}");
    }
    for e in [
        S7Error::S7InvalidAddress,
        S7Error::S7NotFound,
        S7Error::S7Unspecified,
        S7Error::InvalidFunParameter,
        S7Error::NotConnected,
    ] {
        assert!(!e.breaks_session(), "{e}");
    }
}

#[test]
fn s7_error_is_std_error_with_io_source() {
    let err: Box<dyn std::error::Error> = Box::new(S7Error::from(std::io::Error::from(
        std::io::ErrorKind::TimedOut,
    )));
    assert!(err.source().is_some());
    assert!(std::error::Error::source(&S7Error::NotConnected).is_none());
}
