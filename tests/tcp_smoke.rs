//! Run explicitly: cargo test --test tcp_smoke -- --ignored
use rust7::{
    kind, S7Client, CT_OP, CT_S7, S7_AREA_DB, S7_AREA_MK, S7_AREA_PA, S7_AREA_PE, S7_WL_BIT,
    S7_WL_BYTE,
};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

#[track_caller]
fn assert_kind<T>(result: &rust7::Result<T>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error of kind {expected}, got Ok"),
        Err(e) if e.get_type() == Some(expected) => {}
        Err(e) => panic!(
            "expected error of kind {expected}, got {:?}: {e}",
            e.get_type()
        ),
    }
}

fn contexts(e: &rust7::AnyError) -> Vec<String> {
    let value = serde_json::to_value(e).unwrap();
    value["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect()
}

/// A loopback port with no listener, so connecting is refused.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback sockets required");
    listener.local_addr().unwrap().port()
}

fn frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let size = usize::from(u16::from_be_bytes([header[2], header[3]]));
    assert!((4..=487).contains(&size));
    let mut packet = vec![0; size];
    packet[..4].copy_from_slice(&header);
    stream.read_exact(&mut packet[4..])?;
    Ok(packet)
}

fn send_split(stream: &mut TcpStream, packet: &[u8]) -> io::Result<()> {
    stream.write_all(&packet[..3])?;
    stream.write_all(&packet[3..7])?;
    stream.write_all(&packet[7..])
}

fn reply(reference: &[u8], function: u8, data: &[u8]) -> Vec<u8> {
    let mut packet = vec![
        3,
        0,
        0,
        0,
        2,
        0xf0,
        0x80,
        0x32,
        3,
        0,
        0,
        reference[0],
        reference[1],
        0,
        2,
        0,
        0,
        0,
        0,
        function,
        1,
    ];
    packet[15..17].copy_from_slice(&(data.len() as u16).to_be_bytes());
    packet.extend_from_slice(data);
    let size = packet.len() as u16;
    packet[2..4].copy_from_slice(&size.to_be_bytes());
    packet
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Normal,
    BadPdu,
    SmallTpdu(u8),
    OversizedPdu(u8),
    BadCotp,
    LateReply(bool),
    PlcError,
    AckError(bool),
    MalformedAck(bool),
    BadHeader(bool),
    Fragment(bool),
    MidFrameClose,
    S7300,
    Tsap,
    /// Connection type, rack, slot and the remote TSAP the client must send.
    RackSlot(u16, u16, u16, u16),
    NoConfirm,
    FailedReconnect,
    Dropped,
    Slow,
    SlowDrip,
    Addressing,
}

/// Server delay in the `Slow` scenario, before the connection confirm and every reply.
const SLOW: Duration = Duration::from_millis(50);

/// `last_time` is in milliseconds and spans the job, including the server's delay.
#[track_caller]
fn assert_job_time(last_time: f64, start: Instant) {
    let measured = start.elapsed().as_secs_f64() * 1000.0;
    assert!(
        last_time >= SLOW.as_secs_f64() * 1000.0 && last_time <= measured,
        "last_time {last_time} ms, measured {measured} ms"
    );
}

/// One read or write request as the mock PLC decoded it.
#[derive(Debug, PartialEq)]
struct Seen {
    function: u8,
    wordlen: u8,
    db: u16,
    area: u8,
    address: u32,
    payload: Vec<u8>,
}

fn serve(stream: TcpStream, scenario: Scenario) -> io::Result<Vec<Seen>> {
    let mut seen = Vec::new();
    serve_requests(stream, scenario, &mut seen)?;
    Ok(seen)
}

