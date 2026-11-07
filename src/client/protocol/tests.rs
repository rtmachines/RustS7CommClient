use super::*;
use crate::error::{assert_kind, kind};
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Scripted {
    reads: VecDeque<io::Result<Vec<u8>>>,
    writes: Vec<u8>,
    delay: Duration,
    timeouts: Vec<Duration>,
    write_error: Option<io::ErrorKind>,
    limit_error: Option<io::ErrorKind>,
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
        match self.limit_error {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
}

impl<T: AsRef<[u8]>> FrameRead for Cursor<T> {
    fn limit_next_read(&mut self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }
}

impl Write for Scripted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(kind) = self.write_error {
            return Err(kind.into());
        }
        self.writes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Bytes still queued for reading.
fn unread(io: &Scripted) -> usize {
    io.reads
        .iter()
        .map(|r| r.as_ref().map_or(0, Vec::len))
        .sum()
}

/// `validate_connection_confirm` on a watchdog thread: an option walk that stops
/// advancing fails the test instead of hanging the whole suite. After one hang,
/// later calls fail at once, so property shrinking cannot stall either.
fn confirm(frame: &[u8]) -> Result<u16> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static HUNG: AtomicBool = AtomicBool::new(false);
    assert!(
        !HUNG.load(Ordering::Relaxed),
        "validate_connection_confirm did not return earlier"
    );
    let frame = frame.to_vec();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(validate_connection_confirm(&frame));
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            HUNG.store(true, Ordering::Relaxed);
            panic!("validate_connection_confirm did not return within 2 s")
        }
        Err(e) => panic!("validate_connection_confirm panicked: {e}"),
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
fn read_tpkt_rejects_each_bad_header_byte_even_with_full_body() {
    for header in [[2, 0, 0, 10], [3, 1, 0, 10]] {
        let mut io = Cursor::new([&header[..], &[0; 6]].concat());
        assert_kind(
            &read_tpkt(&mut io, 487, Duration::from_secs(1)),
            kind::ISO_INVALID_HEADER,
        );
        assert_eq!(io.position(), 4);
    }
}

#[test]
fn read_tpkt_accepts_header_only_frame() {
    let mut io = Cursor::new([3, 0, 0, 4]);
    assert_eq!(
        read_tpkt(&mut io, 487, Duration::from_secs(1)).unwrap(),
        [3, 0, 0, 4]
    );
}

#[test]
fn read_tpkt_propagates_eof_and_timeout() {
    for io_kind in [
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::TimedOut,
        io::ErrorKind::WouldBlock,
    ] {
        let mut io = Scripted::pieces([vec![3, 0, 0, 22], vec![0; 3]]);
        if io_kind != io::ErrorKind::UnexpectedEof {
            io.reads.push_back(Err(io_kind.into()));
        }
        let result = read_tpkt(&mut io, 487, Duration::from_secs(1));
        if io_kind == io::ErrorKind::UnexpectedEof {
            assert_kind(&result, kind::CONNECTION_CLOSED);
        } else {
            assert_kind(&result, kind::IO);
            assert_eq!(
                result.unwrap_err().to_string(),
                "rust7::Io: no complete response frame within 1000 ms"
            );
        }
    }
}

#[test]
fn other_io_errors_are_not_reported_as_timeout() {
    for io_kind in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
    ] {
        let mut io = Scripted::pieces([vec![3, 0, 0, 22], vec![0; 3]]);
        io.reads.push_back(Err(io_kind.into()));
        let result = read_tpkt(&mut io, 487, Duration::from_secs(1));
        assert_kind(&result, kind::IO);
        let message = result.unwrap_err().to_string();
        assert!(!message.contains("no complete response frame"), "{message}");
        assert!(
            message.contains(&io::Error::from(io_kind).to_string()),
            "{message}"
        );
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
    confirm(&cc).unwrap();
    let cc_with_options = tpkt(&[9, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10]);
    confirm(&cc_with_options).unwrap();
    let bad_tlv = tpkt(&[9, 0xd0, 0, 1, 0, 1, 0, 0xc0, 2, 10]);
    assert!(confirm(&bad_tlv).is_err());
}

/// Copies of `frame` that differ from it only in one TPKT header field.
fn bad_tpkt_headers(frame: &[u8]) -> Vec<Vec<u8>> {
    let mut version = frame.to_vec();
    version[0] = 4;
    let mut reserved = frame.to_vec();
    reserved[1] = 1;
    let mut length = frame.to_vec();
    length[3] += 1;
    vec![version, reserved, length]
}

