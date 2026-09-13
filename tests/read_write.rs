//! Wire format, chunking, error mapping and bit access of the read/write API,
//! tested against the in-process fake PLC.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::*;
use rust7::{S7Error, S7_AREA_DB, S7_AREA_MK, S7_AREA_PA, S7_AREA_PE, S7_WL_BIT, S7_WL_BYTE};

/// Predicate used in table-driven error-mapping tests (`S7Error` is neither `PartialEq` nor `Clone`).
type ErrorMatcher = fn(&S7Error) -> bool;

fn plc(pdu_length: u16) -> FakePlc {
    FakePlc::start(Handshake::Accept { pdu_length }, echo_memory_handler)
}

fn expected_memory(db: u16, start: u16, len: usize) -> Vec<u8> {
    (0..len as u32).map(|i| memory_byte(db, start as u32 + i)).collect()
}

fn single_request(plc: &FakePlc) -> ParsedRequest {
    let requests = plc.requests();
    assert_eq!(requests.len(), 1, "exactly one frame expected, got {}", requests.len());
    parse_request(&requests[0])
}

// ---- reads -------------------------------------------------------------------

#[test]
fn read_db_fills_buffer_with_plc_data() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; 64];

    client.read_db(100, 10, &mut buf).unwrap();

    assert_eq!(buf, expected_memory(100, 10, 64));
    assert_eq!(client.chunks(), 1);
    assert!(client.last_time() > 0.0);
}

#[test]
fn read_request_frame_matches_s7comm_layout() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; 64];
    client.read_db(100, 10, &mut buf).unwrap();

    let raw = &plc.requests()[0];
    assert_eq!(raw.len(), READ_REQ_LEN);
    assert_eq!(&raw[9..11], &[0x00, 0x00], "redundancy id");
    assert_ne!(pdu_ref(raw), 0, "pdu reference is never 0");
    assert_eq!(raw[18], 0x01, "item count");

    let req = parse_request(raw);
    assert_eq!(req.telegram_len as usize, READ_REQ_LEN);
    assert_eq!(req.function, FN_READ_VAR);
    assert_eq!(req.param_len, 14);
    assert_eq!(req.data_len, 0, "reads carry no data");
    assert_eq!(req.wordlen, S7_WL_BYTE);
    assert_eq!(req.count, 64);
    assert_eq!(req.db_number, 100);
    assert_eq!(req.area, S7_AREA_DB);
    assert_eq!(req.address, 10 * 8, "byte start is converted to a bit address");
}

#[test]
fn large_read_is_split_by_negotiated_pdu() {
    let plc = plc(240); // 240 - 18 = 222 bytes per read
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; 462];

    client.read_db(100, 0, &mut buf).unwrap();

    assert_eq!(buf, expected_memory(100, 0, 462));
    assert_eq!(client.chunks(), 3);
    let reqs: Vec<ParsedRequest> = plc.requests().iter().map(|f| parse_request(f)).collect();
    assert_eq!(reqs.iter().map(|r| r.count).collect::<Vec<_>>(), [222, 222, 18]);
    assert_eq!(
        reqs.iter().map(|r| r.byte_address()).collect::<Vec<_>>(),
        [0, 222, 444],
        "each chunk continues where the previous one ended"
    );
}

#[test]
fn read_exactly_one_max_chunk_is_not_split() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; max_read_chunk(240)];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(client.chunks(), 1);
    assert_eq!(buf, expected_memory(1, 0, 222));
}

#[test]
fn read_chunk_size_follows_negotiated_pdu() {
    let plc = plc(480); // 480 - 18 = 462 bytes per read
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; 1000];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(client.chunks(), 3);
    let reqs: Vec<ParsedRequest> = plc.requests().iter().map(|f| parse_request(f)).collect();
    assert_eq!(reqs.iter().map(|r| r.count).collect::<Vec<_>>(), [462, 462, 76]);
    assert_eq!(buf, expected_memory(1, 0, 1000));
}

#[test]
fn read_of_empty_buffer_sends_nothing() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf: [u8; 0] = [];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(client.chunks(), 0);
    assert!(plc.requests().is_empty());
}

#[test]
fn read_start_offset_shifts_the_bit_address() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = [0u8; 1];

    client.read_db(1, 0x1234, &mut buf).unwrap();

    assert_eq!(single_request(&plc).address, 0x1234 << 3);
    assert_eq!(buf, [memory_byte(1, 0x1234)]);
}

