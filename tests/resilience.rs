//! Fault-injection tests: stale / split / short / late replies, timeouts in
//! the middle of multi-chunk writes, and argument limits. Each test is the
//! regression guard for one finding of the v0.1.2 review.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use rust7::{S7Client, S7Error, S7_AREA_DB, S7_WL_BIT, S7_WL_BYTE};

fn plc_with(handler: impl Fn(&[u8]) -> Reply + Send + Sync + 'static) -> FakePlc {
    FakePlc::start(Handshake::Accept { pdu_length: 240 }, handler)
}

/// Connected client with a short read timeout so timeout tests stay fast.
fn fast_client(plc: &FakePlc) -> S7Client {
    let mut client = plc.client();
    client.set_timeout(3000, 100, 500).unwrap();
    client.connect_s71200_1500(&plc.ip()).unwrap();
    client
}

fn memory(db: u16, start: u32, len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| memory_byte(db, start + i))
        .collect()
}

/// Answers reads from the deterministic memory model, like `echo_memory_handler`.
fn echo_read(frame: &[u8]) -> Vec<u8> {
    let req = parse_request(frame);
    read_reply(
        RES_SUCCESS,
        &memory(req.db_number, req.byte_address(), req.count as usize),
    )
}

// ---- request / reply correlation ---------------------------------------------

#[test]
fn consecutive_requests_carry_distinct_pdu_references() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];
    client.read_db(1, 0, &mut buf).unwrap();
    client.read_db(1, 0, &mut buf).unwrap();
    client.write_db(1, 0, &buf).unwrap();

    let refs: Vec<u16> = plc.requests().iter().map(|f| pdu_ref(f)).collect();
    assert_eq!(refs.len(), 3);
    assert_ne!(refs[0], refs[1]);
    assert_ne!(refs[1], refs[2]);
}

#[test]
fn late_reply_after_timeout_is_not_delivered_to_the_next_read() {
    // Request 1 (DB 1) is answered 400 ms late, after the 100 ms read timeout.
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = plc_with(move |frame| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Delayed(Duration::from_millis(400), echo_read(frame))
        } else {
            Reply::Send(echo_read(frame))
        }
    });
    let mut client = fast_client(&plc);
    let mut buf = [0u8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();
    assert!(matches!(err, S7Error::Timeout), "{err:?}");
    assert!(err.requires_reconnect());
    assert!(!client.is_connected(), "a timeout must drop the connection");
    std::thread::sleep(Duration::from_millis(500)); // the late reply lands on the closed socket

    // The caller is told to reconnect; a read on the dead client fails cleanly ...
    assert!(matches!(
        client.read_db(2, 0, &mut buf),
        Err(S7Error::NotConnected)
    ));

    // ... and after reconnecting, DB 2 delivers DB 2 data, never the stale DB 1 reply.
    client.connect_s71200_1500(&plc.ip()).unwrap();
    client.read_db(2, 0, &mut buf).unwrap();
    assert_eq!(buf.to_vec(), memory(2, 0, 4));
}

