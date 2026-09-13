//! In-process fake of a Siemens S7 PLC speaking the ISO-on-TCP (RFC 1006 /
//! COTP) + S7comm subset used by `rust7`.
//!
//! It answers the two-step handshake according to a [`Handshake`] policy,
//! hands every subsequent S7 request frame to a user supplied handler and
//! records everything it received, so tests can assert on the exact bytes the
//! client put on the wire.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rust7::S7Client;

// ---- protocol constants (mirrors of the private ones in rust7::client) ----

pub const ISO_ID: u8 = 0x03;
pub const S7_ID: u8 = 0x32;
pub const EOT: u8 = 0x80;
pub const COTP_CONNECT_REQUEST: u8 = 0xE0;
pub const COTP_CONNECT_CONFIRM: u8 = 0xD0;
pub const COTP_DISCONNECT_REQUEST: u8 = 0x80;
pub const FN_READ_VAR: u8 = 0x04;
pub const FN_WRITE_VAR: u8 = 0x05;
pub const RES_SUCCESS: u8 = 0xFF;
pub const RES_INVALID_ADDRESS: u8 = 0x05;
pub const RES_NOT_FOUND: u8 = 0x0A;
pub const TS_RES_BIT: u8 = 0x03;
pub const TS_RES_BYTE: u8 = 0x04;
pub const DEFAULT_PDU: u16 = 240;

/// Size of the client's ISO connection request.
pub const ISO_CR_LEN: usize = 22;
/// Size of the client's PDU negotiation request.
pub const ISO_PN_REQ_LEN: usize = 25;
/// Size of a Read Var request (no payload).
pub const READ_REQ_LEN: usize = 31;
/// Header size of a Write Var request (payload follows).
pub const WRITE_REQ_HEADER_LEN: usize = 35;
/// S7 bytes of a Read Var response that precede the payload.
pub const READ_RES_LEN: usize = 18;
/// S7 bytes of a Write Var response.
pub const WRITE_RES_LEN: usize = 15;

/// Largest data payload the client puts in a single read request for `pdu`.
pub fn max_read_chunk(pdu: u16) -> usize {
    (pdu - 18) as usize
}

/// Largest data payload the client puts in a single write request for `pdu`.
pub fn max_write_chunk(pdu: u16) -> usize {
    (pdu - 28) as usize
}

// ---- handshake / reply policies -----------------------------------------------

/// How the fake PLC answers the ISO connection + PDU negotiation handshake.
#[derive(Clone, Copy, Debug)]
pub enum Handshake {
    /// Confirm the ISO connection and negotiate `pdu_length`.
    Accept { pdu_length: u16 },
    /// Answer the ISO connection request with a Disconnect Request.
    RejectIso,
    /// Close the socket right after reading the ISO connection request.
    CloseAfterCr,
    /// Confirm ISO, but answer the negotiation with a non-S7 protocol id.
    BadNegotiationProtocolId,
    /// Confirm ISO, but answer the negotiation with a non-zero error class.
    NegotiationErrorClass,
    /// Confirm ISO, then close the socket instead of answering the negotiation.
    CloseAfterNegotiationRequest,
}

/// What the fake PLC does with one S7 request received after the handshake.
///
/// Every `Send`-like reply that carries an S7 header gets the PDU reference
/// of the request it answers copied into bytes `[11..13]` (a real CPU echoes
/// it), so handlers can keep using the frame builders below without caring
/// about references. Use [`Reply::SendRaw`] to send bytes untouched.
pub enum Reply {
    /// Write these bytes back, correlated with the request.
    Send(Vec<u8>),
    /// Write these bytes back verbatim, no reference correlation.
    SendRaw(Vec<u8>),
    /// Wait, then write these bytes back (drives client read timeouts and
    /// late replies).
    Delayed(Duration, Vec<u8>),
    /// Write the first part, wait, write the rest (a reply split across two
    /// TCP segments).
    Split(Vec<u8>, Duration, Vec<u8>),
    /// Shut the connection down without answering.
    Close,
    /// Never answer, keep the connection open (drives client read timeouts).
    Silent,
}

pub type Handler = dyn Fn(&[u8]) -> Reply + Send + Sync + 'static;

#[derive(Clone, Debug, Default)]
pub struct Captured {
    pub connection_requests: Vec<Vec<u8>>,
    pub negotiation_requests: Vec<Vec<u8>>,
    pub requests: Vec<Vec<u8>>,
}

pub struct FakePlc {
    addr: SocketAddr,
    captured: Arc<Mutex<Captured>>,
    closed_connections: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
}

