//! `AsyncS7Client` against a mock PLC on a tokio loopback listener.
use super::{assert_job_time, assert_kind, contexts, reply, SLOW};
use rust7::{kind, AsyncS7Client, S7_AREA_DB, S7_WL_BIT, S7_WL_BYTE};
use std::io;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// What the mock PLC does with one request; requests past the script are `Normal`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Normal,
    /// The response is sent in small TCP segments with pauses between them.
    Segmented,
    /// The response is sent after `SLOW`.
    Slow,
    /// Item return code 10 (object does not exist).
    NotFound,
    /// Part of the response, then silence until the client closes the socket.
    Stall,
    /// Part of the response, then the mock closes the socket.
    Eof,
}

/// What the mock PLC saw.
#[derive(Debug)]
struct Served {
    connection_request: Vec<u8>,
    requests: Vec<Vec<u8>>,
    /// The client closed the socket (the mock read end of stream).
    closed_by_client: bool,
}

async fn frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let size = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let mut packet = vec![0; size];
    packet[..4].copy_from_slice(&header);
    stream.read_exact(&mut packet[4..]).await?;
    Ok(packet)
}

async fn wait_for_close(stream: &mut TcpStream) -> bool {
    let mut byte = [0; 1];
    matches!(
        tokio::time::timeout(Duration::from_secs(3), stream.read(&mut byte)).await,
        Ok(Ok(0)) | Ok(Err(_))
    )
}

/// A PLC with 64 KiB of DB memory, negotiating `pdu` bytes; the connection
/// confirm is sent after `handshake_delay`.
async fn serve(
    mut stream: TcpStream,
    pdu: u16,
    script: Vec<Step>,
    handshake_delay: Duration,
) -> Served {
    stream.set_nodelay(true).unwrap();
    let cr = frame(&mut stream).await.unwrap();
    assert_eq!(cr[5], 0xe0);
    let cc = [
        3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
    ];
    tokio::time::sleep(handshake_delay).await;
    stream.write_all(&cc).await.unwrap();
    let pn = frame(&mut stream).await.unwrap();
    let mut response = vec![
        3, 0, 0, 27, 2, 0xf0, 0x80, 0x32, 3, 0, 0, pn[11], pn[12], 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1,
        0, 1, 0, 0,
    ];
    response[25..27].copy_from_slice(&pdu.to_be_bytes());
    stream.write_all(&response).await.unwrap();

    let mut memory = vec![0u8; 0x10000];
    let mut requests = Vec::new();
    loop {
        let request = match frame(&mut stream).await {
            Ok(request) => request,
            Err(_) => {
                return Served {
                    connection_request: cr,
                    requests,
                    closed_by_client: true,
                }
            }
        };
        let step = script.get(requests.len()).copied().unwrap_or(Step::Normal);
        let function = request[17];
        let bit = request[22] == S7_WL_BIT;
        let count = usize::from(u16::from_be_bytes([request[23], request[24]]));
        let address = u32::from_be_bytes([0, request[28], request[29], request[30]]) as usize;
        let data = match (step, function) {
            (Step::NotFound, 4) => vec![10, 0, 0, 0],
            (Step::NotFound, _) => vec![10],
            (_, 4) if bit => {
                let value = (memory[address / 8] >> (address % 8)) & 1;
                vec![0xff, 3, 0, 1, value]
            }
            (_, 4) => {
                let mut data = vec![0xff, 4];
                data.extend_from_slice(&((count * 8) as u16).to_be_bytes());
                data.extend_from_slice(&memory[address / 8..address / 8 + count]);
                data
            }
            (_, _) if bit => {
                let mask = 1 << (address % 8);
                if request[35] != 0 {
                    memory[address / 8] |= mask;
                } else {
                    memory[address / 8] &= !mask;
                }
                vec![0xff]
            }
            (_, _) => {
                memory[address / 8..address / 8 + count].copy_from_slice(&request[35..]);
                vec![0xff]
            }
        };
        let response = reply(&request[11..13], function, &data);
        requests.push(request);
        match step {
            Step::Normal | Step::NotFound => stream.write_all(&response).await.unwrap(),
            Step::Slow => {
                tokio::time::sleep(SLOW).await;
                stream.write_all(&response).await.unwrap();
            }
            Step::Segmented => {
                for piece in response.chunks(3) {
                    stream.write_all(piece).await.unwrap();
                    stream.flush().await.unwrap();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Step::Stall => {
                stream.write_all(&response[..9]).await.unwrap();
                let closed_by_client = wait_for_close(&mut stream).await;
                return Served {
                    connection_request: cr,
                    requests,
                    closed_by_client,
                };
            }
            Step::Eof => {
                stream.write_all(&response[..9]).await.unwrap();
                return Served {
                    connection_request: cr,
                    requests,
                    closed_by_client: false,
                };
            }
        }
    }
}

/// A mock PLC on a fresh loopback port, serving one connection.
async fn listen(
    pdu: u16,
    script: Vec<Step>,
    handshake_delay: Duration,
) -> (u16, JoinHandle<Served>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback sockets required");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve(stream, pdu, script, handshake_delay).await
    });
    (port, server)
}