#[test]
fn non_db_areas_are_forwarded_unchanged() {
    for area in [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK] {
        let plc = plc(240);
        let mut client = plc.connected_client();
        let mut buf = [0u8; 8];

        client.read_area(area, 0, 4, S7_WL_BYTE, &mut buf).unwrap();

        let req = single_request(&plc);
        assert_eq!(req.area, area);
        assert_eq!(req.db_number, 0);
        assert_eq!(req.byte_address(), 4);
    }
}

#[test]
fn read_return_codes_map_to_s7_errors() {
    let cases: [(u8, ErrorMatcher); 4] = [
        (RES_NOT_FOUND, |e| matches!(e, S7Error::S7NotFound)),
        (RES_INVALID_ADDRESS, |e| matches!(e, S7Error::S7InvalidAddress)),
        (0x07, |e| matches!(e, S7Error::S7Unspecified)),
        (0x00, |e| matches!(e, S7Error::S7Unspecified)),
    ];
    for (code, is_expected) in cases {
        let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |_| {
            Reply::Send(read_reply(code, &[]))
        });
        let mut client = plc.connected_client();
        let mut buf = [0u8; 8];

        let err = client.read_db(1, 0, &mut buf).unwrap_err();

        assert!(is_expected(&err), "return code {code:#04x} produced {err:?}");
        assert_eq!(client.last_time(), 0.0);
        assert!(client.is_connected(), "protocol level errors do not disconnect");
    }
}

#[test]
fn read_error_aborts_after_the_failing_chunk() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |frame| {
        let n = seen.fetch_add(1, Ordering::SeqCst);
        if n == 1 {
            return Reply::Send(read_reply(RES_INVALID_ADDRESS, &[]));
        }
        echo_memory_handler(frame)
    });
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; 700]; // would need 4 chunks

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::S7InvalidAddress));
    assert_eq!(plc.requests().len(), 2, "no further chunks after the failure");
    assert_eq!(client.chunks(), 2);
}

#[test]
fn fragmented_iso_reply_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        let mut reply = read_reply(RES_SUCCESS, &[1, 2, 3, 4]);
        reply[6] = 0x00; // EOT cleared
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::IsoFragmentedPacket)));
}

#[test]
fn reply_with_invalid_iso_header_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        let mut reply = read_reply(RES_SUCCESS, &[1, 2, 3, 4]);
        reply[5] = 0xE0; // not a COTP data PDU
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::IsoInvalidHeader)));
}

#[test]
fn reply_longer_than_negotiated_pdu_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        Reply::Send(read_reply(RES_SUCCESS, &[0xAA; 300]))
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::IsoInvalidTelegram)));
}

#[test]
fn read_reply_shorter_than_its_header_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        // a well-formed ack header, but the item is one byte short
        let reply = read_reply(RES_SUCCESS, &[]);
        Reply::Send(raw_telegram(READ_RES_LEN - 1, &reply[7..7 + READ_RES_LEN - 1]))
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::S7InvalidResponse)));
    assert!(!client.is_connected());
}

#[test]
fn read_reply_shorter_than_the_s7_header_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        Reply::SendRaw(raw_telegram(5, &[0u8; 5]))
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::IsoInvalidTelegram)));
    assert!(!client.is_connected());
}

#[test]
fn read_reply_truncated_below_announced_length_times_out() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        // announces 30 S7 bytes, ships only 10, then goes quiet
        Reply::Send(raw_telegram(30, &[0u8; 10]))
    });
    let mut client = plc.client();
    client.set_timeout(3000, 100, 500).unwrap();
    client.connect_s71200_1500(&plc.ip()).unwrap();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::Timeout)));
    assert!(!client.is_connected());
}

#[test]
fn peer_closing_mid_request_yields_connection_closed() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| Reply::Close);
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::ConnectionClosed), "{err:?}");
    assert_eq!(client.last_time(), 0.0);
    assert!(!client.is_connected());
}

#[test]
fn successful_read_after_error_restores_timing_fields() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |frame| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Send(read_reply(RES_NOT_FOUND, &[]))
        } else {
            echo_memory_handler(frame)
        }
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(client.read_db(1, 0, &mut buf).is_err());
    assert_eq!((client.last_time(), client.chunks()), (0.0, 1));

    client.read_db(1, 0, &mut buf).unwrap();
    assert!(client.last_time() > 0.0);
    assert_eq!(client.chunks(), 1);
}

// ---- writes ------------------------------------------------------------------