#[test]
fn frames_with_inconsistent_tpkt_header_are_rejected() {
    for bad in bad_tpkt_headers(&data_frame(&[0x32])) {
        assert_kind(&data_payload(&bad), kind::ISO_INVALID_HEADER);
    }
    for bad in bad_tpkt_headers(&connection_confirm(&[])) {
        assert_kind(&confirm(&bad), kind::ISO_CONNECTION_FAILED);
    }
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
fn negotiation_keeps_framing_error_as_source() {
    use std::error::Error as _;
    for (index, cause) in [
        (5, kind::ISO_INVALID_HEADER),
        (6, kind::ISO_FRAGMENTED_PACKET),
    ] {
        let mut bad = negotiation_response(480);
        bad[index] = 0;
        let mut io = Scripted::pieces([bad]);
        let err = negotiate(&mut io, 1024, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err.get_type(), Some(kind::PDU_NEGOTIATION_FAILED));
        assert_eq!(
            err.source()
                .and_then(|s| s.downcast_ref::<AnyError>())
                .and_then(|s| s.get_type()),
            Some(cause)
        );
    }
}

#[test]
fn pdu_limits_reject_invalid_sizes() {
    for n in [0, 1, 18, 27, 28, 481, 65535] {
        assert_kind(&PduLimits::new(n), kind::PDU_NEGOTIATION_FAILED);
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
                Err(ref e) if e.get_type() == Some(kind::PDU_NEGOTIATION_FAILED)
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
        let tpdu_limit = confirm(&connection_confirm(&options)).unwrap();
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
                    Err(ref e) if e.get_type() == Some(kind::PDU_NEGOTIATION_FAILED)
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
    let mut reference = 0;
    let mut progress = Progress::default();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(10, false),
        &[7, 8, 9],
        &mut progress,
        Duration::from_secs(1),
    )
    .unwrap();
    let packets = requests(&io);
    assert_eq!(progress.chunks, 3);
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
    confirm(&good).unwrap(); // Any source reference is accepted.
    for (index, value) in [(4, 7), (5, 0xe0), (5, 0xd1), (7, 2), (10, 0x10)] {
        let mut bad = good.clone();
        bad[index] = value;
        assert!(
            matches!(
                confirm(&bad),
                Err(ref e) if e.get_type() == Some(kind::ISO_CONNECTION_FAILED)
            ),
            "index {index}"
        );
    }
    assert!(confirm(&tpkt(&[7, 0xd0, 0, 1, 0, 1, 0, 0xc0])).is_err());
}

fn connection_confirm(options: &[u8]) -> Vec<u8> {
    let mut body = vec![6 + options.len() as u8, 0xd0, 0, 1, 0, 1, 0];
    body.extend_from_slice(options);
    tpkt(&body)
}

#[test]
fn connection_confirm_returns_effective_tpdu_limit() {
    assert_eq!(confirm(&connection_confirm(&[])).unwrap(), 1024);
    assert_eq!(
        confirm(&connection_confirm(&[0xc1, 2, 1, 0])).unwrap(),
        1024
    );
    // A zero-length option may end the frame.
    assert_eq!(confirm(&connection_confirm(&[0xc1, 0])).unwrap(), 1024);
    assert_eq!(
        confirm(&connection_confirm(&[0xc0, 1, 9, 0xc1, 0])).unwrap(),
        512
    );
    for (exponent, expected) in [(7, 128), (8, 256), (9, 512), (10, 1024)] {
        for options in [
            vec![0xc0, 1, exponent],
            vec![0xc1, 2, 1, 0, 0xc0, 1, exponent, 0xc2, 2, 1, 2],
        ] {
            assert_eq!(confirm(&connection_confirm(&options)).unwrap(), expected);
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
                confirm(&connection_confirm(&options)),
                Err(ref e) if e.get_type() == Some(kind::ISO_CONNECTION_FAILED)
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
        let expected = format!("(error class {:#04x}, code {:#04x})", error[0], error[1]);
        // A header-only ACK (ROSCTR 2) and a full ACK_DATA (ROSCTR 3) carry the same fields.
        let header_only = error_ack(0x1234);
        let read_data = ack(0x1234, 4, &[0xff, 4, 0, 32, 1, 2, 3, 4]);
        let write_data = ack(0x1234, 5, &[0xff]);
        for (mut read_pdu, mut write_pdu) in
            [(header_only.clone(), header_only), (read_data, write_data)]
        {
            read_pdu[10..12].copy_from_slice(&error);
            write_pdu[10..12].copy_from_slice(&error);
            let read = validate_read_response(&read_pdu, 0x1234, 2, 4);
            let write = validate_write_response(&write_pdu, 0x1234);
            assert_kind(&read, kind::S7_UNSPECIFIED);
            assert_kind(&write, kind::S7_UNSPECIFIED);
            assert!(read.unwrap_err().to_string().contains(&expected));
            assert!(write.unwrap_err().to_string().contains(&expected));
        }
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
            let mut reference = 0;
            let mut progress = Progress::default();
            let result = if writing {
                write_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(17, false),
                    &buffer,
                    &mut progress,
                    Duration::from_secs(1),
                )
            } else {
                read_transfer(
                    &mut io,
                    &limits,
                    &mut reference,
                    transfer(17, false),
                    &mut buffer,
                    &mut progress,
                    Duration::from_secs(1),
                )
            };
            assert_kind(&result, kind::S7_UNSPECIFIED);
            assert_eq!(progress.chunks, completed + 1);
            assert_eq!(usize::from(reference), progress.chunks);
            assert_eq!(requests(&io).len(), progress.chunks);
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
                    &mut progress,
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
                    &mut progress,
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
                Err(ref e) if e.get_type() == Some(kind::ISO_INVALID_TELEGRAM)
            ),
            "read {pdu:02x?}"
        );
        assert!(
            matches!(
                validate_write_response(&pdu, 0x1234),
                Err(ref e) if e.get_type() == Some(kind::ISO_INVALID_TELEGRAM)
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
                (expected, err.get_type()),
                ("address", Some(kind::S7_INVALID_ADDRESS))
                    | ("missing", Some(kind::S7_NOT_FOUND))
                    | ("other", Some(kind::S7_UNSPECIFIED))
            ));
            if expected == "other" {
                assert!(err.to_string().contains("(item return code 0x07)"), "{err}");
            }
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
        let before = cursor.position();
        result.push(read_tpkt(&mut cursor, 487, Duration::from_secs(1)).unwrap());
        // Without progress this loop would grow `result` until memory runs out.
        assert!(cursor.position() > before, "read_tpkt consumed no input");
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
            let mut reference = 0;
            let mut progress = Progress::default();
            let mut buffer = [0; 4];
            read_transfer(
                &mut io,
                &limits,
                &mut reference,
                transfer(start, true),
                &mut buffer,
                &mut progress,
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
                &mut progress,
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
    assert_kind(&bit_start(1, 8), kind::INVALID_FUN_PARAMETER);
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
                let mut reference = 0;
                let mut progress = Progress::default();
                if writing {
                    write_transfer(
                        &mut io,
                        &limits,
                        &mut reference,
                        transfer(17, false),
                        &expected,
                        &mut progress,
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
                        &mut progress,
                        Duration::from_secs(1),
                    )
                    .unwrap();
                    assert_eq!(buffer, expected);
                }
                let packets = requests(&io);
                assert_eq!(packets.len(), expected.chunks(limit).len());
                assert_eq!(progress.chunks, packets.len());
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
    let mut reference = 0;
    let mut progress = Progress::default();
    assert_kind(
        &read_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, true),
            &mut [],
            &mut progress,
            Duration::from_secs(1),
        ),
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, true),
            &[],
            &mut progress,
            Duration::from_secs(1),
        ),
        kind::INVALID_FUN_PARAMETER,
    );
    assert!(io.writes.is_empty());
    assert_eq!((reference, progress.chunks), (0, 0));
}

