use super::*;
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Scripted {
    reads: VecDeque<io::Result<Vec<u8>>>,
    writes: Vec<u8>,
    delay: Duration,
    timeouts: Vec<Duration>,
}

impl Scripted {
    fn pieces(pieces: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            reads: pieces.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::thread::sleep(self.delay);
        match self.reads.pop_front() {
            Some(Ok(mut bytes)) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() {
                    self.reads.push_front(Ok(bytes.split_off(n)));
                }
                Ok(n)
            }
            Some(Err(e)) => Err(e),
            None => Ok(0),
        }
    }
}

impl FrameRead for Scripted {
    fn limit_next_read(&mut self, timeout: Duration) -> io::Result<()> {
        self.timeouts.push(timeout);
        Ok(())
    }
}

impl<T: AsRef<[u8]>> FrameRead for Cursor<T> {
    fn limit_next_read(&mut self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }
}

impl Write for Scripted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn tpkt(body: &[u8]) -> Vec<u8> {
    let n = u16::try_from(body.len() + 4).unwrap();
    let mut frame = vec![3, 0, (n >> 8) as u8, n as u8];
    frame.extend_from_slice(body);
    frame
}

fn data_frame(pdu: &[u8]) -> Vec<u8> {
    let mut body = vec![2, 0xf0, 0x80];
    body.extend_from_slice(pdu);
    tpkt(&body)
}

#[test]
fn read_tpkt_accepts_every_split() {
    for len in [22, 27, 26] {
        let frame = tpkt(&vec![0x42; len - 4]);
        for split in 1..len {
            let mut io = Scripted::pieces([frame[..split].to_vec(), frame[split..].to_vec()]);
            assert_eq!(
                read_tpkt(&mut io, 1028, Duration::from_secs(1)).unwrap(),
                frame
            );
        }
        let mut io = Scripted::pieces(frame.iter().map(|b| vec![*b]));
        io.reads.push_front(Err(io::ErrorKind::Interrupted.into()));
        assert_eq!(
            read_tpkt(&mut io, 1028, Duration::from_secs(1)).unwrap(),
            frame
        );
    }
}

#[test]
fn read_tpkt_leaves_next_frame() {
    let a = data_frame(&[0x32; 19]);
    let b = data_frame(&[0x32; 20]);
    let mut io = Cursor::new([a.as_slice(), b.as_slice()].concat());
    assert_eq!(read_tpkt(&mut io, 487, Duration::from_secs(1)).unwrap(), a);
    assert_eq!(read_tpkt(&mut io, 487, Duration::from_secs(1)).unwrap(), b);
}

#[test]
fn read_tpkt_rejects_invalid_lengths() {
    for len in 0..4 {
        assert!(read_tpkt(
            &mut Cursor::new([3, 0, 0, len]),
            487,
            Duration::from_secs(1)
        )
        .is_err());
    }
    for header in [[2, 0, 0, 10], [3, 1, 0, 10], [3, 0, 2, 0]] {
        let mut io = Cursor::new(header);
        assert!(read_tpkt(&mut io, 487, Duration::from_secs(1)).is_err());
        assert_eq!(io.position(), 4);
    }
}

#[test]
fn read_tpkt_propagates_eof_and_timeout() {
    for kind in [
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::TimedOut,
        io::ErrorKind::WouldBlock,
    ] {
        let mut io = Scripted::pieces([vec![3, 0, 0, 22], vec![0; 3]]);
        if kind != io::ErrorKind::UnexpectedEof {
            io.reads.push_back(Err(kind.into()));
        }
        let result = read_tpkt(&mut io, 487, Duration::from_secs(1));
        if kind == io::ErrorKind::UnexpectedEof {
            assert!(matches!(result, Err(S7Error::ConnectionClosed)));
        } else {
            assert!(matches!(result, Err(S7Error::Io(e)) if e.kind() == kind));
        }
    }
}

#[test]
fn cotp_headers_are_validated() {
    let frame = data_frame(&[0x32]);
    assert_eq!(data_payload(&frame).unwrap(), &[0x32]);
    for offset in [4, 5, 6] {
        let mut bad = frame.clone();
        bad[offset] = 0;
        assert!(data_payload(&bad).is_err());
    }
    for len in 0..8 {
        assert!(data_payload(&frame[..len]).is_err());
    }
    let cc = tpkt(&[6, 0xd0, 0, 1, 0, 1, 0]);
    validate_connection_confirm(&cc).unwrap();
    let cc_with_options = tpkt(&[9, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10]);
    validate_connection_confirm(&cc_with_options).unwrap();
    let bad_tlv = tpkt(&[9, 0xd0, 0, 1, 0, 1, 0, 0xc0, 2, 10]);
    assert!(validate_connection_confirm(&bad_tlv).is_err());
}

