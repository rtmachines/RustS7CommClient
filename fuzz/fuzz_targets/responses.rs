//! Validation of a read or write ACK_DATA PDU against the request it answers.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust7::client::fuzzing;
use rust7::kind;

const ALLOWED: [&str; 4] = [
    kind::ISO_INVALID_TELEGRAM,
    kind::S7_INVALID_ADDRESS,
    kind::S7_NOT_FOUND,
    kind::S7_UNSPECIFIED,
];

fuzz_target!(|data: &[u8]| {
    let Some((&[r0, r1, wordlen, n0, n1], pdu)) = data.split_first_chunk::<5>() else {
        return;
    };
    let reference = u16::from_be_bytes([r0, r1]);
    let n = usize::from(u16::from_be_bytes([n0, n1]));

    match fuzzing::validate_read_response(pdu, reference, wordlen, n) {
        Ok(payload) => {
            assert_eq!(payload, &pdu[pdu.len() - n..]);
            assert_eq!(&pdu[4..6], &reference.to_be_bytes());
        }
        Err(e) => assert!(e.get_type().is_some_and(|k| ALLOWED.contains(&k))),
    }
    match fuzzing::validate_write_response(pdu, reference) {
        Ok(()) => assert_eq!(pdu.last(), Some(&0xff)),
        Err(e) => assert!(e.get_type().is_some_and(|k| ALLOWED.contains(&k))),
    }
});