#[test]
fn reply_with_foreign_pdu_reference_is_rejected_and_drops_connection() {
    let plc = plc_with(|frame| {
        let mut reply = echo_read(frame);
        let wrong = pdu_ref(frame).wrapping_add(1).to_be_bytes();
        reply[11..13].copy_from_slice(&wrong);
        Reply::SendRaw(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0xEEu8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::S7InvalidResponse), "{err:?}");
    assert!(!client.is_connected());
    assert_eq!(buf, [0xEE; 4], "no payload from an uncorrelated reply");
}

#[test]
fn read_reply_to_a_write_request_is_rejected() {
    // A read-var ack (return code 0xFF at the same offset) must not pass as a write ack.
    let plc = plc_with(|_| Reply::Send(read_reply(RES_SUCCESS, &[1, 2, 3, 4])));
    let mut client = plc.connected_client();

    let err = client.write_db(1, 0, &[9, 9, 9, 9]).unwrap_err();

    assert!(
        matches!(err, S7Error::WriteIncomplete { acknowledged: 0, requested: 4, ref source }
            if matches!(**source, S7Error::S7InvalidResponse)),
        "{err:?}"
    );
    assert!(!client.is_connected());
}

#[test]
fn reply_with_wrong_message_type_is_rejected() {
    let plc = plc_with(|frame| {
        let mut reply = echo_read(frame);
        reply[8] = 0x01; // "job" instead of "ack-data"
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(
        client.read_db(1, 0, &mut buf),
        Err(S7Error::S7InvalidResponse)
    ));
    assert!(!client.is_connected());
}

#[test]
fn ack_with_error_class_is_a_plc_error_and_keeps_the_connection() {
    let plc = plc_with(|frame| {
        let mut reply = echo_read(frame);
        reply[17] = 0x81; // error class: application relationship
        reply[18] = 0x04;
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::S7Unspecified), "{err:?}");
    assert!(!err.requires_reconnect());
    assert!(
        client.is_connected(),
        "a well-formed negative ack leaves the stream in sync"
    );
}

// ---- framing ----------------------------------------------------------------------

#[test]
fn reply_split_across_two_tcp_segments_is_reassembled() {
    let plc = plc_with(|frame| {
        let reply = echo_read(frame);
        let cut = 7 + 18 + 1; // TPKT+COTP, S7 header, one payload byte
        Reply::Split(
            reply[..cut].to_vec(),
            Duration::from_millis(200),
            reply[cut..].to_vec(),
        )
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(buf.to_vec(), memory(1, 0, 4));
    assert!(client.is_connected());
}

#[test]
fn iso_header_split_across_two_tcp_segments_is_reassembled() {
    let plc = plc_with(|frame| {
        let reply = echo_read(frame);
        Reply::Split(
            reply[..3].to_vec(),
            Duration::from_millis(200),
            reply[3..].to_vec(),
        )
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(buf.to_vec(), memory(1, 0, 4));
}

#[test]
fn short_success_payload_is_rejected_and_buffer_left_untouched() {
    let plc = plc_with(|_| Reply::Send(read_reply(RES_SUCCESS, &[0x11, 0x22])));
    let mut client = plc.connected_client();
    let mut buf = [0xEEu8; 8];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::S7InvalidResponse), "{err:?}");
    assert_eq!(
        buf, [0xEE; 8],
        "nothing must be copied from a reply of the wrong size"
    );
    assert!(!client.is_connected());
}

#[test]
fn oversized_success_payload_is_rejected() {
    let plc = plc_with(|_| Reply::Send(read_reply(RES_SUCCESS, &[0x11; 20])));
    let mut client = plc.connected_client();
    let mut buf = [0xEEu8; 4];

    assert!(matches!(
        client.read_db(1, 0, &mut buf),
        Err(S7Error::S7InvalidResponse)
    ));
    assert_eq!(buf, [0xEE; 4]);
}

#[test]
fn declared_item_length_must_match_the_request() {
    // Correct byte count on the wire, but the item header claims 16 bits instead of 32.
    let plc = plc_with(|frame| {
        let mut reply = echo_read(frame);
        reply[23] = 0x00;
        reply[24] = 0x10;
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    assert!(matches!(
        client.read_db(1, 0, &mut buf),
        Err(S7Error::S7InvalidResponse)
    ));
}

#[test]
fn octet_string_transport_size_is_accepted_for_byte_reads() {
    // Some CPUs answer byte reads with transport 0x09 (length in bytes, not bits).
    let plc = plc_with(|frame| {
        let req = parse_request(frame);
        let mut reply = echo_read(frame);
        reply[22] = 0x09;
        reply[23..25].copy_from_slice(&req.count.to_be_bytes());
        Reply::Send(reply)
    });
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];

    client.read_db(1, 0, &mut buf).unwrap();

    assert_eq!(buf.to_vec(), memory(1, 0, 4));
}

#[test]
fn peer_closing_mid_telegram_yields_connection_closed() {
    let plc = plc_with(|frame| {
        let reply = echo_read(frame);
        // TPKT header + half of the S7 part, then EOF: no "tail" is ever written
        // because the connection is closed by the handler thread returning.
        Reply::Split(reply[..12].to_vec(), Duration::from_millis(50), Vec::new())
    });
    // `Split` with an empty tail keeps the socket open, so emulate EOF with a
    // second server that closes right after the head.
    drop(plc);
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| Reply::Close);
    let mut client = fast_client(&plc);
    let mut buf = [0u8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::ConnectionClosed), "{err:?}");
    assert!(err.requires_reconnect());
    assert!(!client.is_connected());
}

#[test]
fn truncated_telegram_times_out_and_drops_the_connection() {
    let plc = plc_with(|frame| {
        let reply = echo_read(frame);
        Reply::SendRaw(reply[..12].to_vec()) // announces 25 bytes, ships 12, stays silent
    });
    let mut client = fast_client(&plc);
    let mut buf = [0u8; 4];

    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    assert!(matches!(err, S7Error::Timeout), "{err:?}");
    assert!(!client.is_connected());
}

// ---- writes -------------------------------------------------------------------------

#[test]
fn write_timeout_after_first_chunk_reports_acknowledged_bytes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = plc_with(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Send(write_reply(RES_SUCCESS))
        } else {
            Reply::Silent
        }
    });
    let mut client = fast_client(&plc);

    let err = client.write_db(1, 0, &[0u8; 300]).unwrap_err(); // 212 + 88 at PDU 240

    match &err {
        S7Error::WriteIncomplete {
            acknowledged,
            requested,
            source,
        } => {
            assert_eq!(*acknowledged, 212);
            assert_eq!(*requested, 300);
            assert!(matches!(**source, S7Error::Timeout), "{source:?}");
        }
        other => panic!("expected WriteIncomplete, got {other:?}"),
    }
    assert!(err.requires_reconnect());
    assert!(!client.is_connected());
}