fn negotiation_response(length: u16) -> Vec<u8> {
    data_frame(&[
        0x32,
        3,
        0,
        0,
        4,
        0,
        0,
        8,
        0,
        0,
        0,
        0,
        0xf0,
        0,
        0,
        1,
        0,
        1,
        (length >> 8) as u8,
        length as u8,
    ])
}

#[test]
fn pdu_limits_reject_invalid_sizes() {
    for n in [0, 1, 18, 27, 28, 481, 65535] {
        assert!(matches!(
            PduLimits::new(n),
            Err(S7Error::PduNegotiationFailed)
        ));
    }
    for (n, read, write) in [(29, 11, 1), (240, 222, 212), (480, 462, 452)] {
        let limits = PduLimits::new(n).unwrap();
        assert_eq!(
            (limits.pdu_length, limits.read_payload, limits.write_payload),
            (n, read, write)
        );
    }
}

#[test]
fn negotiation_rejects_bad_header_or_error() {
    let good = negotiation_response(480);
    for index in [7, 8, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24] {
        let mut bad = good.clone();
        bad[index] ^= 1;
        let mut io = Scripted::pieces([bad]);
        assert!(
            matches!(
                negotiate(&mut io, 1024, Duration::from_secs(1)),
                Err(S7Error::PduNegotiationFailed)
            ),
            "index {index}"
        );
    }
    let mut io = Scripted::pieces(good.iter().map(|b| vec![*b]));
    assert_eq!(
        negotiate(&mut io, 1024, Duration::from_secs(1))
            .unwrap()
            .pdu_length,
        480
    );
    assert_eq!(io.writes.len(), 25);
    assert_eq!(&io.writes[23..25], &[1, 0xe0]);
}

#[test]
fn cotp_limit_caps_setup_offer() {
    for (options, requested) in [
        (vec![0xc0, 1, 7], 125u16),
        (vec![0xc0, 1, 8], 253),
        (vec![0xc0, 1, 9], 480),
        (vec![0xc0, 1, 10], 480),
        (vec![], 480),
    ] {
        let tpdu_limit = validate_connection_confirm(&connection_confirm(&options)).unwrap();
        for accepted in [29, requested - 1, requested] {
            let mut io = Scripted::pieces([negotiation_response(accepted)]);
            let limits = negotiate(&mut io, tpdu_limit, Duration::from_secs(1)).unwrap();
            assert_eq!(limits.pdu_length, accepted);
            assert_eq!(&io.writes[23..25], &requested.to_be_bytes());
        }
    }
}

#[test]
fn negotiation_rejects_size_outside_the_offer() {
    for (tpdu_limit, requested) in [(128, 125), (256, 253), (512, 480), (1024, 480)] {
        for accepted in [0, 28, requested + 1, 481, u16::MAX] {
            let mut io = Scripted::pieces([negotiation_response(accepted)]);
            assert!(
                matches!(
                    negotiate(&mut io, tpdu_limit, Duration::from_secs(1)),
                    Err(S7Error::PduNegotiationFailed)
                ),
                "TPDU {tpdu_limit}, S7 {accepted}"
            );
            assert_eq!(requests(&io).len(), 1);
        }
    }
}

#[test]
fn minimum_pdu_writes_one_byte_per_chunk() {
    let limits = PduLimits::new(29).unwrap();
    let mut io = Scripted::pieces((1..=3).map(|r| data_frame(&ack(r, 5, &[0xff]))));
    let (mut reference, mut chunks) = (0, 0);
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(10, false),
        &[7, 8, 9],
        &mut chunks,
        Duration::from_secs(1),
    )
    .unwrap();
    let packets = requests(&io);
    assert_eq!(chunks, 3);
    assert_eq!(packets.len(), 3);
    for (i, p) in packets.iter().enumerate() {
        assert_eq!(&p[23..25], &[0, 1]);
        assert_eq!(&p[28..31], &((10 + i as u32) * 8).to_be_bytes()[1..]);
        assert_eq!(p[35], 7 + i as u8);
    }
}

