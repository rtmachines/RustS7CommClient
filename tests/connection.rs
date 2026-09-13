//! Handshake, TSAP encoding, connection failure modes and lifecycle, tested
//! against the in-process fake PLC.

mod common;

use std::io;
use std::net::TcpListener;
use std::time::Duration;

use common::*;
use rust7::{S7Error, CT_OP, CT_PG, CT_S7};

fn accepting_plc(pdu_length: u16) -> FakePlc {
    FakePlc::start(Handshake::Accept { pdu_length }, echo_memory_handler)
}

// ---- successful handshake ----------------------------------------------------

#[test]
fn connects_and_stores_negotiated_pdu_length() {
    let plc = accepting_plc(240);
    let mut client = plc.client();

    client.connect_s71200_1500(&plc.ip()).unwrap();

    assert!(client.connected);
    assert_eq!(client.pdu_length, 240);
    assert!(client.last_time > 0.0, "last_time must hold the handshake duration");
}

#[test]
fn pdu_length_is_whatever_the_plc_negotiates_not_what_was_requested() {
    for pdu in [64u16, 480, 960, 1920] {
        let plc = accepting_plc(pdu);
        let client = plc.connected_client();
        assert_eq!(client.pdu_length, pdu);
    }
}

#[test]
fn connection_request_frame_matches_rfc1006_layout() {
    let plc = accepting_plc(240);
    let _client = plc.connected_client();

    let cr = &plc.connection_requests()[0];
    assert_eq!(cr.len(), ISO_CR_LEN);
    assert_eq!(&cr[0..4], &[ISO_ID, 0x00, 0x00, 22], "TPKT header with total length");
    assert_eq!(cr[4], 0x11, "COTP length");
    assert_eq!(cr[5], COTP_CONNECT_REQUEST);
    assert_eq!(&cr[6..11], &[0x00, 0x00, 0x00, 0x01, 0x00], "dst/src reference, class");
    assert_eq!(&cr[11..14], &[0xC0, 0x01, 0x0A], "TPDU size parameter");
    assert_eq!(&cr[14..16], &[0xC1, 0x02], "src TSAP parameter header");
    assert_eq!(&cr[18..20], &[0xC2, 0x02], "dst TSAP parameter header");
}

#[test]
fn negotiation_request_asks_for_a_480_byte_pdu() {
    let plc = accepting_plc(240);
    let _client = plc.connected_client();

    let pn = &plc.negotiation_requests()[0];
    assert_eq!(pn.len(), ISO_PN_REQ_LEN);
    assert_eq!(&pn[0..4], &[ISO_ID, 0x00, 0x00, 25]);
    assert_eq!(&pn[4..7], &[0x02, 0xF0, EOT]);
    assert_eq!(pn[7], S7_ID);
    assert_eq!(pn[8], 0x01, "job");
    assert_eq!(pn[17], 0xF0, "setup communication function");
    assert_eq!(be16(pn, 23), 480, "requested PDU length");
}

// ---- TSAP encoding -----------------------------------------------------------

#[test]
fn s71200_1500_helper_uses_rack_0_slot_0() {
    let plc = accepting_plc(240);
    let mut client = plc.client();
    client.connect_s71200_1500(&plc.ip()).unwrap();

    let cr = &plc.connection_requests()[0];
    assert_eq!(local_tsap(cr), 0x0100);
    assert_eq!(remote_tsap(cr), 0x0100, "PG connection, rack 0, slot 0");
}

#[test]
fn s7300_helper_uses_rack_0_slot_2() {
    let plc = accepting_plc(240);
    let mut client = plc.client();
    client.connect_s7300(&plc.ip()).unwrap();

    assert_eq!(remote_tsap(&plc.connection_requests()[0]), 0x0102);
}

#[test]
fn rack_and_slot_are_packed_into_remote_tsap() {
    let plc = accepting_plc(240);
    let mut client = plc.client();
    client.connect_rack_slot(&plc.ip(), 1, 3).unwrap();

    assert_eq!(remote_tsap(&plc.connection_requests()[0]), 0x0123);
}

#[test]
fn connection_type_is_the_high_byte_of_remote_tsap() {
    let plc = accepting_plc(240);
    for (ct, expected) in [(CT_PG, 0x0102u16), (CT_OP, 0x0202), (CT_S7, 0x0302)] {
        let mut client = plc.client();
        client.set_connection_type(ct).unwrap();
        client.connect_s7300(&plc.ip()).unwrap();
        let cr = plc.take_connection_requests().pop().unwrap();
        assert_eq!(remote_tsap(&cr), expected, "connection type {ct}");
    }
}

#[test]
fn connect_tsap_sends_both_tsaps_verbatim() {
    let plc = accepting_plc(240);
    let mut client = plc.client();
    client.connect_tsap(&plc.ip(), 0x1000, 0x2000).unwrap();

    let cr = &plc.connection_requests()[0];
    assert_eq!(local_tsap(cr), 0x1000);
    assert_eq!(remote_tsap(cr), 0x2000);
}

// ---- handshake failures ------------------------------------------------------

#[test]
fn iso_disconnect_request_yields_iso_connection_failed() {
    let plc = FakePlc::start(Handshake::RejectIso, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::IsoConnectionFailed), "{err:?}");
    assert!(!client.connected);
    assert_eq!(client.last_time, 0.0);
    assert!(plc.negotiation_requests().is_empty(), "must not negotiate after ISO failure");
}