#[test]
fn byte_empty_is_noop() {
    let mut io = Scripted::default();
    let limits = PduLimits::new(480).unwrap();
    let mut reference = 0;
    let mut progress = Progress::default();
    read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &mut [],
        &mut progress,
        Duration::from_secs(1),
    )
    .unwrap();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &[],
        &mut progress,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(io.writes.is_empty());
    assert_eq!((reference, progress.chunks), (0, 0));
}

#[test]
fn address_range_checked_before_io() {
    assert_eq!(validate_transfer(0x1fffff, 2, 1).unwrap(), 1);
    assert!(validate_transfer(0x1fffff, 2, 2).is_err());
    assert!(validate_transfer(1, 2, usize::MAX).is_err());
    assert!(validate_transfer(0x1000000, 1, 1).is_err());
    let mut io = Scripted::default();
    let limits = PduLimits::new(480).unwrap();
    let mut reference = 0;
    let mut progress = Progress::default();
    let buffer = vec![0; 0x200000 - 65535 + 1];
    assert_kind(
        &write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(65535, false),
            &buffer,
            &mut progress,
            Duration::from_secs(1),
        ),
        kind::INVALID_FUN_PARAMETER,
    );
    assert!(io.writes.is_empty());
    let mut buffer = buffer;
    assert!(read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(65535, false),
        &mut buffer,
        &mut progress,
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
    let mut reference = 0;
    let mut progress = Progress::default();
    assert_kind(
        &write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, false),
            &vec![42; 600],
            &mut progress,
            Duration::from_secs(1),
        ),
        kind::S7_INVALID_ADDRESS,
    );
    let packets = requests(&io);
    assert_eq!(packets.len(), 2);
    assert_eq!(progress.chunks, 2);
    assert_eq!(progress.confirmed, 212);
    assert_eq!(&packets[0][28..31], &[0, 0, 0]);
    assert_eq!(&packets[1][28..31], &[0, 6, 0xa0]);
}

#[test]
fn read_progress_counts_only_accepted_chunks() {
    let limits = PduLimits::new(240).unwrap();
    let mut io = Scripted::pieces([
        data_frame(&read_ack(1, &[7; 222], false)),
        data_frame(&ack(2, 4, &[5, 0, 0, 0])),
    ]);
    let mut reference = 0;
    let mut progress = Progress::default();
    let mut buffer = vec![0xaa; 600];
    assert_kind(
        &read_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, false),
            &mut buffer,
            &mut progress,
            Duration::from_secs(1),
        ),
        kind::S7_INVALID_ADDRESS,
    );
    assert_eq!((progress.chunks, progress.confirmed), (2, 222));
    assert_eq!(buffer[..222], [7; 222]);
    assert!(buffer[222..].iter().all(|b| *b == 0xaa));
}

#[test]
fn bit_write_confirms_one_byte() {
    let limits = PduLimits::new(240).unwrap();
    let mut io = Scripted::pieces([data_frame(&ack(1, 5, &[0xff]))]);
    let mut reference = 0;
    let mut progress = Progress::default();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, true),
        &[1, 2, 3],
        &mut progress,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(progress.confirmed, 1);
}

#[test]
fn empty_byte_transfer_confirms_nothing() {
    let limits = PduLimits::new(240).unwrap();
    let mut io = Scripted::pieces([]);
    let mut reference = 0;
    let mut progress = Progress::default();
    write_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &[],
        &mut progress,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!((progress.confirmed, progress.chunks), (0, 0));
}