fn serve_requests(
    mut stream: TcpStream,
    scenario: Scenario,
    seen: &mut Vec<Seen>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_nodelay(true)?;
    let cr = frame(&mut stream)?;
    assert_eq!(&cr[4..11], &[17, 0xe0, 0, 0, 0, 1, 0]);
    assert_eq!(&cr[11..14], &[0xc0, 1, 10]);
    let [local_hi, local_lo, remote_hi, remote_lo] = match scenario {
        Scenario::Tsap => [0x10, 0x00, 0x03, 0x01],
        Scenario::S7300 => [1, 0, 1, 2],
        Scenario::RackSlot(_, _, _, tsap) => {
            let [hi, lo] = tsap.to_be_bytes();
            [1, 0, hi, lo]
        }
        _ => [1, 0, 1, 0],
    };
    assert_eq!(
        &cr[14..22],
        &[0xc1, 2, local_hi, local_lo, 0xc2, 2, remote_hi, remote_lo]
    );
    let exponent = match scenario {
        Scenario::SmallTpdu(exponent) | Scenario::OversizedPdu(exponent) => exponent,
        Scenario::BadCotp => 11,
        _ => 10,
    };
    let tpdu_limit = 1usize << exponent;
    if scenario == Scenario::NoConfirm {
        return Ok(()); // Close instead of confirming the ISO connection.
    }
    let cc = [
        3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, exponent, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
    ];
    if scenario == Scenario::Slow {
        thread::sleep(SLOW);
    }
    send_split(&mut stream, &cc)?;
    if scenario == Scenario::BadCotp {
        assert!(matches!(frame(&mut stream), Err(e) if e.kind() == io::ErrorKind::UnexpectedEof));
        return Ok(());
    }
    let pn = frame(&mut stream)?;
    assert_eq!(&pn[17..23], &[0xf0, 0, 0, 1, 0, 1]);
    let offered = match exponent {
        7 => 125u16,
        8 => 253,
        _ => 480,
    };
    assert_eq!(&pn[23..25], &offered.to_be_bytes());
    let size = match scenario {
        Scenario::BadPdu => 28u16,
        Scenario::OversizedPdu(_) => offered + 1,
        Scenario::SmallTpdu(_) => offered,
        _ => 240,
    };
    let mut response = vec![
        3, 0, 0, 27, 2, 0xf0, 0x80, 0x32, 3, 0, 0, pn[11], pn[12], 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1,
        0, 1, 0, 0,
    ];
    response[25..27].copy_from_slice(&size.to_be_bytes());
    send_split(&mut stream, &response)?;
    if matches!(scenario, Scenario::BadPdu | Scenario::OversizedPdu(_)) {
        assert!(matches!(frame(&mut stream), Err(e) if e.kind() == io::ErrorKind::UnexpectedEof));
        return Ok(());
    }
    let mut written = Vec::new();
    let mut reference = 0u16;
    loop {
        let request = match frame(&mut stream) {
            Ok(packet) => packet,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        reference = reference.wrapping_add(1);
        assert_eq!(&request[11..13], &reference.to_be_bytes());
        let count = usize::from(u16::from_be_bytes([request[23], request[24]]));
        assert_eq!(&request[4..11], &[2, 0xf0, 0x80, 0x32, 1, 0, 0]);
        assert_eq!(&request[13..15], &[0, 14]);
        assert_eq!(&request[18..22], &[1, 0x12, 0x0a, 0x10]);
        if request[17] == 4 {
            assert_eq!(&request[15..17], &[0, 0]);
        } else {
            let bit = request[22] == 1;
            assert_eq!(&request[15..17], &((count + 4) as u16).to_be_bytes());
            assert_eq!(request[31], 0);
            assert_eq!(request[32], if bit { 3 } else { 4 });
            let bits = if bit { 1 } else { count * 8 } as u16;
            assert_eq!(&request[33..35], &bits.to_be_bytes());
        }
        assert!(request.len() - 4 <= tpdu_limit);
        if request[17] == 4 {
            assert!(3 + 18 + count <= tpdu_limit);
        }
        let address = u32::from_be_bytes([0, request[28], request[29], request[30]]);
        if request[22] == 1 {
            if scenario != Scenario::Addressing {
                assert_eq!(address, 0x07ffff);
            }
            assert_eq!(count, 1);
        }
        let first = reference == 1;
        if scenario == Scenario::MidFrameClose {
            // Declare a longer frame, send only 3 more bytes and close the socket.
            stream.write_all(&[3, 0, 0, 30, 2, 0xf0, 0x80])?;
            stream.shutdown(Shutdown::Both)?;
            return Ok(());
        }
        if first && matches!(scenario, Scenario::AckError(_) | Scenario::MalformedAck(_)) {
            let writing = matches!(
                scenario,
                Scenario::AckError(true) | Scenario::MalformedAck(true)
            );
            assert_eq!(request[17], if writing { 5 } else { 4 });
            let mut response = vec![
                3,
                0,
                0,
                19,
                2,
                0xf0,
                0x80,
                0x32,
                2,
                0,
                0,
                request[11],
                request[12],
                0,
                0,
                0,
                0,
                0x81,
                4,
            ];
            if matches!(scenario, Scenario::MalformedAck(_)) {
                response[12] ^= 1;
            }
            send_split(&mut stream, &response)?;
            continue;
        }
        if scenario == Scenario::SlowDrip {
            // One byte arrives late but before the deadline, then nothing more.
            let response = reply(&request[11..13], 4, &[0xff, 4, 0, 32, 0, 0, 0, 0]);
            stream.write_all(&response[..7])?;
            thread::sleep(Duration::from_millis(400));
            stream.write_all(&response[7..8])?;
            let _ = stream.read(&mut [0; 1]); // Wait until the client gives up.
            return Ok(());
        }
        if matches!(scenario, Scenario::LateReply(_)) {
            // Reply after the client's read timeout; the client is gone by then.
            let data: &[u8] = if request[17] == 5 {
                assert_eq!(&request[35..], &[1, 2, 3, 4]);
                &[0xff]
            } else {
                &[0xff, 4, 0, 32, 0, 0, 0, 0]
            };
            thread::sleep(Duration::from_millis(400));
            let _ = send_split(&mut stream, &reply(&request[11..13], request[17], data));
            return Ok(());
        }
        seen.push(Seen {
            function: request[17],
            wordlen: request[22],
            db: u16::from_be_bytes([request[25], request[26]]),
            area: request[27],
            address,
            payload: if request[17] == 5 {
                request[35..].to_vec()
            } else {
                Vec::new()
            },
        });
        let data = match (request[17], request[22]) {
            (4, 2) if first && scenario == Scenario::PlcError => vec![5, 0, 0, 0],
            (4, 2) => {
                let offset = address as usize / 8;
                let mut data = vec![0xff, 4, ((count * 8) >> 8) as u8, (count * 8) as u8];
                data.extend((offset..offset + count).map(|i| (i % 251) as u8));
                data
            }
            (5, 2) => {
                if scenario != Scenario::Addressing {
                    assert_eq!(address as usize / 8, written.len());
                }
                written.extend_from_slice(&request[35..]);
                assert_eq!(written.len().min(65536), written.len());
                if written.len() == 65536 {
                    assert_eq!(
                        written,
                        (0..65536).map(|i| (i % 251) as u8).collect::<Vec<_>>()
                    );
                }
                vec![0xff]
            }
            (4, 1) => vec![0xff, 3, 0, 1, (address % 2) as u8],
            (5, 1) => {
                if scenario != Scenario::Addressing {
                    assert_eq!(&request[35..], &[1]);
                }
                vec![0xff]
            }
            _ => panic!("unexpected request"),
        };
        if scenario == Scenario::Slow {
            thread::sleep(SLOW);
        }
        let mut response = reply(&request[11..13], request[17], &data);
        assert!(response.len() - 4 <= tpdu_limit);
        if first {
            match scenario {
                Scenario::BadHeader(_) => response[0] = 2,
                Scenario::Fragment(_) => response[6] = 0,
                _ => {}
            }
        }
        send_split(&mut stream, &response)?;
    }
}

fn run(scenario: Scenario) -> Vec<Seen> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback sockets required");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    thread::scope(|scope| {
        let server = scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => return serve(stream, scenario),
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => return Err(e),
                }
            }
        });
        let mut client = S7Client::new();
        client.set_connection_port(port).unwrap();
        client.set_timeout(1000, 1000, 1000).unwrap();
        if matches!(scenario, Scenario::LateReply(_)) {
            client.set_timeout(1000, 200, 1000).unwrap();
        }
        if scenario == Scenario::SlowDrip {
            client.set_timeout(1000, 500, 1000).unwrap();
        }
        let start = Instant::now();
        let result = match scenario {
            Scenario::S7300 => client.connect_s7300("127.0.0.1"),
            Scenario::Tsap => client.connect_tsap("127.0.0.1", 0x1000, 0x0301),
            Scenario::RackSlot(connection_type, rack, slot, _) => {
                client.set_connection_type(connection_type).unwrap();
                client.connect_rack_slot("127.0.0.1", rack, slot)
            }
            _ => client.connect_s71200_1500("127.0.0.1"),
        };
        if matches!(
            scenario,
            Scenario::BadPdu | Scenario::OversizedPdu(_) | Scenario::BadCotp
        ) {
            if scenario == Scenario::BadCotp {
                assert_kind(&result, kind::ISO_CONNECTION_FAILED);
            } else {
                assert_kind(&result, kind::PDU_NEGOTIATION_FAILED);
            }
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
            assert_kind(&client.read_db(100, 0, &mut [0]), kind::NOT_CONNECTED);
        } else if let Scenario::AckError(writing)
        | Scenario::MalformedAck(writing)
        | Scenario::BadHeader(writing)
        | Scenario::Fragment(writing) = scenario
        {
            result.unwrap();
            let mut buffer = [0xaa; 600];
            let result = if writing {
                client.write_db(100, 0, &buffer)
            } else {
                client.read_db(100, 0, &mut buffer)
            };
            assert_eq!(buffer, [0xaa; 600]);
            assert_eq!(client.chunks, 1);
            assert_eq!(client.confirmed, 0);
            assert_eq!(client.last_time, 0.0);
            if matches!(
                scenario,
                Scenario::MalformedAck(_) | Scenario::BadHeader(_) | Scenario::Fragment(_)
            ) {
                let expected = match scenario {
                    Scenario::BadHeader(_) => kind::ISO_INVALID_HEADER,
                    Scenario::Fragment(_) => kind::ISO_FRAGMENTED_PACKET,
                    _ => kind::ISO_INVALID_TELEGRAM,
                };
                assert_kind(&result, expected);
                assert!(!client.connected);
                assert_eq!(client.pdu_length, 0);
                assert_kind(&client.write_db(100, 0, &[1]), kind::NOT_CONNECTED);
            } else {
                assert_kind(&result, kind::S7_UNSPECIFIED);
                assert!(client.connected);
                assert_eq!(client.pdu_length, 240);
                let mut next = [0; 4];
                client.read_db(100, 10, &mut next).unwrap();
                assert_eq!(next, [10, 11, 12, 13]);
                client.write_db(100, 0, &next).unwrap();
            }
        } else if let Scenario::LateReply(writing) = scenario {
            result.unwrap();
            let mut buffer = [0; 4];
            let result = if writing {
                client.write_db(100, 0, &[1, 2, 3, 4])
            } else {
                client.read_db(100, 0, &mut buffer)
            };
            assert_kind(&result, kind::IO);
            let message = result.unwrap_err().to_string();
            assert!(
                message.contains("no complete response frame within 200 ms"),
                "{message}"
            );
            assert_eq!(client.chunks, 1);
            assert_eq!(client.confirmed, 0);
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
            assert_kind(&client.read_db(100, 0, &mut buffer), kind::NOT_CONNECTED);
        } else if scenario == Scenario::MidFrameClose {
            result.unwrap();
            assert_kind(
                &client.read_db(100, 0, &mut [0; 4]),
                kind::CONNECTION_CLOSED,
            );
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
            assert_kind(&client.read_db(100, 0, &mut [0; 4]), kind::NOT_CONNECTED);
        } else if scenario == Scenario::NoConfirm {
            assert_kind(&result, kind::CONNECTION_CLOSED);
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
            assert_kind(&client.read_db(100, 0, &mut [0]), kind::NOT_CONNECTED);
        } else if scenario == Scenario::FailedReconnect {
            result.unwrap();
            let refused = closed_port();
            client.set_connection_port(refused).unwrap();
            let result = client.connect_s7300("127.0.0.1");
            assert_kind(&result, kind::IO);
            assert_eq!(
                contexts(&result.unwrap_err()),
                [format!("connect_s7300 ip=127.0.0.1 port={refused}")]
            );
            // The previous session is closed first: the server sees end of stream.
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
            assert_kind(&client.read_db(100, 0, &mut [0]), kind::NOT_CONNECTED);
        } else if scenario == Scenario::Dropped {
            result.unwrap();
            drop(client);
            // The server returns only once it reads end of stream (2 s read timeout otherwise).
            return server.join().unwrap().unwrap();
        } else if matches!(
            scenario,
            Scenario::S7300 | Scenario::Tsap | Scenario::RackSlot(..)
        ) {
            result.unwrap();
            assert!(client.connected);
            assert_eq!(client.pdu_length, 240);
            let mut buffer = [0; 4];
            client.read_db(100, 0, &mut buffer).unwrap();
            assert_eq!(buffer, [0, 1, 2, 3]);
        } else if scenario == Scenario::Slow {
            result.unwrap();
            assert_job_time(client.last_time, start);
            let start = Instant::now();
            client.read_db(100, 0, &mut [0; 4]).unwrap();
            assert_job_time(client.last_time, start);
            let start = Instant::now();
            client.write_db(100, 0, &[1, 2, 3, 4]).unwrap();
            assert_job_time(client.last_time, start);
        } else if scenario == Scenario::SlowDrip {
            result.unwrap();
            let start = Instant::now();
            let result = client.read_db(100, 0, &mut [0; 4]);
            let elapsed = start.elapsed();
            assert_kind(&result, kind::IO);
            // The late byte must not restart the full read timeout.
            assert!(elapsed < Duration::from_millis(700), "{elapsed:?}");
            assert!(!client.connected);
        } else if scenario == Scenario::Addressing {
            result.unwrap();
            client.write_db(7, 3, &[0xde, 0xad]).unwrap();
            client.read_db(11, 4, &mut [0; 2]).unwrap();
            for area in [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB] {
                client.write_area(area, 9, 5, S7_WL_BYTE, &[area]).unwrap();
                client.read_area(area, 9, 5, S7_WL_BYTE, &mut [0]).unwrap();
            }
            client.write_bit(S7_AREA_MK, 9, 10, 0, true).unwrap();
            client.write_bit(S7_AREA_DB, 12, 10, 3, false).unwrap();
            assert!(!client.read_bit(S7_AREA_MK, 9, 10, 0).unwrap());
            assert!(client.read_bit(S7_AREA_PE, 9, 10, 3).unwrap());
            client
                .write_area(S7_AREA_MK, 9, 83, S7_WL_BIT, &[5, 0])
                .unwrap();
            let mut buffer = [0xaa, 0xbb];
            client
                .read_area(S7_AREA_PE, 9, 83, S7_WL_BIT, &mut buffer)
                .unwrap();
            assert_eq!(buffer, [1, 0xbb]);
        } else if scenario == Scenario::PlcError {
            result.unwrap();
            let mut buffer = [0; 4];
            assert_kind(
                &client.read_db(100, 0, &mut buffer),
                kind::S7_INVALID_ADDRESS,
            );
            assert!(client.connected);
            client.read_db(100, 0, &mut buffer).unwrap();
            assert_eq!(buffer, [0, 1, 2, 3]);
        } else {
            result.unwrap();
            assert_eq!(
                client.pdu_length,
                match scenario {
                    Scenario::SmallTpdu(7) => 125,
                    Scenario::SmallTpdu(8) => 253,
                    _ => 240,
                }
            );
            assert_kind(
                &client.connect_rack_slot("127.0.0.1", 8, 0),
                kind::INVALID_FUN_PARAMETER,
            );
            assert!(client.connected); // The existing connection is kept.
            client.pdu_length = 65535; // Diagnostics cannot override negotiated limits.
            let mut read = vec![0xaa; 65536];
            client.read_db(100, 0, &mut read).unwrap();
            assert_eq!(
                read,
                (0..65536).map(|i| (i % 251) as u8).collect::<Vec<_>>()
            );
            assert!(client.chunks > 1);
            assert_eq!(client.confirmed, 65536);
            client.write_db(100, 0, &read).unwrap();
            assert!(client.chunks > 1);
            assert_eq!(client.confirmed, 65536);
            assert!(client.read_bit(S7_AREA_DB, 100, 65535, 7).unwrap());
            assert_eq!(client.confirmed, 1);
            client.write_bit(S7_AREA_DB, 100, 65535, 7, true).unwrap();
        }
        client.disconnect();
        server.join().unwrap().unwrap()
    })
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn tcp_smoke() {
    run(Scenario::Normal);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn failed_negotiation_keeps_client_disconnected() {
    run(Scenario::BadPdu);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn late_reply_disconnects_client() {
    for writing in [false, true] {
        run(Scenario::LateReply(writing));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn plc_error_keeps_connection() {
    run(Scenario::PlcError);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn small_tpdu_bounds_reads_and_writes() {
    for exponent in [7, 8] {
        run(Scenario::SmallTpdu(exponent));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn oversized_negotiation_keeps_client_disconnected() {
    for exponent in [7, 8] {
        run(Scenario::OversizedPdu(exponent));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn invalid_cotp_limit_keeps_client_disconnected() {
    run(Scenario::BadCotp);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn terminal_error_ack_keeps_connection_for_next_read_and_write() {
    for writing in [false, true] {
        run(Scenario::AckError(writing));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn malformed_ack_disconnects_client() {
    for writing in [false, true] {
        run(Scenario::MalformedAck(writing));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn bad_tpkt_header_disconnects_client() {
    for writing in [false, true] {
        run(Scenario::BadHeader(writing));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn fragmented_reply_disconnects_client() {
    for writing in [false, true] {
        run(Scenario::Fragment(writing));
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn public_methods_send_their_addresses_and_bit_values() {
    let seen = |function, wordlen, db, area, address, payload: &[u8]| Seen {
        function,
        wordlen,
        db,
        area,
        address,
        payload: payload.to_vec(),
    };
    let mut expected = vec![
        seen(5, 2, 7, 0x84, 24, &[0xde, 0xad]),
        seen(4, 2, 11, 0x84, 32, &[]),
    ];
    for area in [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB] {
        let db = if area == S7_AREA_DB { 9 } else { 0 };
        expected.push(seen(5, 2, db, area, 40, &[area]));
        expected.push(seen(4, 2, db, area, 40, &[]));
    }
    expected.extend([
        seen(5, 1, 0, 0x83, 80, &[1]),
        seen(5, 1, 12, 0x84, 83, &[0]),
        seen(4, 1, 0, 0x83, 80, &[]),
        seen(4, 1, 0, 0x81, 83, &[]),
        seen(5, 1, 0, 0x83, 83, &[1]),
        seen(4, 1, 0, 0x81, 83, &[]),
    ]);
    assert_eq!(run(Scenario::Addressing), expected);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn mid_frame_close_disconnects_client() {
    run(Scenario::MidFrameClose);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn connect_helpers_send_their_tsaps() {
    for scenario in [Scenario::S7300, Scenario::Tsap] {
        run(scenario);
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn last_time_reports_job_duration_in_milliseconds() {
    run(Scenario::Slow);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn trickling_reply_is_bounded_by_the_frame_deadline() {
    run(Scenario::SlowDrip);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn connection_type_and_rack_slot_select_the_remote_tsap() {
    // Remote TSAP: connection type in the high byte, rack in bits 7..5, slot in bits 4..0.
    for scenario in [
        Scenario::RackSlot(CT_OP, 1, 3, 0x0223),
        Scenario::RackSlot(CT_S7, 7, 31, 0x03ff),
        Scenario::RackSlot(CT_S7, 0, 0, 0x0300),
    ] {
        run(scenario);
    }
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn close_before_connection_confirm_leaves_client_disconnected() {
    run(Scenario::NoConfirm);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn failed_connect_closes_the_previous_session() {
    run(Scenario::FailedReconnect);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn dropping_a_connected_client_closes_the_connection() {
    run(Scenario::Dropped);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn refused_connection_is_an_io_error_with_one_connect_context() {
    let port = closed_port();
    let mut client = S7Client::new();
    client.set_connection_port(port).unwrap();
    let ip = "127.0.0.1";
    let cases: [(rust7::Result<()>, String); 4] = [
        (
            client.connect_s71200_1500(ip),
            format!("connect_s71200_1500 ip={ip} port={port}"),
        ),
        (
            client.connect_s7300(ip),
            format!("connect_s7300 ip={ip} port={port}"),
        ),
        (
            client.connect_rack_slot(ip, 0, 1),
            format!("connect_rack_slot ip={ip} port={port} rack=0 slot=1"),
        ),
        (
            client.connect_tsap(ip, 0x0100, 0x0102),
            format!("connect_tsap ip={ip} port={port} local_tsap=0x0100 remote_tsap=0x0102"),
        ),
    ];
    for (result, context) in cases {
        assert_kind(&result, kind::IO);
        assert_eq!(contexts(&result.unwrap_err()), [context]);
    }
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
}