#[test]
fn write_db_sends_payload_in_write_var_frame() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let data: Vec<u8> = (0..16).collect();

    client.write_db(5, 20, &data).unwrap();

    assert_eq!(client.chunks(), 1);
    assert!(client.last_time() > 0.0);
    let raw = &plc.requests()[0];
    assert_eq!(raw.len(), WRITE_REQ_HEADER_LEN + 16);
    assert_eq!(raw[31], 0x00, "reserved byte");

    let req = parse_request(raw);
    assert_eq!(req.telegram_len as usize, raw.len());
    assert_eq!(req.function, FN_WRITE_VAR);
    assert_eq!(req.param_len, 14);
    assert_eq!(req.data_len, 16 + 4, "data length = payload + 4-byte item header");
    assert_eq!(req.wordlen, S7_WL_BYTE);
    assert_eq!(req.count, 16);
    assert_eq!(req.db_number, 5);
    assert_eq!(req.area, S7_AREA_DB);
    assert_eq!(req.address, 20 * 8);
    assert_eq!(req.transport, Some(TS_RES_BYTE));
    assert_eq!(req.bit_count, Some(16 * 8));
    assert_eq!(req.payload, data);
}

#[test]
fn large_write_is_split_by_negotiated_pdu() {
    let plc = plc(240); // 240 - 28 = 212 bytes per write
    let mut client = plc.connected_client();
    let data: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();

    client.write_db(100, 0, &data).unwrap();

    assert_eq!(client.chunks(), 5);
    let reqs: Vec<ParsedRequest> = plc.requests().iter().map(|f| parse_request(f)).collect();
    assert_eq!(reqs.iter().map(|r| r.count).collect::<Vec<_>>(), [212, 212, 212, 212, 176]);
    assert_eq!(reqs.iter().map(|r| r.byte_address()).collect::<Vec<_>>(), [0, 212, 424, 636, 848]);
    let reassembled: Vec<u8> = reqs.iter().flat_map(|r| r.payload.clone()).collect();
    assert_eq!(reassembled, data);
}

#[test]
fn write_of_empty_buffer_sends_nothing() {
    let plc = plc(240);
    let mut client = plc.connected_client();

    client.write_db(1, 0, &[]).unwrap();

    assert_eq!(client.chunks(), 0);
    assert!(plc.requests().is_empty());
}

#[test]
fn write_return_codes_map_to_s7_errors() {
    let cases: [(u8, ErrorMatcher); 3] = [
        (RES_NOT_FOUND, |e| matches!(e, S7Error::S7NotFound)),
        (RES_INVALID_ADDRESS, |e| matches!(e, S7Error::S7InvalidAddress)),
        (0x03, |e| matches!(e, S7Error::S7Unspecified)),
    ];
    for (code, is_expected) in cases {
        let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |_| {
            Reply::Send(write_reply(code))
        });
        let mut client = plc.connected_client();

        let err = client.write_db(1, 0, &[1, 2, 3]).unwrap_err();

        assert!(is_expected(&err), "return code {code:#04x} produced {err:?}");
        assert_eq!(client.last_time(), 0.0);
    }
}

#[test]
fn write_reply_shorter_than_its_header_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| {
        let reply = write_reply(RES_SUCCESS);
        Reply::Send(raw_telegram(WRITE_RES_LEN - 1, &reply[7..7 + WRITE_RES_LEN - 1]))
    });
    let mut client = plc.connected_client();

    let err = client.write_db(1, 0, &[1]).unwrap_err();

    assert!(
        matches!(err, S7Error::WriteIncomplete { acknowledged: 0, requested: 1, ref source }
            if matches!(**source, S7Error::S7InvalidResponse)),
        "{err:?}"
    );
    assert!(!client.is_connected());
}

#[test]
fn write_error_aborts_after_the_failing_chunk() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 2 {
            Reply::Send(write_reply(RES_NOT_FOUND))
        } else {
            Reply::Send(write_reply(RES_SUCCESS))
        }
    });
    let mut client = plc.connected_client();

    let err = client.write_db(1, 0, &[0u8; 1000]).unwrap_err(); // 5 chunks needed

    assert!(
        matches!(err, S7Error::WriteIncomplete { acknowledged: 424, requested: 1000, ref source }
            if matches!(**source, S7Error::S7NotFound)),
        "{err:?}"
    );
    assert_eq!(plc.requests().len(), 3);
    assert_eq!(client.chunks(), 3);
    assert!(client.is_connected(), "a PLC rejection keeps the connection");
}

// ---- bit access ----------------------------------------------------------------