#[test]
fn connection_confirm_rejects_each_invalid_field() {
    let good = tpkt(&[6, 0xd0, 0, 1, 0x12, 0x34, 0]);
    validate_connection_confirm(&good).unwrap(); // Any source reference is accepted.
    for (index, value) in [(4, 7), (5, 0xe0), (5, 0xd1), (7, 2), (10, 0x10)] {
        let mut bad = good.clone();
        bad[index] = value;
        assert!(
            matches!(
                validate_connection_confirm(&bad),
                Err(S7Error::IsoConnectionFailed)
            ),
            "index {index}"
        );
    }
    assert!(validate_connection_confirm(&tpkt(&[7, 0xd0, 0, 1, 0, 1, 0, 0xc0])).is_err());
}

fn connection_confirm(options: &[u8]) -> Vec<u8> {
    let mut body = vec![6 + options.len() as u8, 0xd0, 0, 1, 0, 1, 0];
    body.extend_from_slice(options);
    tpkt(&body)
}

#[test]
fn connection_confirm_returns_effective_tpdu_limit() {
    assert_eq!(
        validate_connection_confirm(&connection_confirm(&[])).unwrap(),
        1024
    );
    assert_eq!(
        validate_connection_confirm(&connection_confirm(&[0xc1, 2, 1, 0])).unwrap(),
        1024
    );
    for (exponent, expected) in [(7, 128), (8, 256), (9, 512), (10, 1024)] {
        for options in [
            vec![0xc0, 1, exponent],
            vec![0xc1, 2, 1, 0, 0xc0, 1, exponent, 0xc2, 2, 1, 2],
        ] {
            assert_eq!(
                validate_connection_confirm(&connection_confirm(&options)).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn connection_confirm_rejects_invalid_tpdu_options() {
    for options in [
        vec![0xc0, 0],
        vec![0xc0, 2, 7, 0],
        vec![0xc0, 1, 0],
        vec![0xc0, 1, 6],
        vec![0xc0, 1, 11],
        vec![0xc0, 1, 255],
        vec![0xc0, 1, 7, 0xc0, 1, 7],
        vec![0xc0, 1, 7, 0xc0, 1, 10],
        vec![0xc0],
        vec![0xc0, 1],
        vec![0xc1, 2, 0],
    ] {
        assert!(
            matches!(
                validate_connection_confirm(&connection_confirm(&options)),
                Err(S7Error::IsoConnectionFailed)
            ),
            "options {options:02x?}"
        );
    }
}

fn error_ack(reference: u16) -> Vec<u8> {
    let [hi, lo] = reference.to_be_bytes();
    vec![0x32, 2, 0, 0, hi, lo, 0, 0, 0, 0, 0x81, 4]
}

#[test]
fn terminal_error_ack_reports_plc_error() {
    for error in [[0x81, 4], [0x81, 0], [0, 4]] {
        let mut pdu = error_ack(0x1234);
        pdu[10..12].copy_from_slice(&error);
        assert!(matches!(
            validate_read_response(&pdu, 0x1234, 2, 4),
            Err(S7Error::S7Unspecified)
        ));
        assert!(matches!(
            validate_write_response(&pdu, 0x1234),
            Err(S7Error::S7Unspecified)
        ));
    }
}

#[test]
fn terminal_error_ack_stops_transfer_without_retry_or_copying_failed_chunk() {
    let limits = PduLimits::new(29).unwrap();
    for writing in [false, true] {
        let chunk_size = if writing { 1 } else { 11 };
        for completed in [0, 1] {
            let success = |reference, len| {
                data_frame(&if writing {
                    ack(reference, 5, &[0xff])
                } else {
                    read_ack(reference, &vec![42; len], false)
                })
            };
            let mut io = Scripted::default();
            if completed == 1 {
                io.reads.push_back(Ok(success(1, chunk_size)));
            }
            io.reads
                .push_back(Ok(data_frame(&error_ack(completed as u16 + 1))));
            // A response for the next operation must remain unread after the error.
            io.reads.push_back(Ok(success(completed as u16 + 2, 1)));
            let mut buffer = vec![0xaa; chunk_size * 3];
            let (mut reference, mut chunks) = (0, 0);
            let result = if writing {
                write_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(17, false),
                    &buffer,
                    &mut chunks,
                    Duration::from_secs(1),
                )
            } else {
                read_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(17, false),
                    &mut buffer,
                    &mut chunks,
                    Duration::from_secs(1),
                )
            };
            assert!(matches!(result, Err(S7Error::S7Unspecified)));
            assert_eq!(chunks, completed + 1);
            assert_eq!(usize::from(reference), chunks);
            assert_eq!(requests(&io).len(), chunks);
            assert_eq!(io.reads.len(), 1);
            let copied = if writing { 0 } else { completed * chunk_size };
            assert_eq!(&buffer[..copied], vec![42; copied]);
            assert_eq!(&buffer[copied..], vec![0xaa; buffer.len() - copied]);
            if writing {
                write_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(0, false),
                    &[1],
                    &mut chunks,
                    Duration::from_secs(1),
                )
                .unwrap();
            } else {
                let mut next = [0];
                read_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(0, false),
                    &mut next,
                    &mut chunks,
                    Duration::from_secs(1),
                )
                .unwrap();
                assert_eq!(next, [42]);
            }
            assert_eq!(requests(&io).len(), completed + 2);
            assert!(io.reads.is_empty());
        }
    }
}

