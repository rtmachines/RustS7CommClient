//! TPKT, COTP and Setup Communication parsing of a whole received frame.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rust7::client::fuzzing;
use rust7::kind;

fuzz_target!(|data: &[u8]| {
    let Some((&size, frame)) = data.split_first() else {
        return;
    };
    // Spread 0..=255 over the valid request range 29..=480.
    let requested = 29 + (u32::from(size) * 451 / 255) as u16;

    if let Some(header) = frame.first_chunk::<4>() {
        let max_total = usize::from(size) * 4;
        match fuzzing::tpkt_length(header, max_total) {
            Ok(len) => assert!((4..=max_total).contains(&len)),
            Err(e) => assert!(matches!(
                e.get_type(),
                Some(kind::ISO_INVALID_HEADER | kind::ISO_INVALID_TELEGRAM)
            )),
        }
    }
    match fuzzing::data_payload(frame) {
        Ok(pdu) => {
            assert_eq!(
                frame.len(),
                usize::from(u16::from_be_bytes([frame[2], frame[3]]))
            );
            assert_eq!(pdu, &frame[7..]);
        }
        Err(e) => assert!(matches!(
            e.get_type(),
            Some(kind::ISO_INVALID_HEADER | kind::ISO_FRAGMENTED_PACKET)
        )),
    }
    match fuzzing::validate_connection_confirm(frame) {
        Ok(limit) => assert!(limit.is_power_of_two() && (128..=1024).contains(&limit)),
        Err(e) => assert_eq!(e.get_type(), Some(kind::ISO_CONNECTION_FAILED)),
    }
    match fuzzing::accept_negotiation(frame, requested) {
        Ok(pdu) => assert!((29..=requested).contains(&pdu)),
        Err(e) => assert_eq!(e.get_type(), Some(kind::PDU_NEGOTIATION_FAILED)),
    }
});