#[test]
fn read_bit_encodes_byte_times_8_plus_bit_and_reads_one_element() {
    let plc = plc(240);
    let mut client = plc.connected_client();

    let value = client.read_bit(S7_AREA_DB, 100, 47, 5).unwrap();

    let req = single_request(&plc);
    assert_eq!(req.function, FN_READ_VAR);
    assert_eq!(req.wordlen, S7_WL_BIT);
    assert_eq!(req.count, 1);
    assert_eq!(req.db_number, 100);
    assert_eq!(req.area, S7_AREA_DB);
    assert_eq!(req.address, 47 * 8 + 5);
    assert_eq!(value, (memory_byte(100, 47) >> 5) & 1 == 1);
    assert_eq!(client.chunks(), 1);
}

#[test]
fn read_bit_interprets_any_nonzero_byte_as_true() {
    for (payload, expected) in [([0u8], false), ([1u8], true), ([0x80u8], true), ([0xFFu8], true)] {
        let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, move |_| {
            Reply::Send(read_reply(RES_SUCCESS, &payload))
        });
        let mut client = plc.connected_client();
        assert_eq!(client.read_bit(S7_AREA_MK, 0, 0, 0).unwrap(), expected, "payload {payload:?}");
    }
}

#[test]
fn read_area_with_bit_wordlen_uses_start_as_bit_address_and_reads_one_element() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = [0xEEu8; 4]; // deliberately larger than one element

    client.read_area(S7_AREA_DB, 3, 363, S7_WL_BIT, &mut buf).unwrap();

    let req = single_request(&plc);
    assert_eq!(req.wordlen, S7_WL_BIT);
    assert_eq!(req.count, 1, "bit access transfers exactly one element");
    assert_eq!(req.address, 363, "bit start is not shifted");
    assert_eq!(buf[0], (memory_byte(3, 45) >> 3) & 1);
    assert_eq!(&buf[1..], &[0xEE; 3], "only the first byte is touched");
}

#[test]
fn write_bit_encodes_address_transport_and_value() {
    for (value, byte) in [(true, 1u8), (false, 0u8)] {
        let plc = plc(240);
        let mut client = plc.connected_client();

        client.write_bit(S7_AREA_MK, 0, 12, 3, value).unwrap();

        let req = single_request(&plc);
        assert_eq!(req.function, FN_WRITE_VAR);
        assert_eq!(req.wordlen, S7_WL_BIT);
        assert_eq!(req.count, 1);
        assert_eq!(req.db_number, 0);
        assert_eq!(req.area, S7_AREA_MK);
        assert_eq!(req.address, 12 * 8 + 3);
        assert_eq!(req.transport, Some(TS_RES_BIT));
        assert_eq!(req.bit_count, Some(1));
        assert_eq!(req.data_len, 1 + 4);
        assert_eq!(req.payload, [byte]);
        assert_eq!(req.telegram_len as usize, WRITE_REQ_HEADER_LEN + 1);
    }
}

#[test]
fn write_area_with_bit_wordlen_sends_only_the_first_byte() {
    let plc = plc(240);
    let mut client = plc.connected_client();

    client.write_area(S7_AREA_PA, 0, 9, S7_WL_BIT, &[1, 2, 3, 4]).unwrap();

    let req = single_request(&plc);
    assert_eq!(req.payload, [1]);
    assert_eq!(req.count, 1);
    assert_eq!(req.address, 9);
    assert_eq!(client.chunks(), 1);
}

// ---- parameter validation while connected ---------------------------------------

#[test]
fn parameter_errors_do_not_put_anything_on_the_wire() {
    let plc = plc(240);
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(client.read_area(0x00, 1, 0, S7_WL_BYTE, &mut buf), Err(S7Error::InvalidFunParameter)));
    assert!(matches!(client.read_area(S7_AREA_DB, 1, 0, 0x03, &mut buf), Err(S7Error::InvalidFunParameter)));
    assert!(matches!(client.write_area(0xFF, 1, 0, S7_WL_BYTE, &buf), Err(S7Error::InvalidFunParameter)));
    assert!(matches!(client.write_area(S7_AREA_DB, 1, 0, 0x00, &buf), Err(S7Error::InvalidFunParameter)));
    assert!(matches!(client.read_bit(S7_AREA_DB, 1, 0, 8), Err(S7Error::InvalidFunParameter)));
    assert!(matches!(client.write_bit(S7_AREA_DB, 1, 0, 9, true), Err(S7Error::InvalidFunParameter)));

    assert!(plc.requests().is_empty());
    assert!(client.is_connected());
    assert_eq!((client.last_time(), client.chunks()), (0.0, 0));
}