#[test]
fn malformed_terminal_ack_is_not_a_plc_error_or_write_success() {
    let good = error_ack(0x1234);
    let mut invalid: Vec<Vec<u8>> = (0..12).map(|len| good[..len].to_vec()).collect();
    for index in [0, 2, 3, 4, 5, 6, 7, 8, 9] {
        let mut bad = good.clone();
        bad[index] ^= 1;
        invalid.push(bad);
    }
    for rosctr in [0, 1, 4, 7] {
        let mut bad = good.clone();
        bad[1] = rosctr;
        invalid.push(bad);
    }
    let mut empty_ack = good.clone();
    empty_ack[10..12].fill(0);
    invalid.push(empty_ack);
    for (field, extra) in [(7, vec![4, 1]), (9, vec![0xff])] {
        let mut bad = good.clone();
        bad[field] = extra.len() as u8;
        bad.extend(extra);
        invalid.push(bad);
    }
    let mut trailing = good.clone();
    trailing.push(0);
    invalid.push(trailing);
    for pdu in invalid {
        assert!(
            matches!(
                validate_read_response(&pdu, 0x1234, 2, 4),
                Err(S7Error::IsoInvalidTelegram)
            ),
            "read {pdu:02x?}"
        );
        assert!(
            matches!(
                validate_write_response(&pdu, 0x1234),
                Err(S7Error::IsoInvalidTelegram)
            ),
            "write {pdu:02x?}"
        );
    }
}

fn ack(reference: u16, function: u8, data: &[u8]) -> Vec<u8> {
    let mut pdu = vec![
        0x32,
        3,
        0,
        0,
        (reference >> 8) as u8,
        reference as u8,
        0,
        2,
        (data.len() >> 8) as u8,
        data.len() as u8,
        0,
        0,
        function,
        1,
    ];
    pdu.extend_from_slice(data);
    pdu
}

fn read_ack(reference: u16, payload: &[u8], bit: bool) -> Vec<u8> {
    let bits = if bit { 1 } else { payload.len() * 8 };
    let mut data = vec![0xff, if bit { 3 } else { 4 }, (bits >> 8) as u8, bits as u8];
    data.extend_from_slice(payload);
    ack(reference, 4, &data)
}

#[test]
fn read_response_requires_exact_payload() {
    for len in [1, 4, 5] {
        let pdu = read_ack(0x1234, &vec![42; len], false);
        let result = validate_read_response(&pdu, 0x1234, 2, 4);
        if len == 4 {
            assert_eq!(result.unwrap(), &[42; 4]);
        } else {
            assert!(result.is_err());
        }
    }
    let bit = read_ack(1, &[1], true);
    assert_eq!(validate_read_response(&bit, 1, 1, 1).unwrap(), &[1]);
    assert!(validate_read_response(&bit, 1, 2, 1).is_err());
    let mut bad = read_ack(1, &[42; 4], false);
    bad[17] = 8;
    assert!(validate_read_response(&bad, 1, 2, 4).is_err());
}

