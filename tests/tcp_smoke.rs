//! Run explicitly: cargo test --test tcp_smoke -- --ignored
use rust7::{kind, S7Client, S7_AREA_DB};
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
    MidFrameClose,
}

fn serve(mut stream: TcpStream, scenario: Scenario) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_nodelay(true)?;
    let cr = frame(&mut stream)?;
    assert_eq!(&cr[4..11], &[17, 0xe0, 0, 0, 0, 1, 0]);
    assert_eq!(&cr[11..14], &[0xc0, 1, 10]);
    let exponent = match scenario {
        Scenario::SmallTpdu(exponent) | Scenario::OversizedPdu(exponent) => exponent,
        Scenario::BadCotp => 11,
        _ => 10,
    };
    let tpdu_limit = 1usize << exponent;
    let cc = [
        3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, exponent, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
    ];
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
        assert!(request.len() - 4 <= tpdu_limit);
        let count = usize::from(u16::from_be_bytes([request[23], request[24]]));
        if request[17] == 4 {
            assert!(3 + 18 + count <= tpdu_limit);
        }
        let address = u32::from_be_bytes([0, request[28], request[29], request[30]]);
        if request[22] == 1 {
            assert_eq!(address, 0x07ffff);
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
        let data = match (request[17], request[22]) {
            (4, 2) if first && scenario == Scenario::PlcError => vec![5, 0, 0, 0],
            (4, 2) => {
                let offset = address as usize / 8;
                let mut data = vec![0xff, 4, ((count * 8) >> 8) as u8, (count * 8) as u8];
                data.extend((offset..offset + count).map(|i| (i % 251) as u8));
                data
            }
            (5, 2) => {
                assert_eq!(address as usize / 8, written.len());
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
            (4, 1) => vec![0xff, 3, 0, 1, 1],
            (5, 1) => {
                assert_eq!(&request[35..], &[1]);
                vec![0xff]
            }
            _ => panic!("unexpected request"),
        };
        let response = reply(&request[11..13], request[17], &data);
        assert!(response.len() - 4 <= tpdu_limit);
        send_split(&mut stream, &response)?;
    }
}

fn run(scenario: Scenario) {
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
        let result = client.connect_s71200_1500("127.0.0.1");
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
        } else if let Scenario::AckError(writing) | Scenario::MalformedAck(writing) = scenario {
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
            if matches!(scenario, Scenario::MalformedAck(_)) {
                assert_kind(&result, kind::ISO_INVALID_TELEGRAM);
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
        server.join().unwrap().unwrap();
    });
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
fn mid_frame_close_disconnects_client() {
    run(Scenario::MidFrameClose);
}