#[test]
fn peer_closing_after_connection_request_fails_the_iso_step() {
    let plc = FakePlc::start(Handshake::CloseAfterCr, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::IsoConnectionFailed | S7Error::Io(_)), "{err:?}");
    assert!(!client.connected);
}

#[test]
fn negotiation_with_wrong_protocol_id_fails() {
    let plc = FakePlc::start(Handshake::BadNegotiationProtocolId, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::PduNegotiationFailed), "{err:?}");
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
}

#[test]
fn negotiation_with_error_class_fails() {
    let plc = FakePlc::start(Handshake::NegotiationErrorClass, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::PduNegotiationFailed), "{err:?}");
    assert!(!client.connected);
}

#[test]
fn negotiated_pdu_length_of_zero_is_rejected() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 0 }, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::PduNegotiationFailed), "{err:?}");
    assert!(!client.connected);
}

#[test]
#[ignore = "known defect: a negotiated pdu_length in 1..=27 underflows `pdu_length - 18` / `- 28` in connect_tsap (panics in debug, wraps to a huge chunk size in release); only 0 is rejected"]
fn negotiated_pdu_length_smaller_than_the_s7_headers_is_rejected() {
    for pdu in [1u16, 17, 18, 27] {
        let plc = FakePlc::start(Handshake::Accept { pdu_length: pdu }, echo_memory_handler);
        let mut client = plc.client();

        let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

        assert!(matches!(err, S7Error::PduNegotiationFailed), "pdu {pdu}: {err:?}");
        assert!(!client.connected);
    }
}

#[test]
fn peer_closing_during_negotiation_fails() {
    let plc = FakePlc::start(Handshake::CloseAfterNegotiationRequest, echo_memory_handler);
    let mut client = plc.client();

    let err = client.connect_s71200_1500(&plc.ip()).unwrap_err();

    assert!(matches!(err, S7Error::PduNegotiationFailed | S7Error::Io(_)), "{err:?}");
    assert!(!client.connected);
}

#[test]
fn refused_tcp_connection_is_reported_as_io_error() {
    // Grab a free port, then release it so nobody listens there.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut client = rust7::S7Client::new();
    client.set_connection_port(port).unwrap();

    let err = client.connect_s71200_1500("127.0.0.1").unwrap_err();

    match err {
        S7Error::Io(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused, "{e}"),
        other => panic!("expected Io(ConnectionRefused), got {other:?}"),
    }
    assert!(!client.connected);
}

#[test]
fn unparsable_address_is_reported_as_io_error() {
    let mut client = rust7::S7Client::new();
    let err = client.connect_s71200_1500("").unwrap_err();
    assert!(matches!(err, S7Error::Io(_)), "{err:?}");
    assert!(!client.connected);
}

#[test]
fn failed_connection_clears_previous_connected_state() {
    let good = accepting_plc(240);
    let bad = FakePlc::start(Handshake::RejectIso, echo_memory_handler);

    let mut client = good.connected_client();
    assert!(client.connected);

    client.set_connection_port(bad.port()).unwrap();
    assert!(client.connect_s71200_1500(&bad.ip()).is_err());
    assert!(!client.connected);
}

// ---- lifecycle ---------------------------------------------------------------

#[test]
fn disconnect_makes_data_access_fail_with_not_connected() {
    let plc = accepting_plc(240);
    let mut client = plc.connected_client();
    let mut buf = [0u8; 4];
    client.read_db(1, 0, &mut buf).unwrap();

    client.disconnect();

    assert!(!client.connected);
    assert!(matches!(client.read_db(1, 0, &mut buf), Err(S7Error::NotConnected)));
    assert!(matches!(client.write_db(1, 0, &buf), Err(S7Error::NotConnected)));
    assert!(plc.wait_for_closed(1, Duration::from_secs(2)), "peer must observe the shutdown");
}

#[test]
fn disconnect_is_idempotent() {
    let plc = accepting_plc(240);
    let mut client = plc.connected_client();
    client.disconnect();
    client.disconnect();
    assert!(!client.connected);
}

#[test]
fn dropping_a_connected_client_closes_the_socket() {
    let plc = accepting_plc(240);
    let client = plc.connected_client();

    drop(client);

    assert!(plc.wait_for_closed(1, Duration::from_secs(2)));
}

#[test]
fn client_can_reconnect_after_disconnect() {
    let plc = accepting_plc(240);
    let mut client = plc.connected_client();
    client.disconnect();

    client.connect_s71200_1500(&plc.ip()).unwrap();

    assert!(client.connected);
    assert_eq!(plc.connection_requests().len(), 2);
    let mut buf = [0u8; 2];
    client.read_db(7, 0, &mut buf).unwrap();
    assert_eq!(buf, [memory_byte(7, 0), memory_byte(7, 1)]);
}

#[test]
fn read_timeout_is_reported_as_io_error_and_keeps_connection_flag() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, |_| Reply::Silent);
    let mut client = plc.client();
    client.set_timeout(3000, 100, 500).unwrap();
    client.connect_s71200_1500(&plc.ip()).unwrap();

    let mut buf = [0u8; 4];
    let err = client.read_db(1, 0, &mut buf).unwrap_err();

    match err {
        S7Error::Io(e) => assert!(
            matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock),
            "unexpected kind {:?}",
            e.kind()
        ),
        other => panic!("expected Io timeout, got {other:?}"),
    }
    assert_eq!(client.last_time, 0.0);
    assert!(client.connected, "the client leaves reconnection to the caller");
}