#[test]
fn response_rejects_wrong_header() {
    let read = read_ack(0x1234, &[1, 2, 3, 4], false);
    for index in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 16, 17] {
        let mut bad = read.clone();
        bad[index] ^= 1;
        assert!(
            validate_read_response(&bad, 0x1234, 2, 4).is_err(),
            "index {index}"
        );
    }
    let write = ack(0x1234, 5, &[0xff]);
    validate_write_response(&write, 0x1234).unwrap();
    for index in 0..14 {
        let mut bad = write.clone();
        bad[index] ^= 1;
        assert!(
            validate_write_response(&bad, 0x1234).is_err(),
            "index {index}"
        );
    }
    let mut zero = vec![0; 15];
    zero[14] = 0xff;
    assert!(validate_write_response(&zero, 0x1234).is_err());
    for len in 0..read.len() {
        assert!(validate_read_response(&read[..len], 0x1234, 2, 4).is_err());
    }
    for len in 0..write.len() {
        assert!(validate_write_response(&write[..len], 0x1234).is_err());
    }
}

#[test]
fn response_preserves_plc_errors() {
    for (status, expected) in [(5, "address"), (10, "missing"), (7, "other")] {
        let read = ack(1, 4, &[status, 0, 0, 0]);
        let write = ack(1, 5, &[status]);
        for err in [
            validate_read_response(&read, 1, 2, 4).unwrap_err(),
            validate_write_response(&write, 1).unwrap_err(),
        ] {
            assert!(matches!(
                (expected, err),
                ("address", S7Error::S7InvalidAddress)
                    | ("missing", S7Error::S7NotFound)
                    | ("other", S7Error::S7Unspecified)
            ));
        }
    }
}

#[test]
fn response_rejects_previous_reference_and_counter_wraps() {
    let mut counter = 65534;
    assert_eq!(next_reference(&mut counter), 65535);
    assert_eq!(next_reference(&mut counter), 0);
    assert_eq!(next_reference(&mut counter), 1);
    assert!(validate_write_response(&ack(0, 5, &[0xff]), 1).is_err());
    assert!(validate_read_response(&read_ack(0, &[1], false), 1, 2, 1).is_err());
}

fn transfer(start: u32, bit: bool) -> Transfer {
    Transfer {
        area: 0x84,
        db_number: 100,
        start,
        wordlen: if bit { 1 } else { 2 },
    }
}

fn requests(io: &Scripted) -> Vec<Vec<u8>> {
    let mut cursor = Cursor::new(&io.writes);
    let mut result = Vec::new();
    while cursor.position() < io.writes.len() as u64 {
        result.push(read_tpkt(&mut cursor, 487, Duration::from_secs(1)).unwrap());
    }
    result
}

#[test]
fn bit_address_does_not_wrap() {
    let limits = PduLimits::new(480).unwrap();
    for byte in [8191, 8192, 65535] {
        for bit in [0, 7] {
            let start = bit_start(byte, bit).unwrap();
            assert_eq!(start, u32::from(byte) * 8 + u32::from(bit));
            let mut io = Scripted::pieces([
                data_frame(&read_ack(1, &[1], true)),
                data_frame(&ack(2, 5, &[0xff])),
            ]);
            let (mut reference, mut chunks) = (0, 0);
            let mut buffer = [0; 4];
            read_transfer(
                &mut io,
                &limits,
                &mut reference,
                transfer(start, true),
                &mut buffer,
                &mut chunks,
                Duration::from_secs(1),
            )
            .unwrap();
            assert_eq!(buffer, [1, 0, 0, 0]);
            write_transfer(
                &mut io,
                &limits,
                &mut reference,
                transfer(start, true),
                &[1, 99],
                &mut chunks,
                Duration::from_secs(1),
            )
            .unwrap();
            let packets = requests(&io);
            for packet in &packets {
                assert_eq!(&packet[28..31], &start.to_be_bytes()[1..]);
                assert_eq!(&packet[23..25], &[0, 1]);
            }
            assert_eq!(packets[1].len(), 36);
        }
    }
    assert!(matches!(bit_start(1, 8), Err(S7Error::InvalidFunParameter)));
}

