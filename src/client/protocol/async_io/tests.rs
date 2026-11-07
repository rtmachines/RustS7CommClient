use super::*;
use crate::client::protocol::{self, FrameRead, Progress, Transfer};
use crate::client::{S7_AREA_DB, S7_AREA_MK, S7_WL_BIT, S7_WL_BYTE};
use crate::error::{assert_kind, kind};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf};

const SECOND: Duration = Duration::from_secs(1);
const TIMEOUTS: Timeouts = Timeouts {
    read: SECOND,
    write: SECOND,
};

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

/// How the scripted PLC answers each request.
#[derive(Clone, Copy)]
enum Plc {
    /// Read data is `address + i`; writes are acknowledged.
    Ok,
    /// The item return code of every request.
    Item(u8),
    /// A header-only error ACK.
    HeaderError,
}

/// The response of the scripted PLC to one read or write request.
fn respond(request: &[u8], plc: Plc) -> Vec<u8> {
    let function = request[17];
    let reference = [request[11], request[12]];
    let wordlen = request[22];
    let count = usize::from(u16::from_be_bytes([request[23], request[24]]));
    let address = u32::from_be_bytes([0, request[28], request[29], request[30]]);
    let header = |params: u16, data: u16, class: u8, code: u8| {
        let mut pdu = vec![0x32, 3, 0, 0, reference[0], reference[1]];
        pdu.extend_from_slice(&params.to_be_bytes());
        pdu.extend_from_slice(&data.to_be_bytes());
        pdu.extend_from_slice(&[class, code]);
        pdu
    };
    let status = match plc {
        Plc::HeaderError => {
            let mut pdu = header(0, 0, 0x81, 4);
            pdu[1] = 2;
            return data_frame(&pdu);
        }
        Plc::Item(code) => code,
        Plc::Ok => 0xff,
    };
    let data = if function == 5 {
        vec![status]
    } else if status != 0xff {
        vec![status, 0, 0, 0]
    } else {
        let (transport, bits) = if wordlen == S7_WL_BIT {
            (3, 1)
        } else {
            (4, count * 8)
        };
        let first = if wordlen == S7_WL_BIT {
            address
        } else {
            address / 8
        };
        let mut data = vec![status, transport];
        data.extend_from_slice(&(bits as u16).to_be_bytes());
        data.extend((0..count).map(|i| (first as usize + i) as u8));
        data
    };
    let mut pdu = header(2, data.len() as u16, 0, 0);
    pdu.extend_from_slice(&[function, 1]);
    pdu.extend_from_slice(&data);
    data_frame(&pdu)
}

/// Synchronous scripted PLC: answers each complete request written to it.
struct SyncPlc {
    plc: Plc,
    inbox: Vec<u8>,
    outbox: VecDeque<u8>,
    requests: Vec<Vec<u8>>,
}

impl SyncPlc {
    fn new(plc: Plc) -> Self {
        Self {
            plc,
            inbox: Vec::new(),
            outbox: VecDeque::new(),
            requests: Vec::new(),
        }
    }
}

