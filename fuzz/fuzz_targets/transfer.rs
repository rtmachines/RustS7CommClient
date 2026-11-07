//! A whole read or write transfer, chunked by the negotiated PDU, against an
//! arbitrary stream of reply bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust7::client::fuzzing;
use rust7::{kind, S7_WL_BIT, S7_WL_BYTE};

fuzz_target!(|data: &[u8]| {
    let Some((&[p0, p1, l0, l1, flags], replies)) = data.split_first_chunk::<5>() else {
        return;
    };
    let pdu = 29 + u16::from_be_bytes([p0, p1]) % 452;
    let bit = flags & 1 != 0;
    let writing = flags & 2 != 0;
    let len = if bit {
        1
    } else {
        usize::from(u16::from_be_bytes([l0, l1])) % 2000
    };
    let wordlen = if bit { S7_WL_BIT } else { S7_WL_BYTE };
    let mut buffer = vec![0xaa; len];

    let outcome = fuzzing::transfer(pdu, 100, wordlen, writing, &mut buffer, replies).unwrap();
    // Each request is answered by at least one 4-byte TPKT header.
    assert!(outcome.chunks <= 1 + replies.len() / 4);
    assert!(outcome.confirmed <= len);
    match outcome.result {
        Ok(()) => assert_eq!(outcome.confirmed, len),
        Err(e) => assert!(e.get_type().is_some_and(|k| kind::ALL.contains(&k))),
    }
    if !writing && !bit {
        // Bytes past the confirmed prefix are never written.
        assert!(buffer[outcome.confirmed..].iter().all(|b| *b == 0xaa));
    }
});