#[test]
fn large_transfer_is_complete() {
    for (tpdu, pdu, read_limit, write_limit) in [
        (128, 125, 107, 97),
        (256, 253, 235, 225),
        (512, 480, 462, 452),
        (1024, 480, 462, 452),
        (1024, 240, 222, 212),
    ] {
        let mut setup = Scripted::pieces([negotiation_response(pdu)]);
        let limits = negotiate(&mut setup, tpdu, Duration::from_secs(1)).unwrap();
        for writing in [false, true] {
            let limit = if writing { write_limit } else { read_limit };
            for size in [0, 1, limit - 1, limit, limit + 1, 65535, 65536, 100000] {
                let expected: Vec<u8> = (0..size)
                    .map(|i| ((i * 17 + i / 251) % 256) as u8)
                    .collect();
                let mut io = Scripted::default();
                for (index, data) in expected.chunks(limit).enumerate() {
                    let pdu = if writing {
                        ack((index + 1) as u16, 5, &[0xff])
                    } else {
                        read_ack((index + 1) as u16, data, false)
                    };
                    let response = data_frame(&pdu);
                    assert!(response.len() - 4 <= usize::from(tpdu));
                    io.reads.push_back(Ok(response));
                }
                let (mut reference, mut chunks) = (0, 0);
                if writing {
                    write_transfer(
                        &mut io,
                        &limits,
                        &mut reference,
                        transfer(17, false),
                        &expected,
                        &mut chunks,
                        Duration::from_secs(1),
                    )
                    .unwrap();
                } else {
                    let mut buffer = vec![0xaa; size];
                    read_transfer(
                        &mut io,
                        &limits,
                        &mut reference,
                        transfer(17, false),
                        &mut buffer,
                        &mut chunks,
                        Duration::from_secs(1),
                    )
                    .unwrap();
                    assert_eq!(buffer, expected);
                }
                let packets = requests(&io);
                assert_eq!(packets.len(), expected.chunks(limit).len());
                assert_eq!(chunks, packets.len());
                let mut offset = 0;
                for (index, packet) in packets.iter().enumerate() {
                    assert!(packet.len() - 4 <= usize::from(tpdu));
                    let n = usize::from(u16::from_be_bytes([packet[23], packet[24]]));
                    assert!(n > 0 && n <= limit);
                    assert_eq!(n, (size - offset).min(limit));
                    assert_eq!(&packet[11..13], &((index + 1) as u16).to_be_bytes());
                    assert_eq!(
                        &packet[28..31],
                        &(((17 + offset) as u32) * 8).to_be_bytes()[1..]
                    );
                    assert_eq!(packet[22], 2);
                    assert_eq!(&packet[25..28], &[0, 100, 0x84]);
                    if writing {
                        assert_eq!(&packet[35..], &expected[offset..offset + n]);
                        assert_eq!(u16::from_be_bytes([packet[33], packet[34]]) as usize, n * 8);
                        assert_eq!(packet.len(), 35 + n);
                    } else {
                        assert_eq!(packet.len(), 31);
                        assert!(18 + n <= usize::from(limits.pdu_length));
                        assert!(3 + 18 + n <= usize::from(tpdu));
                    }
                    offset += n;
                }
                assert_eq!(offset, size);
            }
        }
    }
}

#[test]
fn empty_bit_buffer_rejected_before_io() {
    let mut io = Scripted::default();
    let limits = PduLimits::new(480).unwrap();
    let (mut reference, mut chunks) = (0, 0);
    assert!(matches!(
        read_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, true),
            &mut [],
            &mut chunks,
            Duration::from_secs(1)
        ),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(matches!(
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, true),
            &[],
            &mut chunks,
            Duration::from_secs(1)
        ),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(io.writes.is_empty());
    assert_eq!((reference, chunks), (0, 0));
}

#[test]
fn byte_empty_is_noop() {
    let mut io = Scripted::default();
    let limits = PduLimits::new(480).unwrap();
    let (mut reference, mut chunks) = (0, 0);
    read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &mut [],
        &mut chunks,
        Duration::from_secs(1),
    )
    .unwrap();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &[],
        &mut chunks,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(io.writes.is_empty());
    assert_eq!((reference, chunks), (0, 0));
}