/// A client connected to a fresh mock PLC.
async fn connected(
    pdu: u16,
    rd_timeout_ms: u64,
    script: Vec<Step>,
) -> (AsyncS7Client, JoinHandle<Served>) {
    let (port, server) = listen(pdu, script, Duration::ZERO).await;
    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    client.set_timeout(1000, rd_timeout_ms, 1000).unwrap();
    client.connect_s71200_1500("127.0.0.1").await.unwrap();
    assert!(client.connected);
    assert_eq!(client.pdu_length, pdu);
    (client, server)
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_reads_and_writes_span_several_pdus() {
    // PDU 60: 42 bytes per read chunk, 32 bytes per write chunk.
    let (mut client, server) = connected(60, 1000, vec![]).await;
    let written: Vec<u8> = (0..100).map(|i| i as u8 ^ 0x5a).collect();
    client.write_db(1, 5, &written).await.unwrap();
    assert_eq!((client.chunks, client.confirmed), (4, 100));
    assert!(client.last_time > 0.0);

    let mut read = vec![0; 100];
    client.read_db(1, 5, &mut read).await.unwrap();
    assert_eq!(read, written);
    assert_eq!((client.chunks, client.confirmed), (3, 100));
    assert!(client.last_time > 0.0);

    client.write_bit(S7_AREA_DB, 1, 200, 3, true).await.unwrap();
    assert!(client.read_bit(S7_AREA_DB, 1, 200, 3).await.unwrap());
    assert!(!client.read_bit(S7_AREA_DB, 1, 200, 2).await.unwrap());
    assert!(client.connected);

    client.disconnect();
    let served = server.await.unwrap();
    assert!(served.closed_by_client);
    let counts: Vec<u16> = served
        .requests
        .iter()
        .map(|r| u16::from_be_bytes([r[23], r[24]]))
        .collect();
    assert_eq!(counts, [32, 32, 32, 4, 42, 42, 16, 1, 1, 1]);
    let references: Vec<u16> = served
        .requests
        .iter()
        .map(|r| u16::from_be_bytes([r[11], r[12]]))
        .collect();
    assert_eq!(references, (1..=10).collect::<Vec<u16>>());
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_response_in_several_tcp_segments_is_assembled() {
    let (mut client, server) = connected(240, 1000, vec![Step::Segmented; 2]).await;
    client.write_db(3, 0, &[1, 2, 3, 4]).await.unwrap();
    let mut read = [0; 4];
    client.read_db(3, 0, &mut read).await.unwrap();
    assert_eq!(read, [1, 2, 3, 4]);
    assert!(client.connected);
    drop(client);
    assert!(server.await.unwrap().closed_by_client);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_frame_timeout_is_io_and_disconnects() {
    let (mut client, server) = connected(240, 200, vec![Step::Stall]).await;
    let result = client.read_db(1, 0, &mut [0; 8]).await;
    assert_kind(&result, kind::IO);
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("no complete response frame within 200 ms"),
        "{err}"
    );
    assert_eq!(contexts(&err), ["read_db db=1 start=0 len=8"]);
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
    assert_eq!(
        (client.chunks, client.confirmed, client.last_time),
        (1, 0, 0.0)
    );
    assert!(server.await.unwrap().closed_by_client);
    assert_kind(
        &client.read_db(1, 0, &mut [0; 8]).await,
        kind::NOT_CONNECTED,
    );
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_eof_inside_a_response_is_connection_closed() {
    let (mut client, server) = connected(240, 1000, vec![Step::Eof]).await;
    let result = client.write_db(1, 0, &[9; 4]).await;
    assert_kind(&result, kind::CONNECTION_CLOSED);
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
    server.await.unwrap();
    assert_kind(&client.write_db(1, 0, &[9; 4]).await, kind::NOT_CONNECTED);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_plc_error_keeps_the_session() {
    let (mut client, server) = connected(240, 1000, vec![Step::NotFound, Step::NotFound]).await;
    let result = client.read_db(99, 0, &mut [0; 4]).await;
    assert_kind(&result, kind::S7_NOT_FOUND);
    assert_eq!(
        contexts(&result.unwrap_err()),
        ["read_db db=99 start=0 len=4"]
    );
    assert_kind(&client.write_db(99, 0, &[1; 4]).await, kind::S7_NOT_FOUND);
    // An invalid area sends nothing and keeps the session too.
    assert_kind(
        &client.read_area(0x99, 1, 0, S7_WL_BYTE, &mut [0; 2]).await,
        kind::INVALID_FUN_PARAMETER,
    );
    assert_kind(
        &client.write_area(0x85, 1, 0, S7_WL_BYTE, &[0; 2]).await,
        kind::INVALID_FUN_PARAMETER,
    );
    assert!(client.connected);
    assert_eq!(client.pdu_length, 240);
    assert_eq!(client.last_time, 0.0);

    client.write_db(1, 0, &[1, 2]).await.unwrap();
    let mut read = [0; 2];
    client.read_db(1, 0, &mut read).await.unwrap();
    assert_eq!(read, [1, 2]);
    drop(client);
    assert_eq!(server.await.unwrap().requests.len(), 4);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_read_abandoned_mid_response_disconnects() {
    let (mut client, server) = connected(240, 5000, vec![Step::Stall]).await;
    let mut buffer = [0; 8];
    let abandoned = tokio::time::timeout(
        Duration::from_millis(100),
        client.read_db(1, 0, &mut buffer),
    )
    .await;
    assert!(abandoned.is_err(), "the read must still be waiting");
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
    assert_eq!(
        (client.chunks, client.confirmed, client.last_time),
        (1, 0, 0.0)
    );
    // The socket is closed, not only marked disconnected.
    let served = server.await.unwrap();
    assert!(served.closed_by_client);
    assert_eq!(served.requests.len(), 1);
    let next = client.read_db(1, 0, &mut buffer).await;
    assert_kind(&next, kind::NOT_CONNECTED);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_write_abandoned_mid_transfer_reports_confirmed_bytes() {
    // PDU 60 writes 32 bytes per chunk; the third response never completes.
    let (mut client, server) =
        connected(60, 5000, vec![Step::Normal, Step::Normal, Step::Stall]).await;
    let abandoned =
        tokio::time::timeout(Duration::from_millis(200), client.write_db(1, 0, &[7; 100])).await;
    assert!(abandoned.is_err());
    assert!(!client.connected);
    assert_eq!((client.chunks, client.confirmed), (3, 64));
    let served = server.await.unwrap();
    assert!(served.closed_by_client);
    assert_eq!(served.requests.len(), 3, "the write is not retried");
    assert_kind(&client.write_db(1, 0, &[7; 4]).await, kind::NOT_CONNECTED);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_connect_abandoned_during_handshake_leaves_client_disconnected() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback sockets required");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _cr = frame(&mut stream).await.unwrap();
        wait_for_close(&mut stream).await
    });
    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    let abandoned = tokio::time::timeout(
        Duration::from_millis(100),
        client.connect_s71200_1500("127.0.0.1"),
    )
    .await;
    assert!(abandoned.is_err());
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
    assert!(server.await.unwrap(), "the half-open connection is closed");
    assert_kind(
        &client.read_db(1, 0, &mut [0; 1]).await,
        kind::NOT_CONNECTED,
    );
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_connect_tsap_sends_its_tsaps() {
    let (port, server) = listen(240, vec![], Duration::ZERO).await;
    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    client
        .connect_tsap("127.0.0.1", 0x1000, 0x0301)
        .await
        .unwrap();
    assert!(client.connected);
    assert_eq!(client.pdu_length, 240);
    client.write_db(1, 0, &[1, 2, 3, 4]).await.unwrap();
    let mut read = [0; 4];
    client.read_db(1, 0, &mut read).await.unwrap();
    assert_eq!(read, [1, 2, 3, 4]);
    client.disconnect();
    let served = server.await.unwrap();
    assert_eq!(
        &served.connection_request[14..22],
        &[0xc1, 2, 0x10, 0x00, 0xc2, 2, 0x03, 0x01]
    );
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_last_time_reports_job_duration_in_milliseconds() {
    let (port, server) = listen(240, vec![Step::Slow; 2], SLOW).await;
    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    let start = Instant::now();
    client.connect_s71200_1500("127.0.0.1").await.unwrap();
    assert_job_time(client.last_time, start);
    let start = Instant::now();
    client.read_db(1, 0, &mut [0; 4]).await.unwrap();
    assert_job_time(client.last_time, start);
    let start = Instant::now();
    client.write_db(1, 0, &[1, 2, 3, 4]).await.unwrap();
    assert_job_time(client.last_time, start);
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_cleared_connected_flag_sends_no_request() {
    // Like `S7Client`, a session marked disconnected is not used.
    let (mut client, server) = connected(240, 1000, vec![]).await;
    client.connected = false;
    assert_kind(
        &client.read_db(1, 0, &mut [0; 4]).await,
        kind::NOT_CONNECTED,
    );
    assert_kind(&client.write_db(1, 0, &[1; 4]).await, kind::NOT_CONNECTED);
    assert_eq!(client.pdu_length, 240);
    // The session itself is kept, as in `S7Client`: restoring the flag uses it.
    client.connected = true;
    client.write_db(1, 0, &[1, 2, 3, 4]).await.unwrap();
    drop(client);
    let served = server.await.unwrap();
    assert_eq!(served.requests.len(), 1);
    assert!(served.closed_by_client);
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_failed_connect_closes_the_previous_session() {
    let (mut client, server) = connected(240, 1000, vec![]).await;
    let refused = super::closed_port();
    client.set_connection_port(refused).unwrap();
    let result = client.connect_s7300("127.0.0.1").await;
    assert_kind(&result, kind::IO);
    assert_eq!(
        contexts(&result.unwrap_err()),
        [format!("connect_s7300 ip=127.0.0.1 port={refused}")]
    );
    assert!(!client.connected);
    assert_eq!(client.pdu_length, 0);
    // The previous session is closed first: the mock sees end of stream.
    let served = server.await.unwrap();
    assert!(served.closed_by_client);
    assert!(served.requests.is_empty());
    assert_kind(
        &client.read_db(1, 0, &mut [0; 1]).await,
        kind::NOT_CONNECTED,
    );
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_refused_connection_is_io_with_context() {
    let port = super::closed_port();
    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    let result = client.connect_s7300("127.0.0.1").await;
    assert_kind(&result, kind::IO);
    assert_eq!(
        contexts(&result.unwrap_err()),
        [format!("connect_s7300 ip=127.0.0.1 port={port}")]
    );
    assert!(!client.connected);
}

/// How the mock PLC answers the handshake of a connect that must fail.
#[derive(Clone, Copy, Debug)]
enum Handshake {
    /// The mock closes the socket instead of sending the connection confirm.
    NoConfirm,
    /// A connection request comes back instead of a connection confirm.
    NotAConfirm,
    /// The CPU answers with one byte more than the offered PDU (480 bytes).
    OversizedPdu,
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_failed_handshake_reports_its_kind_and_closes_the_socket() {
    for (handshake, expected) in [
        (Handshake::NoConfirm, kind::CONNECTION_CLOSED),
        (Handshake::NotAConfirm, kind::ISO_CONNECTION_FAILED),
        (Handshake::OversizedPdu, kind::PDU_NEGOTIATION_FAILED),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback sockets required");
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _cr = frame(&mut stream).await.unwrap();
            let mut cc = [
                3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
            ];
            match handshake {
                Handshake::NoConfirm => return None,
                Handshake::NotAConfirm => {
                    cc[5] = 0xe0;
                    stream.write_all(&cc).await.unwrap();
                }
                Handshake::OversizedPdu => {
                    stream.write_all(&cc).await.unwrap();
                    let pn = frame(&mut stream).await.unwrap();
                    let mut response = vec![
                        3, 0, 0, 27, 2, 0xf0, 0x80, 0x32, 3, 0, 0, pn[11], pn[12], 0, 8, 0, 0, 0,
                        0, 0xf0, 0, 0, 1, 0, 1, 0, 0,
                    ];
                    response[25..27].copy_from_slice(&481u16.to_be_bytes());
                    stream.write_all(&response).await.unwrap();
                }
            }
            Some(wait_for_close(&mut stream).await)
        });
        let mut client = AsyncS7Client::new();
        client.set_connection_port(port).unwrap();
        client.set_timeout(1000, 1000, 1000).unwrap();
        let result = client.connect_tsap("127.0.0.1", 0x1000, 0x0301).await;
        assert_kind(&result, expected);
        assert_eq!(
            contexts(&result.unwrap_err()),
            [format!(
                "connect_tsap ip=127.0.0.1 port={port} local_tsap=0x1000 remote_tsap=0x0301"
            )],
            "{handshake:?}"
        );
        assert!(!client.connected, "{handshake:?}");
        assert_eq!(
            (client.pdu_length, client.last_time),
            (0, 0.0),
            "{handshake:?}"
        );
        if let Some(closed) = server.await.unwrap() {
            assert!(closed, "{handshake:?}: the half-open connection is closed");
        }
        assert_kind(
            &client.read_db(1, 0, &mut [0; 1]).await,
            kind::NOT_CONNECTED,
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn async_connect_not_established_within_the_connect_timeout_is_io() {
    // Linux drops a SYN while the accept queue is full; backlog 0 holds one
    // connection, so a second connect stays pending.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(0).expect("loopback sockets required");
    let port = listener.local_addr().unwrap().port();
    let _queued = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

    let mut client = AsyncS7Client::new();
    client.set_connection_port(port).unwrap();
    client.set_timeout(200, 1000, 1000).unwrap();
    let start = Instant::now();
    let result = client.connect_s7300("127.0.0.1").await;
    let elapsed = start.elapsed();
    assert_kind(&result, kind::IO);
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("TCP connection not established within 200 ms"),
        "{err}"
    );
    assert_eq!(
        contexts(&err),
        [format!("connect_s7300 ip=127.0.0.1 port={port}")]
    );
    assert!(
        (Duration::from_millis(200)..Duration::from_secs(1)).contains(&elapsed),
        "{elapsed:?}"
    );
    assert!(!client.connected);
    assert_eq!((client.pdu_length, client.last_time), (0, 0.0));
    drop(listener);
}
