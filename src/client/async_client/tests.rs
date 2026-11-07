use super::*;

const MS: fn(u64) -> Duration = Duration::from_millis;

#[test]
fn a_connect_uses_the_configured_timeouts() {
    let mut client = AsyncS7Client::new();
    client.set_timeout(1, 2, 3).unwrap();
    let t = client.connect_timeouts();
    assert_eq!((t.read, t.write), (MS(2), MS(3)));
}

#[test]
fn set_timeout_keeps_the_write_timeout_of_the_open_session() {
    // Like `S7Client`, which sets the socket send timeout only when connecting,
    // the write timeout is fixed at connect; the read timeout applies at once.
    let mut client = AsyncS7Client::new();
    client.set_timeout(1, 2, 3).unwrap();
    client.session_wr_timeout = client.connect_timeouts().write; // recorded by a connect
    client.set_timeout(7, 8, 9).unwrap();
    let t = client.session_timeouts();
    assert_eq!((t.read, t.write), (MS(8), MS(3)));
    assert_eq!(client.connect_timeouts().write, MS(9));
}

#[test]
fn a_new_client_has_the_default_write_timeout() {
    let client = AsyncS7Client::new();
    assert_eq!(
        client.session_timeouts().write,
        client.connect_timeouts().write
    );
    assert_eq!(client.connect_timeouts().write, MS(500));
}

/// Accepts one connection and completes the ISO and PDU handshakes, then
/// waits until the client closes the socket.
async fn handshake_only(listener: tokio::net::TcpListener) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut cr = [0; 22];
    stream.read_exact(&mut cr).await.unwrap();
    let cc = [
        3, 0, 0, 22, 17, 0xd0, 0, 1, 0, 1, 0, 0xc0, 1, 10, 0xc1, 2, 1, 0, 0xc2, 2, 1, 0,
    ];
    stream.write_all(&cc).await.unwrap();
    let mut pn = [0; 25];
    stream.read_exact(&mut pn).await.unwrap();
    let ack = [
        3, 0, 0, 27, 2, 0xf0, 0x80, 0x32, 3, 0, 0, pn[11], pn[12], 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1,
        0, 1, 0, 240,
    ];
    stream.write_all(&ack).await.unwrap();
    let _ = stream.read(&mut [0; 1]).await;
}

#[tokio::test]
#[ignore = "requires permission to open loopback sockets"]
async fn connect_records_the_write_timeout_of_the_session() {
    let mut client = AsyncS7Client::new();
    for wr_timeout_ms in [1234, 99] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback sockets required");
        client
            .set_connection_port(listener.local_addr().unwrap().port())
            .unwrap();
        let server = tokio::spawn(handshake_only(listener));
        client.set_timeout(1000, 1000, wr_timeout_ms).unwrap();
        client.connect_s71200_1500("127.0.0.1").await.unwrap();
        assert_eq!(client.session_timeouts().write, MS(wr_timeout_ms));
        // A later setting waits for the next connect.
        client.set_timeout(1000, 1000, 7).unwrap();
        assert_eq!(client.session_timeouts().write, MS(wr_timeout_ms));
        client.disconnect();
        server.await.unwrap();
    }
}