impl Write for SyncPlc {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inbox.extend_from_slice(buf);
        while self.inbox.len() >= 4 {
            let len = usize::from(u16::from_be_bytes([self.inbox[2], self.inbox[3]]));
            if self.inbox.len() < len {
                break;
            }
            let request: Vec<u8> = self.inbox.drain(..len).collect();
            self.outbox.extend(respond(&request, self.plc));
            self.requests.push(request);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for SyncPlc {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.outbox.len());
        for (dst, src) in buf.iter_mut().zip(self.outbox.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }
}

impl FrameRead for SyncPlc {
    fn limit_next_read(&mut self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }
}

/// Async scripted PLC on the other end of a duplex; replies one byte per write.
/// Returns the requests it received once the client end is closed.
async fn async_plc(mut io: DuplexStream, plc: Plc) -> Vec<Vec<u8>> {
    let mut requests = Vec::new();
    loop {
        let mut header = [0; 4];
        if io.read_exact(&mut header).await.is_err() {
            return requests;
        }
        let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
        let mut request = header.to_vec();
        request.resize(len, 0);
        io.read_exact(&mut request[4..]).await.unwrap();
        for byte in respond(&request, plc) {
            io.write_all(&[byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        requests.push(request);
    }
}

struct Outcome {
    result: Result<()>,
    requests: Vec<Vec<u8>>,
    buffer: Vec<u8>,
    chunks: usize,
    confirmed: usize,
    reference: u16,
}

fn sync_transfer(
    plc: Plc,
    pdu: u16,
    transfer: Transfer,
    write: bool,
    mut buffer: Vec<u8>,
) -> Outcome {
    let limits = protocol::PduLimits::new(pdu).unwrap();
    let mut io = SyncPlc::new(plc);
    let mut reference = 0x7ffe;
    let mut progress = Progress::default();
    let result = if write {
        protocol::write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer,
            &buffer,
            &mut progress,
            SECOND,
        )
    } else {
        protocol::read_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer,
            &mut buffer,
            &mut progress,
            SECOND,
        )
    };
    Outcome {
        result,
        requests: io.requests,
        buffer,
        chunks: progress.chunks,
        confirmed: progress.confirmed,
        reference,
    }
}

async fn async_transfer(
    plc: Plc,
    pdu: u16,
    transfer: Transfer,
    write: bool,
    mut buffer: Vec<u8>,
) -> Outcome {
    let limits = protocol::PduLimits::new(pdu).unwrap();
    let (mut client, server) = duplex(64);
    let peer = tokio::spawn(async_plc(server, plc));
    let mut reference = 0x7ffe;
    let mut progress = Progress::default();
    let result = if write {
        write_transfer(
            &mut client,
            &limits,
            &mut reference,
            transfer,
            &buffer,
            &mut progress,
            TIMEOUTS,
        )
        .await
    } else {
        read_transfer(
            &mut client,
            &limits,
            &mut reference,
            transfer,
            &mut buffer,
            &mut progress,
            TIMEOUTS,
        )
        .await
    };
    drop(client);
    Outcome {
        result,
        requests: peer.await.unwrap(),
        buffer,
        chunks: progress.chunks,
        confirmed: progress.confirmed,
        reference,
    }
}

#[tokio::test]
async fn transfers_match_the_sync_implementation() {
    let cases = [
        (29, Transfer::db(7, 10), false, 40),
        (29, Transfer::db(7, 10), true, 5),
        (
            240,
            Transfer::area(S7_AREA_MK, 0, 3, S7_WL_BYTE),
            false,
            1000,
        ),
        (240, Transfer::area(S7_AREA_MK, 0, 3, S7_WL_BYTE), true, 700),
        (480, Transfer::bit(S7_AREA_DB, 9, 71, 4).unwrap(), false, 1),
        (480, Transfer::bit(S7_AREA_DB, 9, 71, 4).unwrap(), true, 1),
        (480, Transfer::db(1, 0), false, 0),
    ];
    for (pdu, transfer, write, len) in cases {
        let input: Vec<u8> = (0..len).map(|i| (i * 7) as u8 | 1).collect();
        let sync = sync_transfer(Plc::Ok, pdu, transfer, write, input.clone());
        let tokio = async_transfer(Plc::Ok, pdu, transfer, write, input).await;
        let case = format!("pdu={pdu} {transfer:?} write={write} len={len}");
        sync.result.unwrap();
        tokio.result.unwrap();
        assert_eq!(tokio.requests, sync.requests, "{case}");
        assert_eq!(tokio.buffer, sync.buffer, "{case}");
        assert_eq!(
            (tokio.chunks, tokio.confirmed, tokio.reference),
            (sync.chunks, sync.confirmed, sync.reference),
            "{case}"
        );
        assert_eq!(tokio.confirmed, len, "{case}");
        if len > 0 && pdu == 29 {
            assert!(tokio.chunks > 1, "{case} must span several PDUs");
        }
    }
}

#[tokio::test]
async fn plc_errors_match_the_sync_implementation() {
    for (plc, expected) in [
        (Plc::Item(10), kind::S7_NOT_FOUND),
        (Plc::Item(5), kind::S7_INVALID_ADDRESS),
        (Plc::Item(3), kind::S7_UNSPECIFIED),
        (Plc::HeaderError, kind::S7_UNSPECIFIED),
    ] {
        for write in [false, true] {
            let sync = sync_transfer(plc, 29, Transfer::db(7, 0), write, vec![1; 30]);
            let tokio = async_transfer(plc, 29, Transfer::db(7, 0), write, vec![1; 30]).await;
            assert_kind(&sync.result, expected);
            assert_kind(&tokio.result, expected);
            assert_eq!(
                tokio.result.unwrap_err().to_string(),
                sync.result.unwrap_err().to_string()
            );
            assert_eq!((tokio.chunks, tokio.confirmed), (1, 0));
            assert_eq!(tokio.requests, sync.requests);
        }
    }
}

#[tokio::test]
async fn read_tpkt_assembles_a_frame_from_single_bytes() {
    let frame = data_frame(&[0x32, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    let (mut client, mut server) = duplex(64);
    let sent = frame.clone();
    let peer = tokio::spawn(async move {
        for byte in sent {
            server.write_all(&[byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        server
    });
    assert_eq!(read_tpkt(&mut client, 487, SECOND).await.unwrap(), frame);
    drop(peer.await.unwrap());
}

#[tokio::test]
async fn read_tpkt_leaves_the_next_frame() {
    let a = tpkt(&[1, 2, 3]);
    let b = tpkt(&[4]);
    let (mut client, mut server) = duplex(64);
    server
        .write_all(&[a.clone(), b.clone()].concat())
        .await
        .unwrap();
    assert_eq!(read_tpkt(&mut client, 487, SECOND).await.unwrap(), a);
    assert_eq!(read_tpkt(&mut client, 487, SECOND).await.unwrap(), b);
}

#[tokio::test(start_paused = true)]
async fn read_timeout_bounds_the_whole_frame() {
    let frame = tpkt(&[0; 8]);
    let (mut client, mut server) = duplex(64);
    // Every byte arrives well within the timeout, the whole frame does not.
    let _peer = tokio::spawn(async move {
        for byte in frame {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if server.write_all(&[byte]).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
    });
    let start = tokio::time::Instant::now();
    let result = read_tpkt(&mut client, 487, SECOND).await;
    assert_kind(&result, kind::IO);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("no complete response frame within 1000 ms"),);
    assert_eq!(start.elapsed(), SECOND);
}

#[tokio::test(start_paused = true)]
async fn missing_frame_times_out_with_the_sync_message() {
    let (mut client, _server) = duplex(64);
    let result = read_tpkt(&mut client, 487, Duration::from_millis(250)).await;
    assert_eq!(
        result.unwrap_err().to_string(),
        protocol::timed_out(Duration::from_millis(250)).to_string()
    );
}

#[tokio::test]
async fn eof_inside_a_frame_is_connection_closed() {
    for sent in [vec![], vec![3, 0], tpkt(&[1, 2, 3])[..5].to_vec()] {
        let (mut client, mut server) = duplex(64);
        server.write_all(&sent).await.unwrap();
        drop(server);
        assert_kind(
            &read_tpkt(&mut client, 487, SECOND).await,
            kind::CONNECTION_CLOSED,
        );
    }
}

/// Reader that returns scripted pieces and errors, then end of stream.
struct ScriptedReads(VecDeque<io::Result<Vec<u8>>>);

impl AsyncRead for ScriptedReads {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.0.pop_front() {
            Some(Ok(mut bytes)) => {
                let n = bytes.len().min(buf.remaining());
                buf.put_slice(&bytes[..n]);
                if n < bytes.len() {
                    self.0.push_front(Ok(bytes.split_off(n)));
                }
                Poll::Ready(Ok(()))
            }
            Some(Err(e)) => Poll::Ready(Err(e)),
            None => Poll::Ready(Ok(())),
        }
    }
}

#[tokio::test]
async fn other_read_errors_are_io_errors_not_timeouts() {
    for io_kind in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
    ] {
        let mut reader =
            ScriptedReads(VecDeque::from([Ok(vec![3, 0, 0, 22]), Err(io_kind.into())]));
        let result = read_tpkt(&mut reader, 487, SECOND).await;
        assert_kind(&result, kind::IO);
        let message = result.unwrap_err().to_string();
        assert!(!message.contains("no complete response frame"), "{message}");
        assert!(
            message.contains(&io::Error::from(io_kind).to_string()),
            "{message}"
        );
    }
}

#[tokio::test]
async fn interrupted_read_is_retried() {
    let frame = tpkt(&[1, 2, 3]);
    let mut reader = ScriptedReads(VecDeque::from([
        Ok(frame[..2].to_vec()),
        Err(io::ErrorKind::Interrupted.into()),
        Ok(frame[2..].to_vec()),
    ]));
    assert_eq!(read_tpkt(&mut reader, 487, SECOND).await.unwrap(), frame);
}

#[tokio::test]
async fn read_tpkt_validates_the_header_like_sync() {
    for (header, expected) in [
        ([2, 0, 0, 4], kind::ISO_INVALID_HEADER),
        ([3, 1, 0, 4], kind::ISO_INVALID_HEADER),
        ([3, 0, 0, 3], kind::ISO_INVALID_TELEGRAM),
        ([3, 0, 1, 232], kind::ISO_INVALID_TELEGRAM),
    ] {
        let (mut client, mut server) = duplex(64);
        server.write_all(&header).await.unwrap();
        assert_kind(&read_tpkt(&mut client, 487, SECOND).await, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn request_not_sent_within_write_timeout_is_io() {
    // Nobody reads the other end, so the small duplex buffer fills up.
    let (mut client, _server) = duplex(8);
    let result = send(&mut client, &[0; 64], Duration::from_millis(500)).await;
    assert_kind(&result, kind::IO);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("request not sent within 500 ms"));
}

#[tokio::test]
async fn send_flushes_a_buffered_writer() {
    let (client, mut server) = duplex(64);
    let mut writer = tokio::io::BufWriter::new(client);
    send(&mut writer, &[3, 0, 0, 5, 9], SECOND).await.unwrap();
    let mut received = [0; 5];
    let read = tokio::time::timeout(SECOND, server.read_exact(&mut received)).await;
    assert!(read.is_ok(), "the request must leave the writer's buffer");
    assert_eq!(received, [3, 0, 0, 5, 9]);
}

#[tokio::test]
async fn negotiation_matches_the_sync_implementation() {
    let reply = data_frame(&[
        0x32, 3, 0, 0, 4, 0, 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1, 0, 1, 0, 240,
    ]);
    let (mut client, mut server) = duplex(64);
    let expected = reply.clone();
    let peer = tokio::spawn(async move {
        let mut request = [0; 25];
        server.read_exact(&mut request).await.unwrap();
        server.write_all(&expected).await.unwrap();
        request
    });
    let limits = negotiate(&mut client, 512, TIMEOUTS).await.unwrap();
    assert_eq!(limits.pdu_length, 240);
    assert_eq!(peer.await.unwrap(), protocol::negotiation_request(512).0);

    let (mut client, mut server) = duplex(64);
    server.write_all(&tpkt(&[2, 0xf0, 0])).await.unwrap();
    let mut request = [0; 25];
    let err = negotiate(&mut client, 512, TIMEOUTS).await;
    server.read_exact(&mut request).await.unwrap();
    assert_kind(&err, kind::PDU_NEGOTIATION_FAILED);
}