impl Drop for FakePlc {
    /// Stops the acceptor thread: raise the flag, then poke the listener so
    /// the blocking `accept()` returns and observes it.
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

impl FakePlc {
    /// Starts listening on an ephemeral loopback port. Every accepted
    /// connection is served on its own thread, so tests may reconnect freely.
    pub fn start(
        handshake: Handshake,
        handler: impl Fn(&[u8]) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(Captured::default()));
        let closed_connections = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);

        let cap = Arc::clone(&captured);
        let closed = Arc::clone(&closed_connections);
        let stop = Arc::clone(&stopping);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                let cap = Arc::clone(&cap);
                let handler = Arc::clone(&handler);
                let closed = Arc::clone(&closed);
                thread::spawn(move || {
                    serve(stream, handshake, &*handler, &cap);
                    closed.fetch_add(1, Ordering::SeqCst);
                });
            }
        });

        Self {
            addr,
            captured,
            closed_connections,
            stopping,
        }
    }

    pub fn ip(&self) -> String {
        self.addr.ip().to_string()
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// A fresh client already pointed at this fake PLC (not connected yet).
    pub fn client(&self) -> S7Client {
        let mut client = S7Client::new();
        client
            .set_connection_port(self.port())
            .expect("ephemeral port is never 0");
        client
    }

    /// A client connected with `connect_s71200_1500`.
    pub fn connected_client(&self) -> S7Client {
        let mut client = self.client();
        client
            .connect_s71200_1500(&self.ip())
            .expect("handshake with fake PLC");
        client
    }

    pub fn captured(&self) -> Captured {
        self.captured.lock().unwrap().clone()
    }

    pub fn connection_requests(&self) -> Vec<Vec<u8>> {
        self.captured().connection_requests
    }

    pub fn negotiation_requests(&self) -> Vec<Vec<u8>> {
        self.captured().negotiation_requests
    }

    /// S7 request frames received after the handshake, oldest first.
    pub fn requests(&self) -> Vec<Vec<u8>> {
        self.captured().requests
    }

    /// Removes and returns the recorded S7 request frames.
    pub fn take_requests(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.captured.lock().unwrap().requests)
    }

    /// Removes and returns the recorded ISO connection requests.
    pub fn take_connection_requests(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.captured.lock().unwrap().connection_requests)
    }

    pub fn closed_connections(&self) -> usize {
        self.closed_connections.load(Ordering::SeqCst)
    }

    /// Waits (bounded) until at least `n` connections have been closed by the peer.
    pub fn wait_for_closed(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.closed_connections() >= n {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.closed_connections() >= n
    }
}

/// Reads one TPKT-framed telegram (4-byte header carrying the total length).
fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).ok()?;
    if header[0] != ISO_ID {
        return None;
    }
    let len = u16::from_be_bytes([header[2], header[3]]) as usize;
    if len < header.len() {
        return None;
    }
    let mut frame = vec![0u8; len];
    frame[..4].copy_from_slice(&header);
    stream.read_exact(&mut frame[4..]).ok()?;
    Some(frame)
}