#[test]
fn short_read_leaves_destination_unchanged() {
    let limits = PduLimits::new(480).unwrap();
    let mut io = Scripted::pieces([data_frame(&read_ack(1, &[1], false))]);
    let mut reference = 0;
    let mut progress = Progress::default();
    let mut buffer = [42; 4];
    assert!(read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &mut buffer,
        &mut progress,
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
        assert_kind(&remote_tsap(1, rack, slot), kind::INVALID_FUN_PARAMETER);
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
        let mut reference = 0;
        let mut progress = Progress::default();
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(3, true),
            &input,
            &mut progress,
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
    assert_eq!(err.get_type(), Some(kind::IO));
    assert!(err
        .to_string()
        .contains("no complete response frame within 100 ms"));
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

#[test]
fn huge_timeout_with_wouldblock_reports_unified_message() {
    let mut io = Scripted::pieces([vec![3, 0, 0, 22]]);
    io.reads.push_back(Err(io::ErrorKind::WouldBlock.into()));
    let err = read_tpkt(&mut io, 487, Duration::from_millis(u64::MAX)).unwrap_err();
    assert_eq!(err.get_type(), Some(kind::IO));
    assert!(err.to_string().contains("within 18446744073709551615 ms"));
}

#[test]
fn interrupted_read_is_retried() {
    let frame = data_frame(&[0x32; 20]);
    let mut io = Scripted::pieces([]);
    io.reads.push_back(Err(io::ErrorKind::Interrupted.into()));
    io.reads.push_back(Ok(frame.clone()));
    assert_eq!(
        read_tpkt(&mut io, 487, Duration::from_secs(1)).unwrap(),
        frame
    );
}

#[test]
fn read_payload_length_must_match_bits_field() {
    let limits = PduLimits::new(480).unwrap();
    let mut data = vec![0xff, 4, 0, 32];
    data.extend_from_slice(&[9; 5]);
    let mut io = Scripted::pieces([data_frame(&ack(1, 4, &data))]);
    let mut reference = 0;
    let mut progress = Progress::default();
    let mut buffer = [0xaa; 4];
    let result = read_transfer(
        &mut io,
        &limits,
        &mut reference,
        transfer(0, false),
        &mut buffer,
        &mut progress,
        Duration::from_secs(1),
    );
    assert_kind(&result, kind::ISO_INVALID_TELEGRAM);
    assert_eq!(buffer, [0xaa; 4]);
}

#[test]
fn read_item_shorter_than_header_is_rejected() {
    assert_kind(
        &validate_read_response(&ack(1, 4, &[0xff, 4, 0]), 1, 2, 4),
        kind::ISO_INVALID_TELEGRAM,
    );
}

#[test]
fn write_ack_with_extra_data_is_not_success() {
    for data in [&[0xff, 0x00][..], &[][..]] {
        assert_kind(
            &validate_write_response(&ack(1, 5, data), 1),
            kind::ISO_INVALID_TELEGRAM,
        );
    }
}

#[test]
fn ack_requires_exactly_two_parameter_bytes() {
    // Same bytes as a valid write ACK, with the parameter/data split moved.
    for (params, data) in [(3, 0), (0, 3)] {
        let mut pdu = ack(1, 5, &[0xff]);
        pdu[7] = params;
        pdu[9] = data;
        assert_kind(
            &validate_write_response(&pdu, 1),
            kind::ISO_INVALID_TELEGRAM,
        );
    }
}

#[test]
fn ack_without_data_leaves_payload_checks_to_caller() {
    assert_eq!(ack_data(&ack(1, 5, &[]), 1, 5).unwrap(), &[] as &[u8]);
}

// Reference frames follow the S7comm layout (Snap7, Wireshark s7comm
// dissector). Without a PLC capture they are a reference, not proof of
// firmware conformance.
#[test]
fn requests_match_reference_bytes() {
    // TPKT(4) | COTP DT(3) | Job header(10) | parameters | data header
    let read_bytes = Transfer {
        area: 0x84,
        db_number: 0x1234,
        start: 0x0102,
        wordlen: 2,
    };
    assert_eq!(
        request(&read_bytes, 0, 3, 0x0a0b, None),
        vec![
            3, 0, 0, 0x1f, // TPKT: version, reserved, length
            2, 0xf0, 0x80, // COTP DT
            0x32, 1, 0, 0, 0x0a, 0x0b, 0, 0x0e, 0, 0, // S7 header: Job, ref, lens
            4, 1, 0x12, 0x0a, 0x10, 2, 0, 3, 0x12, 0x34, 0x84, 0, 0x08, 0x10,
        ]
    );
    let read_bit = Transfer {
        area: 0x83,
        db_number: 0,
        start: 83,
        wordlen: 1,
    };
    assert_eq!(
        request(&read_bit, 0, 1, 0x0102, None),
        vec![
            3, 0, 0, 0x1f, 2, 0xf0, 0x80, 0x32, 1, 0, 0, 0x01, 0x02, 0, 0x0e, 0, 0, //
            4, 1, 0x12, 0x0a, 0x10, 1, 0, 1, 0, 0, 0x83, 0, 0, 0x53,
        ]
    );
    assert_eq!(
        request(&read_bytes, 0, 3, 0x0a0c, Some(&[0xde, 0xad, 0xbe])),
        vec![
            3, 0, 0, 0x26, 2, 0xf0, 0x80, 0x32, 1, 0, 0, 0x0a, 0x0c, 0, 0x0e, 0, 7, //
            5, 1, 0x12, 0x0a, 0x10, 2, 0, 3, 0x12, 0x34, 0x84, 0, 0x08, 0x10, //
            0, 4, 0, 0x18, 0xde, 0xad, 0xbe,
        ]
    );
    let write_bit = Transfer {
        area: 0x82,
        db_number: 0,
        start: 83,
        wordlen: 1,
    };
    assert_eq!(
        request(&write_bit, 0, 1, 0x0a0d, Some(&[0])),
        vec![
            3, 0, 0, 0x24, 2, 0xf0, 0x80, 0x32, 1, 0, 0, 0x0a, 0x0d, 0, 0x0e, 0, 5, //
            5, 1, 0x12, 0x0a, 0x10, 1, 0, 1, 0, 0, 0x82, 0, 0, 0x53, //
            0, 3, 0, 1, 0,
        ]
    );
}

mod properties {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    fn read_item() -> impl Strategy<Value = Vec<u8>> {
        (
            prop_oneof![4 => Just(0xffu8), 1 => any::<u8>()],
            prop_oneof![4 => Just(4u8), 2 => Just(3u8), 1 => any::<u8>()],
            vec(any::<u8>(), 0..=40),
            prop_oneof![
                Just(None),
                any::<u16>().prop_map(Some),
                (0u16..=40).prop_map(|n| Some(n * 8)),
            ],
        )
            .prop_map(|(status, transport, payload, bits)| {
                let bits = bits.unwrap_or((payload.len() * 8) as u16);
                let mut item = vec![status, transport, (bits >> 8) as u8, bits as u8];
                item.extend_from_slice(&payload);
                item
            })
    }

    fn short_item() -> impl Strategy<Value = Vec<u8>> {
        vec(any::<u8>(), 0..=3).prop_map(|rest| [&[0xff][..], &rest].concat())
    }

    fn coherent_ack() -> impl Strategy<Value = Vec<u8>> {
        (
            prop_oneof![6 => Just(3u8), 1 => Just(2u8), 1 => any::<u8>()],
            prop_oneof![6 => Just(1u16), 1 => any::<u16>()],
            prop_oneof![6 => Just([0u8, 0]), 1 => any::<[u8; 2]>()],
            prop_oneof![
                3 => Just(vec![4u8, 1]),
                2 => Just(vec![5u8, 1]),
                1 => vec(any::<u8>(), 0..=4)
            ],
            prop_oneof![
                4 => read_item(),
                2 => short_item(),
                1 => Just(Vec::new()),
                1 => vec(any::<u8>(), 0..=40)
            ],
        )
            .prop_map(|(rosctr, reference, error, params, data)| {
                let mut pdu = vec![0x32, rosctr, 0, 0];
                pdu.extend_from_slice(&reference.to_be_bytes());
                pdu.extend_from_slice(&(params.len() as u16).to_be_bytes());
                pdu.extend_from_slice(&(data.len() as u16).to_be_bytes());
                pdu.extend_from_slice(&error);
                pdu.extend_from_slice(&params);
                pdu.extend_from_slice(&data);
                pdu
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn coherent_ack_never_panics(pdu in coherent_ack(), n in prop_oneof![1usize..=40, 1usize..=462], bit in any::<bool>()) {
            let (wl, n) = if bit { (1, 1) } else { (2, n) };
            match validate_read_response(&pdu, 1, wl, n) {
                Ok(data) => prop_assert_eq!(data.len(), n),
                Err(e) => prop_assert!(e.get_type().is_some_and(|k| kind::ALL.contains(&k))),
            }
            if let Err(e) = validate_write_response(&pdu, 1) {
                prop_assert!(e.get_type().is_some_and(|k| kind::ALL.contains(&k)));
            }
        }

        #[test]
        fn read_transfer_never_panics_and_keeps_buffer_on_error(pdu in coherent_ack(), n in prop_oneof![1usize..=40, 1usize..=462]) {
            let limits = PduLimits::new(480).unwrap();
            let mut io = Scripted::pieces([data_frame(&pdu)]);
            let mut reference = 0;
            let mut progress = Progress::default();
            let mut buffer = vec![0xaa; n];
            let result = read_transfer(&mut io, &limits, &mut reference, transfer(0, false),
                &mut buffer, &mut progress, Duration::from_secs(1));
            // n fits one chunk, so the response is a single frame whose payload ends the PDU.
            prop_assert_eq!(progress.chunks, 1);
            if result.is_ok() {
                prop_assert_eq!(&buffer[..], &pdu[pdu.len() - n..]);
                prop_assert_eq!(progress.confirmed, n);
            } else {
                prop_assert!(buffer.iter().all(|b| *b == 0xaa));
                prop_assert_eq!(progress.confirmed, 0);
            }
        }

        #[test]
        fn read_transfer_copies_valid_payload_and_confirms_it(payload in vec(any::<u8>(), 1..=462)) {
            let limits = PduLimits::new(480).unwrap();
            let n = payload.len();
            let mut io = Scripted::pieces([data_frame(&read_ack(1, &payload, false))]);
            let mut reference = 0;
            let mut progress = Progress::default();
            let mut buffer = vec![0xaa; n];
            read_transfer(&mut io, &limits, &mut reference, transfer(0, false),
                &mut buffer, &mut progress, Duration::from_secs(1)).unwrap();
            prop_assert_eq!(&buffer, &payload);
            prop_assert_eq!((progress.chunks, progress.confirmed), (1, n));
        }

        #[test]
        fn read_accepts_only_payload_of_requested_length(n in 1usize..=40, payload in vec(any::<u8>(), 0..=60)) {
            let bits = (n * 8) as u16;
            let mut item = vec![0xff, 4, (bits >> 8) as u8, bits as u8];
            item.extend_from_slice(&payload);
            let pdu = ack(1, 4, &item);
            let result = validate_read_response(&pdu, 1, 2, n);
            if payload.len() == n {
                prop_assert_eq!(result.unwrap(), &payload[..]);
            } else {
                prop_assert!(result.is_err());
            }
        }
    }
}

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn negotiation_reports_transport_errors_unchanged() {
    // A failed send is an I/O error and no reply is consumed.
    let mut io = Scripted::pieces([negotiation_response(480)]);
    io.write_error = Some(io::ErrorKind::BrokenPipe);
    assert_kind(&negotiate(&mut io, 1024, SECOND), kind::IO);
    assert_eq!(unread(&io), 27);
    // The peer closing before the Setup Communication reply ends the stream.
    let mut io = Scripted::default();
    assert_kind(&negotiate(&mut io, 1024, SECOND), kind::CONNECTION_CLOSED);
    assert_eq!(io.writes.len(), 25);
}

#[test]
fn read_tpkt_length_limit_is_inclusive() {
    for (len, accepted) in [(486u16, true), (487, true), (488, false)] {
        let header = [3, 0, (len >> 8) as u8, len as u8];
        let body = vec![0x5a; usize::from(len) - 4];
        let mut io = Scripted::pieces([header.to_vec(), body.clone()]);
        let result = read_tpkt(&mut io, 487, SECOND);
        if accepted {
            assert_eq!(result.unwrap(), [&header[..], &body].concat());
            assert_eq!(unread(&io), 0);
        } else {
            assert_kind(&result, kind::ISO_INVALID_TELEGRAM);
            assert_eq!(unread(&io), body.len(), "the body must stay unread");
        }
    }
    for header in [[2, 0, 0, 10], [3, 1, 0, 10]] {
        let mut io = Scripted::pieces([header.to_vec(), vec![0; 6]]);
        assert_kind(&read_tpkt(&mut io, 487, SECOND), kind::ISO_INVALID_HEADER);
        assert_eq!(unread(&io), 6);
    }
}

#[test]
fn unrepresentable_deadline_bounds_each_read_by_the_timeout() {
    let frame = data_frame(&[0x32; 20]);
    let mut io = Scripted::pieces([frame[..4].to_vec(), frame[4..].to_vec()]);
    assert_eq!(read_tpkt(&mut io, 487, Duration::MAX).unwrap(), frame);
    assert_eq!(io.timeouts, [Duration::MAX, Duration::MAX]);
}

#[test]
fn failure_to_bound_a_read_is_an_io_error_before_reading() {
    let frame = data_frame(&[0x32]);
    let mut io = Scripted::pieces([frame.clone()]);
    io.limit_error = Some(io::ErrorKind::InvalidInput);
    assert_kind(&read_tpkt(&mut io, 487, SECOND), kind::IO);
    assert_eq!(unread(&io), frame.len());
}

#[test]
fn read_response_shape_is_fixed_by_wordlen() {
    // Bit access reads exactly one element; a bit-shaped item cannot stand for two bytes.
    let two_bits = ack(1, 4, &[0xff, 3, 0, 1, 7, 7]);
    assert_kind(
        &validate_read_response(&two_bits, 1, 1, 2),
        kind::ISO_INVALID_TELEGRAM,
    );
    let bit = read_ack(1, &[1], true);
    for wordlen in [0, 3, 0xff] {
        assert_kind(
            &validate_read_response(&bit, 1, wordlen, 1),
            kind::ISO_INVALID_TELEGRAM,
        );
    }
    // A byte count whose bit length overflows is rejected, not wrapped.
    let bytes = read_ack(1, &[0; 4], false);
    assert_kind(
        &validate_read_response(&bytes, 1, 2, usize::MAX / 8 + 1),
        kind::ISO_INVALID_TELEGRAM,
    );
}

#[test]
fn address_limits_accept_the_last_address_and_reject_the_next() {
    // Bit access: 24-bit bit address, so 0xffffff is the last valid start.
    for (start, expected) in [(0xfffffe, Some(1)), (0xffffff, Some(1)), (0x1000000, None)] {
        assert_eq!(
            validate_transfer(start, 1, 1).ok(),
            expected,
            "bit {start:#x}"
        );
    }
    // Byte access: the last byte may be 0x1fffff (bit address 0xfffff8).
    for (start, len, expected) in [
        (0x1ffffe, 2, Some(2)),
        (0x1ffffe, 3, None),
        (0x1fffff, 1, Some(1)),
        (0x1fffff, 0, Some(0)),
        (0x200000, 0, None),
        (0, 0x200000, Some(0x200000)),
        (0, 0x200001, None),
    ] {
        assert_eq!(
            validate_transfer(start, 2, len).ok(),
            expected,
            "byte {start:#x}+{len:#x}"
        );
    }
}

#[test]
fn byte_range_overflow_is_an_invalid_parameter() {
    for (start, len) in [(2, usize::MAX), (0x1fffff, usize::MAX - 1)] {
        assert_kind(
            &validate_transfer(start, 2, len),
            kind::INVALID_FUN_PARAMETER,
        );
    }
}

#[test]
fn db_number_is_sent_only_for_the_db_area() {
    let limits = PduLimits::new(480).unwrap();
    for (area, db) in [(0x81, 0u16), (0x82, 0), (0x83, 0), (0x84, 100)] {
        let mut io = Scripted::pieces([
            data_frame(&read_ack(1, &[7], false)),
            data_frame(&ack(2, 5, &[0xff])),
        ]);
        let mut reference = 0;
        let mut progress = Progress::default();
        let at = |start| Transfer {
            area,
            db_number: 100,
            start,
            wordlen: 2,
        };
        read_transfer(
            &mut io,
            &limits,
            &mut reference,
            at(3),
            &mut [0],
            &mut progress,
            SECOND,
        )
        .unwrap();
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            at(3),
            &[9],
            &mut progress,
            SECOND,
        )
        .unwrap();
        for packet in requests(&io) {
            assert_eq!(&packet[25..27], &db.to_be_bytes(), "area {area:#04x}");
            assert_eq!(packet[27], area);
        }
    }
}

fn run_transfer(
    io: &mut Scripted,
    writing: bool,
    transfer: Transfer,
    buffer: &mut [u8],
) -> (Result<()>, Progress, u16) {
    let limits = PduLimits::new(29).unwrap();
    let mut reference = 0;
    let mut progress = Progress::default();
    let result = if writing {
        write_transfer(
            io,
            &limits,
            &mut reference,
            transfer,
            buffer,
            &mut progress,
            SECOND,
        )
    } else {
        read_transfer(
            io,
            &limits,
            &mut reference,
            transfer,
            buffer,
            &mut progress,
            SECOND,
        )
    };
    (result, progress, reference)
}

#[test]
fn transfers_reject_unknown_area_before_io() {
    for writing in [false, true] {
        for (area, valid) in [
            (0x00, false),
            (0x80, false),
            (0x81, true),
            (0x82, true),
            (0x83, true),
            (0x84, true),
            (0x85, false),
            (0xff, false),
        ] {
            let reply = data_frame(&if writing {
                ack(1, 5, &[0xff])
            } else {
                read_ack(1, &[7], false)
            });
            let mut io = Scripted::pieces([reply]);
            let transfer = Transfer {
                area,
                db_number: 1,
                start: 0,
                wordlen: 2,
            };
            let (result, progress, reference) = run_transfer(&mut io, writing, transfer, &mut [0]);
            if valid {
                result.unwrap();
                assert_eq!((progress.chunks, progress.confirmed, reference), (1, 1, 1));
            } else {
                assert_kind(&result, kind::INVALID_FUN_PARAMETER);
                assert!(io.writes.is_empty(), "area {area:#04x}");
                assert_eq!((progress.chunks, progress.confirmed, reference), (0, 0, 0));
                assert_eq!(unread(&io), io.reads[0].as_ref().unwrap().len());
            }
        }
    }
}

#[test]
fn failed_send_counts_the_chunk_but_confirms_nothing() {
    for writing in [false, true] {
        let mut io = Scripted::pieces([data_frame(&ack(1, 5, &[0xff]))]);
        io.write_error = Some(io::ErrorKind::ConnectionReset);
        let mut buffer = [0xaa; 2];
        let (result, progress, reference) =
            run_transfer(&mut io, writing, transfer(0, false), &mut buffer);
        assert_kind(&result, kind::IO);
        assert_eq!((progress.chunks, progress.confirmed, reference), (1, 0, 1));
        assert_eq!(buffer, [0xaa; 2]);
        assert_eq!(unread(&io), 22, "no reply may be read after a failed send");
    }
}

#[test]
fn cotp_errors_in_transfer_replies_keep_their_kind() {
    for writing in [false, true] {
        for (index, expected) in [
            (5, kind::ISO_INVALID_HEADER),
            (6, kind::ISO_FRAGMENTED_PACKET),
        ] {
            let mut reply = data_frame(&if writing {
                ack(1, 5, &[0xff])
            } else {
                read_ack(1, &[7], false)
            });
            reply[index] = 0;
            let mut io = Scripted::pieces([reply]);
            let mut buffer = [0xaa];
            let (result, progress, _) =
                run_transfer(&mut io, writing, transfer(0, false), &mut buffer);
            assert_kind(&result, expected);
            assert_eq!((progress.chunks, progress.confirmed), (1, 0));
            assert_eq!(buffer, [0xaa]);
        }
    }
}

#[test]
fn reply_frame_may_not_exceed_pdu_plus_tpkt_and_cotp_headers() {
    // A reply carries at most `pdu_length` (29) bytes of S7 PDU behind the
    // 4-byte TPKT and 3-byte COTP headers, so 36 bytes is the largest frame.
    for writing in [false, true] {
        for total in [35, 36, 37] {
            let mut io = Scripted::pieces([data_frame(&vec![0; total - 7])]);
            let (result, _, _) = run_transfer(&mut io, writing, transfer(0, false), &mut [0]);
            assert_kind(&result, kind::ISO_INVALID_TELEGRAM);
            let expected = if total <= 36 { 0 } else { total - 4 };
            assert_eq!(unread(&io), expected, "total {total}, writing {writing}");
        }
    }
}

#[test]
fn write_request_declares_item_header_plus_payload_as_data_length() {
    let limits = PduLimits::new(480).unwrap();
    for (bit, buffer, count) in [
        (false, vec![1, 2, 3], 3usize),
        (true, vec![1], 1),
        (false, vec![9; 452], 452),
    ] {
        let mut io = Scripted::pieces([data_frame(&ack(1, 5, &[0xff]))]);
        let mut reference = 0;
        let mut progress = Progress::default();
        write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer(0, bit),
            &buffer,
            &mut progress,
            SECOND,
        )
        .unwrap();
        let packet = &requests(&io)[0];
        // Data section: return code, transport size, 16-bit length, payload.
        assert_eq!(&packet[15..17], &((4 + count) as u16).to_be_bytes());
        assert_eq!(packet.len(), 31 + 4 + count);
    }
}

mod invariants {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    fn is_kind<T>(result: &Result<T>, expected: &str) -> bool {
        matches!(result, Err(e) if e.get_type() == Some(expected))
    }

    fn header_byte(valid: u8) -> impl Strategy<Value = u8> {
        prop_oneof![3 => Just(valid), 1 => any::<u8>()]
    }

    fn tlv() -> impl Strategy<Value = Vec<u8>> {
        (
            any::<u8>().prop_filter("not the TPDU size code", |c| *c != 0xc0),
            vec(any::<u8>(), 0..=4),
        )
            .prop_map(|(code, value)| [&[code, value.len() as u8][..], &value].concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

        /// TPKT: a frame is read whole iff version 3, reserved 0 and 4 <= length <= limit;
        /// otherwise only the 4 header bytes are consumed and the body stays queued.
        #[test]
        fn read_tpkt_reads_body_only_after_a_valid_header(
            version in header_byte(3),
            reserved in header_byte(0),
            len in prop_oneof![0u16..=8, 480u16..=500, any::<u16>()],
            max_total in prop_oneof![Just(487usize), 4usize..=600],
        ) {
            let header = [version, reserved, (len >> 8) as u8, len as u8];
            let body = vec![0x5a; usize::from(len).saturating_sub(4)];
            let mut io = Scripted::pieces([header.to_vec(), body.clone()]);
            let result = read_tpkt(&mut io, max_total, SECOND);
            if version != 3 || reserved != 0 {
                prop_assert!(is_kind(&result, kind::ISO_INVALID_HEADER));
                prop_assert_eq!(unread(&io), body.len());
            } else if usize::from(len) < 4 || usize::from(len) > max_total {
                prop_assert!(is_kind(&result, kind::ISO_INVALID_TELEGRAM));
                prop_assert_eq!(unread(&io), body.len());
            } else {
                prop_assert_eq!(result.unwrap(), [&header[..], &body].concat());
                prop_assert_eq!(unread(&io), 0);
            }
        }

        /// COTP DT roundtrip: wrapping a nonempty S7 PDU in TPKT + DT header and unwrapping
        /// it is the identity.
        #[test]
        fn data_payload_inverts_data_frame(pdu in vec(any::<u8>(), 1..=500)) {
            let frame = data_frame(&pdu);
            prop_assert_eq!(data_payload(&frame).unwrap(), &pdu[..]);
        }

        /// COTP DT: changing any TPKT/COTP header byte is rejected. A TPKT or DT header
        /// change is an invalid header; a DT without the EOT bit (byte 6) is a fragment.
        #[test]
        fn data_payload_rejects_any_header_corruption(pdu in vec(any::<u8>(), 1..=40), index in 0usize..7, flip in 1u8..=255) {
            let mut frame = data_frame(&pdu);
            frame[index] ^= flip;
            let result = data_payload(&frame);
            if index < 6 {
                prop_assert!(is_kind(&result, kind::ISO_INVALID_HEADER));
            } else if frame[6] & 0x80 == 0 {
                prop_assert!(is_kind(&result, kind::ISO_FRAGMENTED_PACKET));
            } else {
                // EOT set with a nonzero TPDU-NR, unused in class 0: rejected, kind unspecified.
                prop_assert!(result.is_err());
            }
        }

        /// CC: any well-formed option list yields 2^exponent of its single C0 option, or 1024 without one.
        #[test]
        fn connection_confirm_returns_the_tpdu_size_option(
            before in vec(tlv(), 0..4),
            exponent in proptest::option::of(7u8..=10),
            after in vec(tlv(), 0..4),
        ) {
            let size = exponent.map(|e| vec![0xc0, 1, e]).unwrap_or_default();
            let options = [before.concat(), size, after.concat()].concat();
            let expected = exponent.map_or(1024, |e| 1u16 << e);
            prop_assert_eq!(confirm(&connection_confirm(&options)).unwrap(), expected);
        }

        /// CC: an option cut short by the end of the frame is rejected, never read past the frame.
        #[test]
        fn connection_confirm_rejects_truncated_option(options in vec(tlv(), 1..4), cut in any::<prop::sample::Index>()) {
            let last = options.last().unwrap().len();
            let cut = 1 + cut.index(last - 1);
            let mut bytes = options.concat();
            bytes.truncate(bytes.len() - cut);
            prop_assert!(is_kind(&confirm(&connection_confirm(&bytes)), kind::ISO_CONNECTION_FAILED));
        }

        /// PDU limits: 29..=480 accepted; the read payload leaves room for the 18-byte read
        /// ACK_DATA overhead and the write payload for the 28-byte write request overhead.
        #[test]
        fn pdu_limits_reserve_s7_overhead(n in any::<u16>()) {
            match PduLimits::new(n) {
                Ok(limits) => {
                    prop_assert!((29..=480).contains(&n));
                    prop_assert_eq!(limits.read_payload + 18, usize::from(n));
                    prop_assert_eq!(limits.write_payload + 28, usize::from(n));
                }
                Err(e) => {
                    prop_assert!(!(29..=480).contains(&n));
                    prop_assert_eq!(e.get_type(), Some(kind::PDU_NEGOTIATION_FAILED));
                }
            }
        }

        /// Address range: a bit transfer is one element at a 24-bit bit address; a byte
        /// transfer's last byte must lie at or below 0x1fffff (its bit address fits 24 bits).
        #[test]
        fn validate_transfer_matches_the_24_bit_address_space(
            start in prop_oneof![0u32..=16, 0x1ffffcu32..=0x200002, 0xfffffcu32..=0x1000002, any::<u32>()],
            len in prop_oneof![0usize..=16, 0x1ffff0usize..=0x200010, Just(usize::MAX), any::<usize>()],
            wordlen in prop_oneof![Just(1u8), Just(2u8), any::<u8>()],
        ) {
            let expected = match wordlen {
                1 => (len > 0 && start <= 0xff_ffff).then_some(1),
                2 => (u128::from(start) + len.max(1) as u128 - 1 <= 0x1f_ffff).then_some(len),
                _ => None,
            };
            match validate_transfer(start, wordlen, len) {
                Ok(n) => prop_assert_eq!(Some(n), expected),
                Err(e) => {
                    prop_assert_eq!(expected, None);
                    prop_assert_eq!(e.get_type(), Some(kind::INVALID_FUN_PARAMETER));
                }
            }
        }

        /// Area: only PE, PA, MK and DB (0x81..=0x84) are accepted, and the check needs no I/O.
        #[test]
        fn only_the_four_s7_areas_are_accepted(area in prop_oneof![0x7fu8..=0x86, any::<u8>()], writing in any::<bool>()) {
            let mut io = Scripted::default();
            let transfer = Transfer { area, db_number: 1, start: 0, wordlen: 2 };
            let (result, progress, _) = run_transfer(&mut io, writing, transfer, &mut []);
            if (0x81..=0x84).contains(&area) {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(is_kind(&result, kind::INVALID_FUN_PARAMETER));
            }
            prop_assert!(io.writes.is_empty());
            prop_assert_eq!(progress.chunks, 0);
        }

        /// TSAP: the remote TSAP decodes back to (connection type, rack, slot) for every
        /// rack 0..=7 and slot 0..=31; anything outside is rejected.
        #[test]
        fn remote_tsap_decodes_to_its_fields(conn_type in 0u16..=0xff, rack in 0u16..=12, slot in 0u16..=40) {
            match remote_tsap(conn_type, rack, slot) {
                Ok(tsap) => {
                    prop_assert!(rack <= 7 && slot <= 31);
                    let [hi, lo] = tsap.to_be_bytes();
                    prop_assert_eq!((u16::from(hi), u16::from(lo >> 5), u16::from(lo & 0x1f)), (conn_type, rack, slot));
                }
                Err(e) => {
                    prop_assert!(rack > 7 || slot > 31);
                    prop_assert_eq!(e.get_type(), Some(kind::INVALID_FUN_PARAMETER));
                }
            }
        }

        /// Chunking: a transfer is split into ceil(len / payload) requests with consecutive
        /// references and addresses, each carrying min(remaining, payload) bytes, and the
        /// chunks together reproduce the buffer exactly.
        #[test]
        fn transfers_split_into_consecutive_bounded_chunks(
            pdu in 29u16..=480,
            len in 0usize..=1500,
            start in 0u32..=0x1000,
            writing in any::<bool>(),
        ) {
            let limits = PduLimits::new(pdu).unwrap();
            let limit = usize::from(pdu) - if writing { 28 } else { 18 };
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let mut io = Scripted::default();
            for (index, chunk) in data.chunks(limit).enumerate() {
                let reference = (index + 1) as u16;
                let pdu = if writing { ack(reference, 5, &[0xff]) } else { read_ack(reference, chunk, false) };
                io.reads.push_back(Ok(data_frame(&pdu)));
            }
            let mut reference = 0;
            let mut progress = Progress::default();
            let mut buffer = vec![0xaa; len];
            let at = Transfer { area: 0x84, db_number: 100, start, wordlen: 2 };
            if writing {
                write_transfer(&mut io, &limits, &mut reference, at, &data, &mut progress, SECOND).unwrap();
            } else {
                read_transfer(&mut io, &limits, &mut reference, at, &mut buffer, &mut progress, SECOND).unwrap();
                prop_assert_eq!(&buffer, &data);
            }
            let packets = requests(&io);
            prop_assert_eq!(packets.len(), len.div_ceil(limit));
            prop_assert_eq!((progress.chunks, progress.confirmed), (packets.len(), len));
            prop_assert_eq!(usize::from(reference), packets.len());
            let mut offset = 0;
            for (index, packet) in packets.iter().enumerate() {
                let n = usize::from(u16::from_be_bytes([packet[23], packet[24]]));
                prop_assert_eq!(n, (len - offset).min(limit));
                prop_assert_eq!(&packet[11..13], &((index + 1) as u16).to_be_bytes());
                prop_assert_eq!(&packet[28..31], &((start + offset as u32) * 8).to_be_bytes()[1..]);
                if writing {
                    prop_assert_eq!(&packet[15..17], &((n + 4) as u16).to_be_bytes());
                    prop_assert_eq!(&packet[35..], &data[offset..offset + n]);
                }
                offset += n;
            }
            prop_assert_eq!(offset, len);
        }
    }
}