#[test]
fn plc_rejection_after_acknowledged_chunks_reports_acknowledged_bytes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let plc = plc_with(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 2 {
            Reply::Send(write_reply(RES_NOT_FOUND))
        } else {
            Reply::Send(write_reply(RES_SUCCESS))
        }
    });
    let mut client = plc.connected_client();

    let err = client.write_db(1, 0, &[0u8; 1000]).unwrap_err();

    match &err {
        S7Error::WriteIncomplete {
            acknowledged,
            requested,
            source,
        } => {
            assert_eq!(*acknowledged, 424);
            assert_eq!(*requested, 1000);
            assert!(matches!(**source, S7Error::S7NotFound));
        }
        other => panic!("expected WriteIncomplete, got {other:?}"),
    }
    assert!(
        !err.requires_reconnect(),
        "a PLC rejection is not a transport failure"
    );
    assert!(client.is_connected());
}

#[test]
fn plc_rejection_of_the_first_chunk_is_a_plain_error() {
    let plc = plc_with(|_| Reply::Send(write_reply(RES_INVALID_ADDRESS)));
    let mut client = plc.connected_client();

    let err = client.write_db(1, 0, &[0u8; 1000]).unwrap_err();

    assert!(matches!(err, S7Error::S7InvalidAddress), "{err:?}");
    assert!(client.is_connected());
}

// ---- argument limits ----------------------------------------------------------------

#[test]
fn bit_wordlen_with_empty_buffer_is_rejected_before_any_io() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();
    let mut empty: [u8; 0] = [];

    assert!(matches!(
        client.read_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &mut empty),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.write_area(S7_AREA_DB, 1, 0, S7_WL_BIT, &[]),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(plc.requests().is_empty());
    assert!(client.is_connected());
}

