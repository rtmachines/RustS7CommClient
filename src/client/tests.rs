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
