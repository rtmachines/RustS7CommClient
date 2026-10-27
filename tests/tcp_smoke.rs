//! Run explicitly: cargo test --test tcp_smoke -- --ignored
use rust7::{S7Client, S7Error, S7_AREA_DB};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

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

fn serve(mut stream: TcpStream, bad_pdu: bool) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_nodelay(true)?;
    let cr = frame(&mut stream)?;
    assert_eq!(&cr[4..11], &[17, 0xe0, 0, 0, 0, 1, 0]);
    send_split(
        &mut stream,
        &[
            3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
        ],
    )?;
    let pn = frame(&mut stream)?;
    assert_eq!(&pn[17..], &[0xf0, 0, 0, 1, 0, 1, 1, 0xe0]);
    let size = if bad_pdu { 28u16 } else { 240u16 };
    let mut response = vec![
        3, 0, 0, 27, 2, 0xf0, 0x80, 0x32, 3, 0, 0, pn[11], pn[12], 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1,
        0, 1, 0, 0,
    ];
    response[25..27].copy_from_slice(&size.to_be_bytes());
    send_split(&mut stream, &response)?;
    if bad_pdu {
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
        let address = u32::from_be_bytes([0, request[28], request[29], request[30]]);
        if request[22] == 1 {
            assert_eq!(address, 0x07ffff);
            assert_eq!(count, 1);
        }
        let data = match (request[17], request[22]) {
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
        send_split(&mut stream, &reply(&request[11..13], request[17], &data))?;
    }
}

fn run(bad_pdu: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback sockets required");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    thread::scope(|scope| {
        let server = scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => return serve(stream, bad_pdu),
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
        let result = client.connect_s71200_1500("127.0.0.1");
        if bad_pdu {
            assert!(matches!(result, Err(S7Error::PduNegotiationFailed)));
            assert!(!client.connected);
            assert_eq!(client.pdu_length, 0);
        } else {
            result.unwrap();
            client.pdu_length = 65535; // Diagnostics cannot override negotiated limits.
            let mut read = vec![0xaa; 65536];
            client.read_db(100, 0, &mut read).unwrap();
            assert_eq!(
                read,
                (0..65536).map(|i| (i % 251) as u8).collect::<Vec<_>>()
            );
            assert!(client.chunks > 1);
            client.write_db(100, 0, &read).unwrap();
            assert!(client.chunks > 1);
            assert!(client.read_bit(S7_AREA_DB, 100, 65535, 7).unwrap());
            client.write_bit(S7_AREA_DB, 100, 65535, 7, true).unwrap();
        }
        client.disconnect();
        server.join().unwrap().unwrap();
    });
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn tcp_smoke() {
    run(false);
}

#[test]
#[ignore = "requires permission to open loopback sockets"]
fn failed_negotiation_keeps_client_disconnected() {
    run(true);
}
