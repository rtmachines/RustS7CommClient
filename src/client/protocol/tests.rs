use super::*;
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};

#[derive(Default)]
struct Scripted {
    reads: VecDeque<io::Result<Vec<u8>>>,
    writes: Vec<u8>,
}

impl Scripted {
    fn pieces(pieces: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            reads: pieces.into_iter().map(Ok).collect(),
            writes: Vec::new(),
        }
    }
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
            assert_eq!(read_tpkt(&mut io, 1028).unwrap(), frame);
        }
        let mut io = Scripted::pieces(frame.iter().map(|b| vec![*b]));
        io.reads.push_front(Err(io::ErrorKind::Interrupted.into()));
        assert_eq!(read_tpkt(&mut io, 1028).unwrap(), frame);
    }
}

#[test]
fn read_tpkt_leaves_next_frame() {
    let a = data_frame(&[0x32; 19]);
    let b = data_frame(&[0x32; 20]);
    let mut io = Cursor::new([a.as_slice(), b.as_slice()].concat());
    assert_eq!(read_tpkt(&mut io, 487).unwrap(), a);
    assert_eq!(read_tpkt(&mut io, 487).unwrap(), b);
}

#[test]
fn read_tpkt_rejects_invalid_lengths() {
    for len in 0..4 {
        assert!(read_tpkt(&mut Cursor::new([3, 0, 0, len]), 487).is_err());
    }
    for header in [[2, 0, 0, 10], [3, 1, 0, 10], [3, 0, 2, 0]] {
        let mut io = Cursor::new(header);
        assert!(read_tpkt(&mut io, 487).is_err());
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
        assert!(matches!(read_tpkt(&mut io, 487), Err(S7Error::Io(e)) if e.kind() == kind));
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
            matches!(negotiate(&mut io), Err(S7Error::PduNegotiationFailed)),
            "index {index}"
        );
    }
    let mut io = Scripted::pieces(good.iter().map(|b| vec![*b]));
    assert_eq!(negotiate(&mut io).unwrap().pdu_length, 480);
    assert_eq!(io.writes.len(), 25);
    assert_eq!(&io.writes[23..25], &[1, 0xe0]);
}

#[test]
fn write_chunk_is_never_zero() {
    let limits = PduLimits::new(29).unwrap();
    assert_eq!(1usize.min(limits.write_payload), 1);
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
        result.push(read_tpkt(&mut cursor, 487).unwrap());
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
    for pdu in [240, 480] {
        let limits = PduLimits::new(pdu).unwrap();
        for writing in [false, true] {
            let limit = if writing {
                limits.write_payload
            } else {
                limits.read_payload
            };
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
                    io.reads.push_back(Ok(data_frame(&pdu)));
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
                    )
                    .unwrap();
                    assert_eq!(buffer, expected);
                }
                let packets = requests(&io);
                assert_eq!(packets.len(), expected.chunks(limit).len());
                assert_eq!(chunks, packets.len());
                let mut offset = 0;
                for (index, packet) in packets.iter().enumerate() {
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
            &mut chunks
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
            &mut chunks
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
    )
    .unwrap();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &[],
        &mut chunks,
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
            &mut chunks
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
        &mut chunks
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
            &mut chunks
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
        &mut chunks
    )
    .is_err());
    assert_eq!(buffer, [42; 4]);
}