fn serve(
    mut stream: TcpStream,
    handshake: Handshake,
    handler: &Handler,
    captured: &Mutex<Captured>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_nodelay(true);

    let Some(cr) = read_frame(&mut stream) else {
        return;
    };
    captured.lock().unwrap().connection_requests.push(cr);

    match handshake {
        Handshake::CloseAfterCr => return,
        Handshake::RejectIso => {
            let _ = stream.write_all(&iso_connection_reply(COTP_DISCONNECT_REQUEST));
            return;
        }
        _ => {
            if stream
                .write_all(&iso_connection_reply(COTP_CONNECT_CONFIRM))
                .is_err()
            {
                return;
            }
        }
    }

    let Some(pn) = read_frame(&mut stream) else {
        return;
    };
    captured
        .lock()
        .unwrap()
        .negotiation_requests
        .push(pn.clone());

    let mut reply = match handshake {
        Handshake::Accept { pdu_length } => negotiation_reply(S7_ID, 0x00, pdu_length),
        Handshake::BadNegotiationProtocolId => negotiation_reply(0x00, 0x00, DEFAULT_PDU),
        Handshake::NegotiationErrorClass => negotiation_reply(S7_ID, 0x81, DEFAULT_PDU),
        Handshake::CloseAfterNegotiationRequest => return,
        Handshake::RejectIso | Handshake::CloseAfterCr => unreachable!("handled above"),
    };
    correlate(&mut reply, &pn);
    if stream.write_all(&reply).is_err() {
        return;
    }

    loop {
        let Some(request) = read_frame(&mut stream) else {
            return;
        };
        captured.lock().unwrap().requests.push(request.clone());
        match handler(&request) {
            Reply::Send(mut bytes) => {
                correlate(&mut bytes, &request);
                if stream.write_all(&bytes).is_err() {
                    return;
                }
            }
            Reply::SendRaw(bytes) => {
                if stream.write_all(&bytes).is_err() {
                    return;
                }
            }
            Reply::Delayed(delay, mut bytes) => {
                correlate(&mut bytes, &request);
                thread::sleep(delay);
                if stream.write_all(&bytes).is_err() {
                    return;
                }
            }
            Reply::Split(head, gap, tail) => {
                let cut = head.len();
                let mut whole = head;
                whole.extend_from_slice(&tail);
                correlate(&mut whole, &request);
                if stream.write_all(&whole[..cut]).is_err() || stream.flush().is_err() {
                    return;
                }
                thread::sleep(gap);
                if stream.write_all(&whole[cut..]).is_err() {
                    return;
                }
            }
            Reply::Close => {
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            // Just wait for the next frame (or for the client to give up).
            Reply::Silent => {}
        }
    }
}

/// Copies the PDU reference of `request` into `reply`, if both carry an S7 header.
fn correlate(reply: &mut [u8], request: &[u8]) {
    const REF: std::ops::Range<usize> = 11..13;
    if reply.len() >= REF.end
        && request.len() >= REF.end
        && reply[7] == S7_ID
        && request[7] == S7_ID
    {
        reply[REF].copy_from_slice(&request[REF]);
    }
}

/// PDU reference carried by an S7 frame (request or reply).
pub fn pdu_ref(frame: &[u8]) -> u16 {
    be16(frame, 11)
}

// ---- frame builders ----------------------------------------------------------

fn hi(n: usize) -> u8 {
    ((n >> 8) & 0xFF) as u8
}

fn lo(n: usize) -> u8 {
    (n & 0xFF) as u8
}

pub fn be16(frame: &[u8], index: usize) -> u16 {
    u16::from_be_bytes([frame[index], frame[index + 1]])
}

/// 22-byte COTP connection reply (`pdu_type` = 0xD0 confirm, 0x80 disconnect).
///
/// For the disconnect case only the PDU-type byte is realistic: the client
/// inspects nothing but `[5]`, so the rest of a real DR frame is not modelled.
pub fn iso_connection_reply(pdu_type: u8) -> Vec<u8> {
    vec![
        ISO_ID, 0x00, 0x00, 0x16, // TPKT, length 22
        0x11, pdu_type, // COTP length, PDU type
        0x00, 0x00, // dst ref
        0x00, 0x01, // src ref
        0x00, // class/options
        0xC0, 0x01, 0x0A, // TPDU size
        0xC1, 0x02, 0x01, 0x00, // src TSAP
        0xC2, 0x02, 0x01, 0x00, // dst TSAP
    ]
}

/// 27-byte S7 "Setup Communication" ack-data telegram.
pub fn negotiation_reply(protocol_id: u8, error_class: u8, pdu_length: u16) -> Vec<u8> {
    vec![
        ISO_ID,
        0x00,
        0x00,
        0x1B, // TPKT, length 27
        0x02,
        0xF0,
        EOT, // COTP data
        protocol_id,
        0x03,
        0x00,
        0x00, // S7 header: id, ack-data, redundancy
        0x05,
        0x00, // pdu reference
        0x00,
        0x08, // parameter length
        0x00,
        0x00, // data length
        error_class,
        0x00, // error class / code
        0xF0,
        0x00, // function: setup communication, reserved
        0x00,
        0x01, // max AMQ calling
        0x00,
        0x01, // max AMQ called
        hi(pdu_length as usize),
        lo(pdu_length as usize),
    ]
}

/// Read Var response: 7 (TPKT+COTP) + 18 (S7 header + item) + payload.
pub fn read_reply(return_code: u8, payload: &[u8]) -> Vec<u8> {
    let total = 7 + READ_RES_LEN + payload.len();
    let data_len = 4 + payload.len();
    let (transport, bits) = if return_code == RES_SUCCESS {
        (TS_RES_BYTE, payload.len() * 8)
    } else {
        (0x00, 0)
    };
    let mut frame = vec![
        ISO_ID,
        0x00,
        hi(total),
        lo(total),
        0x02,
        0xF0,
        EOT,
        S7_ID,
        0x03,
        0x00,
        0x00,
        0x05,
        0x00,
        0x00,
        0x02,
        hi(data_len),
        lo(data_len),
        0x00,
        0x00,
        FN_READ_VAR,
        0x01,
        return_code,
        transport,
        hi(bits),
        lo(bits),
    ];
    frame.extend_from_slice(payload);
    frame
}

/// Write Var response: 7 (TPKT+COTP) + 15 (S7 header + item return code).
pub fn write_reply(return_code: u8) -> Vec<u8> {
    vec![
        ISO_ID,
        0x00,
        0x00,
        0x16,
        0x02,
        0xF0,
        EOT,
        S7_ID,
        0x03,
        0x00,
        0x00,
        0x05,
        0x00,
        0x00,
        0x02,
        0x00,
        0x01,
        0x00,
        0x00,
        FN_WRITE_VAR,
        0x01,
        return_code,
    ]
}

/// A raw telegram whose TPKT header claims `claimed_s7_len` S7 bytes but which
/// actually carries `actual` bytes after the 7-byte header.
pub fn raw_telegram(claimed_s7_len: usize, actual: &[u8]) -> Vec<u8> {
    let total = 7 + claimed_s7_len;
    let mut frame = vec![ISO_ID, 0x00, hi(total), lo(total), 0x02, 0xF0, EOT];
    frame.extend_from_slice(actual);
    frame
}

// ---- request parsing ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRequest {
    pub telegram_len: u16,
    pub function: u8,
    pub param_len: u16,
    pub data_len: u16,
    pub wordlen: u8,
    pub count: u16,
    pub db_number: u16,
    pub area: u8,
    /// 24-bit bit address inside the area.
    pub address: u32,
    /// Write only: transport size byte.
    pub transport: Option<u8>,
    /// Write only: payload length in bits.
    pub bit_count: Option<u16>,
    /// Write only: data bytes following the 35-byte header.
    pub payload: Vec<u8>,
}

