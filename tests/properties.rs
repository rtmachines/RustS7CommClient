//! Property-based tests that drive the real client against the fake PLC.
//!
//! They use `TestRunner` directly (instead of the `proptest!` macro) so that
//! one TCP connection can be shared by all cases of a property.

mod common;

use std::cell::RefCell;

use common::*;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use rust7::{S7Error, CT_OP, CT_PG, CT_S7, S7_AREA_DB, S7_AREA_MK, S7_AREA_PA, S7_AREA_PE};

fn runner(cases: u32) -> TestRunner {
    TestRunner::new(Config { cases, ..Config::default() })
}

fn ok<T>(result: Result<T, S7Error>) -> Result<T, TestCaseError> {
    result.map_err(|e| TestCaseError::fail(e.to_string()))
}

fn area() -> impl Strategy<Value = u8> {
    prop::sample::select(vec![S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB])
}

#[test]
fn read_returns_exact_plc_memory_for_any_db_start_and_length() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, echo_memory_handler);
    let client = RefCell::new(plc.connected_client());
    let max_chunk = max_read_chunk(240);

    runner(64)
        .run(&(any::<u16>(), any::<u16>(), 0usize..=2000), |(db, start, len)| {
            let mut client = client.borrow_mut();
            let mut buf = vec![0u8; len];

            ok(client.read_db(db, start, &mut buf))?;

            let expected: Vec<u8> = (0..len as u32).map(|i| memory_byte(db, start as u32 + i)).collect();
            prop_assert_eq!(&buf, &expected);
            prop_assert_eq!(client.chunks, len.div_ceil(max_chunk));

            let reqs = plc.take_requests();
            prop_assert_eq!(reqs.len(), client.chunks);
            let mut expected_addr = (start as u32) << 3;
            for frame in &reqs {
                let req = parse_request(frame);
                prop_assert_eq!(req.function, FN_READ_VAR);
                prop_assert_eq!(req.db_number, db);
                prop_assert_eq!(req.area, S7_AREA_DB);
                prop_assert_eq!(req.address, expected_addr);
                prop_assert!(req.count as usize <= max_chunk);
                expected_addr += (req.count as u32) << 3;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn write_chunks_reassemble_to_the_original_buffer_with_contiguous_addresses() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, echo_memory_handler);
    let client = RefCell::new(plc.connected_client());
    let max_chunk = max_write_chunk(240);

    runner(64)
        .run(
            &(any::<u16>(), any::<u16>(), prop::collection::vec(any::<u8>(), 0..=1500)),
            |(db, start, data)| {
                let mut client = client.borrow_mut();

                ok(client.write_db(db, start, &data))?;

                let reqs = plc.take_requests();
                prop_assert_eq!(reqs.len(), data.len().div_ceil(max_chunk));
                prop_assert_eq!(client.chunks, reqs.len());

                let mut reassembled = Vec::with_capacity(data.len());
                let mut expected_addr = (start as u32) << 3;
                for frame in &reqs {
                    let req = parse_request(frame);
                    prop_assert_eq!(req.function, FN_WRITE_VAR);
                    prop_assert_eq!(req.db_number, db);
                    prop_assert_eq!(req.area, S7_AREA_DB);
                    prop_assert_eq!(req.wordlen, 0x02);
                    prop_assert_eq!(req.address, expected_addr);
                    prop_assert!(!req.payload.is_empty() && req.payload.len() <= max_chunk);
                    prop_assert_eq!(req.count as usize, req.payload.len());
                    prop_assert_eq!(req.telegram_len as usize, frame.len());
                    prop_assert_eq!(req.data_len as usize, req.payload.len() + 4);
                    prop_assert_eq!(req.transport, Some(TS_RES_BYTE));
                    prop_assert_eq!(req.bit_count, Some((req.payload.len() * 8) as u16));
                    expected_addr += (req.payload.len() as u32) << 3;
                    reassembled.extend_from_slice(&req.payload);
                }
                prop_assert_eq!(reassembled, data);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn bit_access_encodes_byte_times_8_plus_bit_for_all_valid_indices() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, echo_memory_handler);
    let client = RefCell::new(plc.connected_client());

    runner(64)
        .run(
            &(0u16..8192, 0u8..8, any::<u16>(), area(), any::<bool>()),
            |(byte_num, bit, db, area, value)| {
                let mut client = client.borrow_mut();
                let expected_addr = byte_num as u32 * 8 + bit as u32;

                ok(client.write_bit(area, db, byte_num, bit, value))?;
                let reqs = plc.take_requests();
                prop_assert_eq!(reqs.len(), 1);
                let req = parse_request(&reqs[0]);
                prop_assert_eq!(req.function, FN_WRITE_VAR);
                prop_assert_eq!(req.wordlen, 0x01);
                prop_assert_eq!(req.count, 1);
                prop_assert_eq!(req.area, area);
                prop_assert_eq!(req.db_number, db);
                prop_assert_eq!(req.address, expected_addr);
                prop_assert_eq!(req.transport, Some(TS_RES_BIT));
                prop_assert_eq!(req.bit_count, Some(1));
                prop_assert_eq!(&req.payload, &[value as u8]);

                let read = ok(client.read_bit(area, db, byte_num, bit))?;
                let reqs = plc.take_requests();
                prop_assert_eq!(reqs.len(), 1);
                let req = parse_request(&reqs[0]);
                prop_assert_eq!(req.function, FN_READ_VAR);
                prop_assert_eq!(req.wordlen, 0x01);
                prop_assert_eq!(req.count, 1);
                prop_assert_eq!(req.address, expected_addr);
                prop_assert_eq!(read, (memory_byte(db, byte_num as u32) >> bit) & 1 == 1);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn remote_tsap_encodes_connection_type_rack_and_slot() {
    let plc = FakePlc::start(Handshake::Accept { pdu_length: 240 }, echo_memory_handler);

    runner(24)
        .run(
            &(prop::sample::select(vec![CT_PG, CT_OP, CT_S7]), 0u16..8, 0u16..32),
            |(ct, rack, slot)| {
                let mut client = plc.client();
                ok(client.set_connection_type(ct))?;
                ok(client.connect_rack_slot(&plc.ip(), rack, slot))?;

                let cr = plc.take_connection_requests().pop().expect("connection request captured");
                prop_assert_eq!(local_tsap(&cr), 0x0100);
                prop_assert_eq!(remote_tsap(&cr), (ct << 8) + rack * 0x20 + slot);
                prop_assert!(client.connected);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn chunk_count_follows_negotiated_pdu_for_reads_and_writes() {
    // 480 is the PDU the client asks for; larger negotiated values are a known
    // defect (see `read_with_pdu_larger_than_requested_480` in read_write.rs).
    runner(16)
        .run(&(64u16..=480, 0usize..=3000), |(pdu, len)| {
            let plc = FakePlc::start(Handshake::Accept { pdu_length: pdu }, echo_memory_handler);
            let mut client = plc.connected_client();
            prop_assert_eq!(client.pdu_length, pdu);

            let mut buf = vec![0u8; len];
            ok(client.read_db(1, 0, &mut buf))?;
            prop_assert_eq!(client.chunks, len.div_ceil(max_read_chunk(pdu)));
            for frame in plc.take_requests() {
                prop_assert!(parse_request(&frame).count as usize <= max_read_chunk(pdu));
            }

            ok(client.write_db(1, 0, &buf))?;
            prop_assert_eq!(client.chunks, len.div_ceil(max_write_chunk(pdu)));
            for frame in plc.take_requests() {
                prop_assert!(parse_request(&frame).payload.len() <= max_write_chunk(pdu));
            }
            Ok(())
        })
        .unwrap();
}