#[test]
fn buffers_longer_than_65535_bytes_are_rejected() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();
    let mut big = vec![0u8; 65536];

    assert!(matches!(
        client.read_db(1, 0, &mut big),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.write_db(1, 0, &big),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(plc.requests().is_empty());

    big.truncate(65535);
    client.read_db(1, 0, &mut big).unwrap();
    assert_eq!(big, memory(1, 0, 65535));
}

#[test]
fn bit_address_that_overflows_u16_is_rejected_in_every_build_profile() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();

    assert!(matches!(
        client.write_bit(S7_AREA_DB, 1, 8192, 3, true),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.read_bit(S7_AREA_DB, 1, u16::MAX, 7),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(plc.requests().is_empty(), "nothing may reach the wire");

    client.write_bit(S7_AREA_DB, 1, 8191, 7, true).unwrap();
    assert_eq!(parse_request(&plc.requests()[0]).address, 8191 * 8 + 7);
}

#[test]
fn rack_and_slot_out_of_range_are_rejected() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.client();

    assert!(matches!(
        client.connect_rack_slot(&plc.ip(), 0, 32),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        client.connect_rack_slot(&plc.ip(), 8, 0),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(!client.is_connected());
    assert!(plc.connection_requests().is_empty());

    client.connect_rack_slot(&plc.ip(), 7, 31).unwrap();
    assert_eq!(remote_tsap(&plc.connection_requests()[0]), 0x01FF);
}

#[test]
fn negotiated_pdu_above_the_requested_480_is_rejected() {
    for pdu in [481u16, 960, u16::MAX] {
        let plc = FakePlc::start(Handshake::Accept { pdu_length: pdu }, echo_memory_handler);
        let mut client = plc.client();
        let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();
        assert!(
            matches!(err, S7Error::PduNegotiationFailed),
            "pdu {pdu}: {err:?}"
        );
        assert!(!client.is_connected());
        assert_eq!(client.pdu_length(), 0);
    }
}

#[test]
fn full_size_chunks_at_the_maximum_pdu_round_trip() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 480 }, echo_memory_handler);
    let mut client = plc.connected_client();
    let mut buf = vec![0u8; max_read_chunk(480)];

    client.read_db(1, 0, &mut buf).unwrap();
    assert_eq!(client.chunks(), 1);
    assert_eq!(buf, memory(1, 0, buf.len()));

    let data = vec![0xA5u8; max_write_chunk(480)];
    client.write_db(1, 0, &data).unwrap();
    assert_eq!(client.chunks(), 1);
}

// ---- lifecycle --------------------------------------------------------------------------

#[test]
fn byte_wordlen_with_empty_buffer_is_a_successful_noop() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();
    let mut empty: [u8; 0] = [];
    client
        .read_area(S7_AREA_DB, 1, 0, S7_WL_BYTE, &mut empty)
        .unwrap();
    client
        .write_area(S7_AREA_DB, 1, 0, S7_WL_BYTE, &[])
        .unwrap();
    assert!(plc.requests().is_empty());
}

#[test]
fn reconnect_while_connected_closes_the_previous_socket() {
    let plc = plc_with(echo_memory_handler);
    let mut client = plc.connected_client();

    client.connect_s71200_1500(&plc.ip()).unwrap();

    assert!(
        plc.wait_for_closed(1, Duration::from_secs(2)),
        "old socket must be shut down"
    );
    assert_eq!(plc.connection_requests().len(), 2);
    let mut buf = [0u8; 2];
    client.read_db(1, 0, &mut buf).unwrap();
}

#[test]
fn failed_reconnect_closes_the_previous_socket() {
    let good = plc_with(echo_memory_handler);
    let bad = FakePlc::start(Handshake::RejectIso, echo_memory_handler);
    let mut client = good.connected_client();

    client.set_connection_port(bad.port()).unwrap();
    assert!(client.connect_s71200_1500(&bad.ip()).is_err());

    assert!(!client.is_connected());
    assert!(
        good.wait_for_closed(1, Duration::from_secs(2)),
        "old socket must not leak until Drop"
    );
}

#[test]
fn error_classification_is_stable() {
    assert!(S7Error::Timeout.requires_reconnect());
    assert!(S7Error::ConnectionClosed.requires_reconnect());
    assert!(S7Error::IsoInvalidTelegram.requires_reconnect());
    assert!(S7Error::S7InvalidResponse.requires_reconnect());
    assert!(!S7Error::NotConnected.requires_reconnect());
    assert!(!S7Error::InvalidFunParameter.requires_reconnect());
    assert!(!S7Error::S7NotFound.requires_reconnect());
    assert!(!S7Error::S7InvalidAddress.requires_reconnect());
    assert!(!S7Error::S7Unspecified.requires_reconnect());

    assert!(S7Error::S7NotFound.is_plc_error());
    assert!(!S7Error::Timeout.is_plc_error());
}