impl ParsedRequest {
    pub fn byte_address(&self) -> u32 {
        self.address >> 3
    }

    pub fn bit_index(&self) -> u8 {
        (self.address & 0x07) as u8
    }
}

pub fn parse_request(frame: &[u8]) -> ParsedRequest {
    assert!(
        frame.len() >= READ_REQ_LEN,
        "frame too short: {} bytes",
        frame.len()
    );
    assert_eq!(frame[0], ISO_ID, "TPKT id");
    assert_eq!(&frame[4..7], &[0x02, 0xF0, EOT], "COTP data header");
    assert_eq!(frame[7], S7_ID, "S7 protocol id");
    assert_eq!(frame[8], 0x01, "S7 job");
    assert_eq!(
        &frame[19..22],
        &[0x12, 0x0A, 0x10],
        "item var-spec / syntax id"
    );

    let function = frame[17];
    let (transport, bit_count, payload) = if function == FN_WRITE_VAR {
        assert!(frame.len() >= WRITE_REQ_HEADER_LEN, "write frame too short");
        (
            Some(frame[32]),
            Some(be16(frame, 33)),
            frame[WRITE_REQ_HEADER_LEN..].to_vec(),
        )
    } else {
        (None, None, Vec::new())
    };

    ParsedRequest {
        telegram_len: be16(frame, 2),
        function,
        param_len: be16(frame, 13),
        data_len: be16(frame, 15),
        wordlen: frame[22],
        count: be16(frame, 23),
        db_number: be16(frame, 25),
        area: frame[27],
        address: ((frame[28] as u32) << 16) | ((frame[29] as u32) << 8) | frame[30] as u32,
        transport,
        bit_count,
        payload,
    }
}

pub fn local_tsap(connection_request: &[u8]) -> u16 {
    be16(connection_request, 16)
}

pub fn remote_tsap(connection_request: &[u8]) -> u16 {
    be16(connection_request, 20)
}

// ---- default "memory" model --------------------------------------------------

/// Deterministic content of the fake PLC memory, keyed by DB and byte address.
pub fn memory_byte(db: u16, byte_addr: u32) -> u8 {
    (byte_addr.wrapping_mul(31) ^ (db as u32).wrapping_mul(7) ^ 0xA5) as u8
}

/// Serves reads from [`memory_byte`] and acknowledges every write.
pub fn echo_memory_handler(request: &[u8]) -> Reply {
    let req = parse_request(request);
    match req.function {
        FN_READ_VAR if req.wordlen == 0x01 => {
            let byte = memory_byte(req.db_number, req.byte_address());
            let bit = (byte >> req.bit_index()) & 1;
            Reply::Send(read_reply(RES_SUCCESS, &[bit]))
        }
        FN_READ_VAR => {
            let payload: Vec<u8> = (0..req.count as u32)
                .map(|i| memory_byte(req.db_number, req.byte_address() + i))
                .collect();
            Reply::Send(read_reply(RES_SUCCESS, &payload))
        }
        FN_WRITE_VAR => Reply::Send(write_reply(RES_SUCCESS)),
        _ => Reply::Close,
    }
}