#[test]
fn address_range_checked_before_io() {
    assert_eq!(validate_transfer(0x1fffff, 2, 1).unwrap(), 1);
    assert!(validate_transfer(0x1fffff, 2, 2).is_err());
    assert!(validate_transfer(1, 2, usize::MAX).is_err());
    assert!(validate_transfer(0x1000000, 1, 1).is_err());
    let mut io = Scripted::default();
    let limits = PduLimits::new(480).unwrap();
    let (mut reference, mut chunks) = (0, 0);
    let buffer = vec![0; 0x200000 - 65535 + 1];
    assert!(matches!(
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(65535, false),
            &buffer,
            &mut chunks,
            Duration::from_secs(1)
        ),
        Err(S7Error::InvalidFunParameter)
    ));
    assert!(io.writes.is_empty());
    let mut buffer = buffer;
    assert!(read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(65535, false),
        &mut buffer,
        &mut chunks,
        Duration::from_secs(1)
    )
    .is_err());
    assert!(io.writes.is_empty());
}

#[test]
fn second_write_chunk_failure_stops_without_retry() {
    let limits = PduLimits::new(240).unwrap();
    let mut io = Scripted::pieces([
        data_frame(&ack(1, 5, &[0xff])),
        data_frame(&ack(2, 5, &[5])),
    ]);
    let (mut reference, mut chunks) = (0, 0);
    assert!(matches!(
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, false),
            &vec![42; 600],
            &mut chunks,
            Duration::from_secs(1)
        ),
        Err(S7Error::S7InvalidAddress)
    ));
    let packets = requests(&io);
    assert_eq!(packets.len(), 2);
    assert_eq!(chunks, 2);
    assert_eq!(&packets[0][28..31], &[0, 0, 0]);
    assert_eq!(&packets[1][28..31], &[0, 6, 0xa0]);
}

#[test]
fn short_read_leaves_destination_unchanged() {
    let limits = PduLimits::new(480).unwrap();
    let mut io = Scripted::pieces([data_frame(&read_ack(1, &[1], false))]);
    let (mut reference, mut chunks) = (0, 0);
    let mut buffer = [42; 4];
    assert!(read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &mut buffer,
        &mut chunks,
        Duration::from_secs(1)
    )
    .is_err());
    assert_eq!(buffer, [42; 4]);
}

#[test]
fn remote_tsap_encodes_and_rejects_out_of_range() {
    for (ct, rack, slot, tsap) in [
        (1, 0, 0, 0x0100),
        (1, 0, 2, 0x0102),
        (2, 1, 0, 0x0220),
        (3, 7, 31, 0x03ff),
    ] {
        assert_eq!(remote_tsap(ct, rack, slot).unwrap(), tsap);
    }
    for (rack, slot) in [(8, 0), (0, 32), (2048, 0), (0, 65535), (65535, 65535)] {
        assert!(matches!(
            remote_tsap(1, rack, slot),
            Err(S7Error::InvalidFunParameter)
        ));
    }
}

#[test]
fn bit_write_payload_is_normalized() {
    let limits = PduLimits::new(480).unwrap();
    for (input, expected) in [
        (vec![0], 0),
        (vec![1], 1),
        (vec![2], 1),
        (vec![0x80], 1),
        (vec![0xff], 1),
        (vec![0, 1], 0),
    ] {
        let mut io = Scripted::pieces([data_frame(&ack(1, 5, &[0xff]))]);
        let (mut reference, mut chunks) = (0, 0);
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(3, true),
            &input,
            &mut chunks,
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            &requests(&io)[0][31..],
            &[0, 3, 0, 1, expected],
            "input {input:?}"
        );
    }
}

#[test]
fn frame_deadline_bounds_slow_peer() {
    let frame = data_frame(&[0x32; 20]);
    let mut io = Scripted::pieces(frame.iter().map(|b| vec![*b]));
    io.delay = Duration::from_millis(20);
    let started = Instant::now();
    let err = read_tpkt(&mut io, 487, Duration::from_millis(100)).unwrap_err();
    assert!(matches!(err, S7Error::Io(ref e) if e.kind() == io::ErrorKind::TimedOut));
    assert!(started.elapsed() < Duration::from_millis(400));
    assert!(!io.reads.is_empty());
    assert!(io.timeouts.iter().all(|t| *t <= Duration::from_millis(100)));
    assert!(io.timeouts.windows(2).all(|w| w[1] <= w[0]));
}

#[test]
fn huge_read_timeout_does_not_panic() {
    let frame = data_frame(&[0x32; 20]);
    let mut io = Scripted::pieces([frame.clone()]);
    assert_eq!(
        read_tpkt(&mut io, 487, Duration::from_millis(u64::MAX)).unwrap(),
        frame
    );
}
