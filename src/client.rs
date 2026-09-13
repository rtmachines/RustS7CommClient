// Rust7 - Native Rust S7 client (Snap7‑style) for Siemens PLCs.
// Copyright 2025 - Davide Nardella

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub use crate::error::S7Error;

// Connection types
pub const CT_PG: u16 = 0x0001; // As PG (Default)
pub const CT_OP: u16 = 0x0002; // As OP
pub const CT_S7: u16 = 0x0003; // AS S7 Basic

// Areas
pub const S7_AREA_PE: u8 = 0x81; // Process Inputs
pub const S7_AREA_PA: u8 = 0x82; // Process Outputs
pub const S7_AREA_MK: u8 = 0x83; // Merkers
pub const S7_AREA_DB: u8 = 0x84; // Data Block

// Wordlen
pub const S7_WL_BIT: u8 = 0x01;
pub const S7_WL_BYTE: u8 = 0x02;

// Transport sizes found in read/write items
const TS_RES_BIT: u8 = 0x03; // length in bits, 1 bit
const TS_RES_BYTE: u8 = 0x04; // length in bits
const TS_RES_INT: u8 = 0x05; // length in bits
const TS_RES_DINT: u8 = 0x06; // length in bits
const TS_RES_REAL: u8 = 0x07; // length in bits
const TS_RES_OCTET: u8 = 0x09; // length in bytes

// PDU related
const TPKT_HDR_LEN: usize = 4; // TPKT header length
const TPKT_ISO_LEN: usize = 7; // TPKT + COTP data header length
const PDU_LEN_REQ: u16 = 480; // PDU Length requested for negotiation
const PDU_LEN_MIN: u16 = 29; // Write request header (28) + at least 1 payload byte
const ISO_CR_LEN: usize = 22; // Connection request telegram size
const ISO_CC_MIN_LEN: usize = 11; // TPKT + smallest COTP connection confirm
const ISO_CC_MAX_LEN: usize = TPKT_HDR_LEN + 256; // TPKT + COTP length byte range
const ISO_CONN_REQ: u8 = 0xE0; // ISO connection request
const ISO_CONN_OK: u8 = 0xD0; // ISO connection accepted
const ISO_PN_REQ_LEN: usize = 25; // PDU negotiation request telegram size
const S7_PN_RES_LEN: usize = 20; // PDU negotiation response, S7 part
const ISO_ID: u8 = 0x03; // RFC 1006 ID
const S7_ID: u8 = 0x32; // S7 Protocol ID
const S7_ACK_DATA: u8 = 0x03; // S7 message type "ack data"
const S7_ACK_HDR_LEN: usize = 12; // S7 ack header (with error class/code)
const S7_ACK_MIN_LEN: usize = 14; // ack header + function + item count

const READ_REQ_LEN: usize = 31; // TPKT + ISO + S7 headers
const READ_RES_LEN: usize = 18; // Read job response header length
const WRITE_RES_LEN: usize = 15; // Write job response header length
const READ_RES_OVERHEAD: u16 = 18; // S7 bytes of a read response that are not payload
const WRITE_REQ_OVERHEAD: u16 = 28; // S7 bytes of a write request that are not payload

const FN_READ_VAR: u8 = 0x04;
const FN_WRITE_VAR: u8 = 0x05;
const FN_SETUP_COMM: u8 = 0xF0;

const EOT: u8 = 0x80; // ISO End of Transmission
const RW_RES_OFFSET: usize = 14; // Item return code inside the S7 part

const MAX_RACK: u16 = 7;
const MAX_SLOT: u16 = 31;
const MAX_BIT_IDX: u8 = 7;

/// Operation successful
const RES_SUCCESS: u8 = 0xFF;
/// Invalid Address requested
/// - Trying to read beyond the limits
/// - The DB is optimized
const RES_INVALID_ADDRESS: u8 = 0x05;
/// Resource not found
/// - The DB doesn't exists in the CPU
const RES_NOT_FOUND: u8 = 0x0A;

// Macros
macro_rules! hi_part {
    ($x:expr) => {
        (($x >> 8) & 0xFF) as u8
    };
}

macro_rules! lo_part {
    ($x:expr) => {
        ($x & 0xFF) as u8
    };
}

macro_rules! make_u16 {
    ($hi:expr, $lo:expr) => {
        ((($hi as u16) << 8) | ($lo as u16))
    };
}

/// ### Native S7 client
///
/// One `S7Client` owns one TCP connection to one CPU and performs one job at a
/// time: every method takes `&mut self`, so two threads cannot interleave
/// requests on the same client without an explicit `Mutex`. The type is
/// `Send`, so it can be moved to (or wrapped in a `Mutex` and shared with)
/// another thread.
///
/// ### Connection state
/// Any transport or protocol error (see [`S7Error::requires_reconnect`])
/// drops the connection: the client refuses further jobs with
/// `S7Error::NotConnected` until one of the `connect_*` methods succeeds
/// again. This is deliberate — after such an error the byte stream may be out
/// of sync with the CPU and a reply could be attributed to the wrong request.
pub struct S7Client {
    stream: Option<TcpStream>,
    port: u16,
    co_timeout_ms: u64,
    rd_timeout_ms: u64,
    wr_timeout_ms: u64,
    conn_type: u16,
    max_rd_pdu_data: u16, // Max Read PDU Payload
    max_wr_pdu_data: u16, // Max Write PDU Payload
    pdu_length: u16,      // PDU length negotiated by the CPU
    pdu_ref: u16,         // Reference of the last request sent
    rx_buf: Vec<u8>,      // S7 part of the last telegram received
    last_time: f64,
    chunks: usize,
}

/// ### Checks the incoming ISO Packet coherence
///
/// Typically, a PLC never sends incorrect values, but we may find data in the buffer
/// from a fragmented transmission, so it is good practice to check.
///
fn check_iso_packet(pdu_length: u16, iso_packet: &[u8; TPKT_ISO_LEN]) -> Result<usize, S7Error> {
    //
    //  TPKT + ISO Header
    //
    //  TPKT
    //      [0]    RFC 1006 ID          0x03
    //      [1]    Reserved             0x00
    //      [2]    HI Telegram length   Variable
    //      [3]    LO Telegram length   Variable
    //  ISO
    //      [4]    Length               0x02
    //      [5]    PDU Type             0xF0
    //      [6]    EOT                  0x80

    if iso_packet[0] != ISO_ID || iso_packet[4] != 0x02 || iso_packet[5] != 0xF0 {
        return Err(S7Error::IsoInvalidHeader);
    }

    if iso_packet[6] != EOT {
        return Err(S7Error::IsoFragmentedPacket);
    }

    let telegram_length: usize = make_u16!(iso_packet[2], iso_packet[3]) as usize;

    if telegram_length < TPKT_ISO_LEN
        || telegram_length - TPKT_ISO_LEN > pdu_length as usize
        || telegram_length - TPKT_ISO_LEN == 0
    {
        return Err(S7Error::IsoInvalidTelegram);
    }

    // Returns the remaining bytes to read from the telegram
    Ok(telegram_length - TPKT_ISO_LEN)
}

/// Number of payload bytes announced by a read item header.
fn item_payload_len(transport: u8, count: u16) -> Option<usize> {
    match transport {
        TS_RES_BIT | TS_RES_OCTET => Some(count as usize),
        TS_RES_BYTE | TS_RES_INT | TS_RES_DINT | TS_RES_REAL => {
            if count.is_multiple_of(8) {
                Some((count / 8) as usize)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Bit address of `byte_num.bit_idx`, or `InvalidFunParameter` when it does not fit
/// the 16-bit `start` of the area functions.
fn bit_start(byte_num: u16, bit_idx: u8) -> Result<u16, S7Error> {
    if bit_idx > MAX_BIT_IDX {
        return Err(S7Error::InvalidFunParameter);
    }
    byte_num
        .checked_mul(8)
        .and_then(|bits| bits.checked_add(u16::from(bit_idx)))
        .ok_or(S7Error::InvalidFunParameter)
}

/// Reads one COTP telegram (TPKT header + whatever the length field announces).
/// Used for the connection confirm, whose size varies between devices.
fn recv_cotp_frame(stream: &mut TcpStream, frame: &mut Vec<u8>) -> Result<(), S7Error> {
    let mut tpkt = [0u8; TPKT_HDR_LEN];
    stream.read_exact(&mut tpkt)?;
    if tpkt[0] != ISO_ID {
        return Err(S7Error::IsoInvalidHeader);
    }
    let len = make_u16!(tpkt[2], tpkt[3]) as usize;
    if !(ISO_CC_MIN_LEN..=ISO_CC_MAX_LEN).contains(&len) {
        return Err(S7Error::IsoInvalidTelegram);
    }
    frame.clear();
    frame.resize(len, 0);
    frame[..TPKT_HDR_LEN].copy_from_slice(&tpkt);
    stream.read_exact(&mut frame[TPKT_HDR_LEN..])?;
    Ok(())
}

/// Reads one complete data telegram: the S7 part lands in `rx[..n]` and `n` is returned.
fn recv_telegram(stream: &mut TcpStream, pdu_length: u16, rx: &mut [u8]) -> Result<usize, S7Error> {
    let mut iso_packet = [0u8; TPKT_ISO_LEN];
    stream.read_exact(&mut iso_packet)?;
    let n = check_iso_packet(pdu_length, &iso_packet)?;
    if n > rx.len() {
        return Err(S7Error::IsoInvalidTelegram);
    }
    stream.read_exact(&mut rx[..n])?;
    Ok(n)
}

/// Reads the acknowledgement of the request identified by `expected_ref` /
/// `expected_fn` and validates its S7 header.
///
/// Errors: transport errors, `Iso*` (bad framing), `S7InvalidResponse` (a
/// telegram that does not answer our request) and `S7Unspecified` (the CPU
/// answered our request with a non-zero error class: consumed, in sync).
fn recv_ack(
    stream: &mut TcpStream,
    pdu_length: u16,
    rx: &mut [u8],
    expected_ref: u16,
    expected_fn: u8,
    min_len: usize,
) -> Result<usize, S7Error> {
    //  S7 ack-data header
    //      [0]      Protocol ID          0x32
    //      [1]      Message type         0x03 (ack data)
    //      [2..4]   Redundancy id
    //      [4..6]   PDU reference        echo of the request
    //      [6..8]   Parameter length
    //      [8..10]  Data length
    //      [10]     Error class
    //      [11]     Error code
    //      [12]     Function             echo of the request
    //      [13]     Item count
    let n = recv_telegram(stream, pdu_length, rx)?;
    let s7 = &rx[..n];
    if n < S7_ACK_HDR_LEN {
        return Err(S7Error::IsoInvalidTelegram);
    }
    if s7[0] != S7_ID || s7[1] != S7_ACK_DATA || make_u16!(s7[4], s7[5]) != expected_ref {
        return Err(S7Error::S7InvalidResponse);
    }
    if s7[10] != 0x00 || s7[11] != 0x00 {
        return Err(S7Error::S7Unspecified);
    }
    if n < S7_ACK_MIN_LEN || s7[12] != expected_fn || n < min_len {
        return Err(S7Error::S7InvalidResponse);
    }
    Ok(n)
}

/// Maps an item return code to the operation result.
fn item_result(return_code: u8) -> Result<(), S7Error> {
    match return_code {
        RES_SUCCESS => Ok(()),
        RES_NOT_FOUND => Err(S7Error::S7NotFound),
        RES_INVALID_ADDRESS => Err(S7Error::S7InvalidAddress),
        _ => Err(S7Error::S7Unspecified),
    }
}

impl Default for S7Client {
    fn default() -> Self {
        Self::new()
    }
}

impl S7Client {
    /// ### Creates a new `S7Client` instance with default settings.
    ///
    /// The client starts disconnected, use one of `connect_XXX` methods to open a connection to a PLC.
    ///
    /// ### Returns
    /// A new `S7Client` ready to connect.
    ///
    pub fn new() -> Self {
        S7Client {
            stream: None,
            port: 102,
            co_timeout_ms: 3000,
            rd_timeout_ms: 1000,
            wr_timeout_ms: 500,
            conn_type: CT_PG,
            max_rd_pdu_data: 0,
            max_wr_pdu_data: 0,
            pdu_length: 0x0000,
            pdu_ref: 0,
            rx_buf: vec![0u8; PDU_LEN_REQ as usize],
            last_time: 0.0,
            chunks: 0,
        }
    }

    /// ### Client connected
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// ### PDU length negotiated by the CPU
    ///
    /// 0 while disconnected or after a failed negotiation.
    pub fn pdu_length(&self) -> u16 {
        self.pdu_length
    }

    /// ### Last Job time (ms).
    ///
    /// If an error occurred the value will be 0
    pub fn last_time(&self) -> f64 {
        self.last_time
    }

    /// ### Indicates how many pieces the data to be read or written in the last operation was divided into
    ///
    /// Maybe you need to know it only for extreme tuning
    pub fn chunks(&self) -> usize {
        self.chunks
    }

    /// ### Check S7 Area validity
    ///
    fn check_area(&self, area: u8) -> Result<(), S7Error> {
        const AREAS: [u8; 4] = [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB];
        if !AREAS.contains(&area) {
            return Err(S7Error::InvalidFunParameter);
        }
        Ok(())
    }

    /// Reference for the next request (never 0, wraps around).
    fn next_ref(&mut self) -> u16 {
        self.pdu_ref = self.pdu_ref.wrapping_add(1).max(1);
        self.pdu_ref
    }

    /// Drops the connection when `result` carries a transport/protocol error,
    /// so a stream that may be out of sync is never reused.
    fn settle<T>(&mut self, result: Result<T, S7Error>) -> Result<T, S7Error> {
        if let Err(e) = &result {
            if e.requires_reconnect() {
                self.disconnect();
            }
        }
        result
    }

    /// ### Changes the S7 connection type to the PLC
    ///
    /// The three possible connection types are:
    /// - `CT_PG`: (as a programming device)
    /// - `CT_OP`: (as an HMI)
    /// - `CT_S7`: (as a generic device)
    ///
    /// In practice, there aren't many differences; the S7_PG connection should ensure
    /// better system responsiveness, but in reality, I've never noticed any noticeable differences.
    ///
    /// `CT_PG` is used by default.
    ///
    /// With very old PLCs (early S7300 series) that have limited communication resources,
    /// the connection may be rejected if we have S7Manager with many online windows open at the same time.
    /// In this case, use `S7_OP` or `S7_BASIC`.
    ///
    /// ### Parameters
    /// - `connection_type`: Connection type.
    ///
    /// ### Returns
    /// `Ok(())` on success, or an `S7Error` on failure.
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    ///
    /// #### Notes
    /// 1. The client must not be connected (that is, call this method before connecting).
    /// 2. This method is not useful if you use `connect_tsap()` because the connection_type is already contained in the REMOTE_TSAP record.
    ///
    pub fn set_connection_type(&mut self, connection_type: u16) -> Result<(), S7Error> {
        if !(CT_PG..=CT_S7).contains(&connection_type) {
            return Err(S7Error::InvalidFunParameter);
        }

        self.conn_type = connection_type;

        Ok(())
    }

    /// ### Sets operations timeout
    ///
    /// ### Parameters
    /// - `co_timeout_ms` : TCP Connection timeout (ms) (Default = 3000 ms)
    /// - `rd_timeout_ms` : Read Connection timeout (ms) (Default = 1000 ms)
    /// - `wr_timeout_ms` : Write Connection timeout (ms) (Default = 500 ms)
    ///
    /// ### Returns
    /// `Ok(())` on success, or an `S7Error` on failure.
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    ///
    /// ### Notes
    /// 1. Values must be > 0, otherwise they are ignored
    /// 2. The client must not be connected (that is, call this method before connecting).
    ///
    pub fn set_timeout(&mut self, co_timeout_ms: u64, rd_timeout_ms: u64, wr_timeout_ms: u64) -> Result<(), S7Error> {
        if co_timeout_ms == 0 || rd_timeout_ms == 0 || wr_timeout_ms == 0 {
            return Err(S7Error::InvalidFunParameter);
        }

        self.co_timeout_ms = co_timeout_ms;
        self.rd_timeout_ms = rd_timeout_ms;
        self.wr_timeout_ms = wr_timeout_ms;

        Ok(())
    }

    /// ### Sets the TCP Connection Port
    ///
    /// The default S7 Port is 102, but if you need NAT the addresses you can use this method to change the default value.
    ///
    /// ### Parameters
    /// - `port`: TCP Connection port (1..65535)
    ///
    /// ### Returns
    /// `Ok(())` on success, or an `S7Error` on failure.
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    ///
    /// ### Notes
    /// 1. Value must be > 0, otherwise it is ignored
    /// 2. The client must not be connected (that is, call this method before connecting).
    ///
    pub fn set_connection_port(&mut self, port: u16) -> Result<(), S7Error> {
        if port == 0 {
            return Err(S7Error::InvalidFunParameter);
        }

        self.port = port;

        Ok(())
    }

    /// ### Connects to the S71200 or S71500 families
    ///
    /// This helper method is same as `connect_rack_slot()` with rack=0 and slot=0
    /// ### Parameters
    /// - `ip`  : PLC IPV4 address.
    ///
    /// ---
    /// For Notes, Return and Errors look at `connect_tsap()`
    ///
    pub fn connect_s71200_1500(&mut self, ip: &str) -> Result<(), S7Error> {
        self.connect_rack_slot(ip, 0, 0)
    }

    /// ### Connects to the S7300 family
    ///
    /// This helper method is same as `connect_rack_slot()` with rack=0 and slot=2
    /// ### Parameters
    /// - `ip`  : PLC IPV4 address.
    ///
    /// ---
    /// For Notes, Return and Errors look at `connect_tsap()`
    ///
    pub fn connect_s7300(&mut self, ip: &str) -> Result<(), S7Error> {
        self.connect_rack_slot(ip, 0, 2)
    }

    /// ### Connects to a Siemens PLC/Drive using Rack and Slot
    ///
    /// Rack and Slot are Hardware configuration parameters.
    ///
    /// For S7300 and S71200/1500 they are fixed, (see `connect_s7300()` and `connect_s71200_1500()` ).
    ///
    /// Ultimately, you will need of this method only to connect to S7400, WinAC or other Siemens
    /// hardware, like Drives, which Rack and Slot can vary.
    ///
    /// ### Parameters
    /// - `ip` : PLC IPV4 address.
    /// - `rack` : CPU/CU Rack (0..7).
    /// - `slot` : CPU/CU Slot (0..31).
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: rack or slot out of range.
    ///
    /// ---
    /// For Notes, Return and other Errors look at `connect_tsap()`
    ///
    pub fn connect_rack_slot(&mut self, ip: &str, rack: u16, slot: u16) -> Result<(), S7Error> {
        if rack > MAX_RACK || slot > MAX_SLOT {
            return Err(S7Error::InvalidFunParameter);
        }
        let local_tsap: u16 = 0x0100;
        let remote_tsap: u16 = (self.conn_type << 8) | (rack << 5) | slot;
        self.connect_tsap(ip, local_tsap, remote_tsap)
    }

    /// ### Connects to a Siemens ISO-Hardware using TSAP records
    ///
    /// This is the deepest connection method, you will need it only to connect to LOGO! or S7200.
    /// It's internally called by all other connection methods.
    ///
    /// ### Parameters
    /// - `ip` : PLC IPV4 address.
    /// - `local_tsap` : Client TSAP.
    /// - `remote_tsap` : Server TSAP (PLC).
    ///
    /// ### Notes
    /// 1. The connection port used is 102 (S7Protocol Port) unless you
    ///    changed it via `set_connection_port()`
    /// 2. If the client is already connected, the old connection is closed first.
    ///
    /// ### Returns
    /// `Ok(())` on success, or an `S7Error` on failure.
    ///
    /// ### Errors
    /// - `S7Error::TcpConnectionFailed`: the address did not resolve.
    /// - `S7Error::IsoConnectionFailed`: ISO connection refused by the peer.
    /// - `S7Error::PduNegotiationFailed`: PDU negotiation refused, or the CPU
    ///   proposed a PDU length outside `29..=480`.
    /// - `S7Error::Io`, `S7Error::Timeout`, `S7Error::ConnectionClosed`: network failure.
    ///
    pub fn connect_tsap(&mut self, ip: &str, local_tsap: u16, remote_tsap: u16) -> Result<(), S7Error> {
        self.disconnect();
        self.last_time = 0.0;
        self.pdu_length = 0;
        let start_time = Instant::now();

        let (stream, pdu_length) = self.handshake(ip, local_tsap, remote_tsap)?;

        self.pdu_length = pdu_length;
        self.max_rd_pdu_data = pdu_length - READ_RES_OVERHEAD;
        self.max_wr_pdu_data = pdu_length - WRITE_REQ_OVERHEAD;
        self.stream = Some(stream);
        self.last_time = start_time.elapsed().as_secs_f64() * 1000.0;

        Ok(())
    }

    /// TCP connect, ISO connection request and S7 PDU negotiation.
    fn handshake(&mut self, ip: &str, local_tsap: u16, remote_tsap: u16) -> Result<(TcpStream, u16), S7Error> {
        let addr = format!("{}:{}", ip, self.port);
        let co_timeout = Duration::from_millis(self.co_timeout_ms);
        let rd_timeout = Duration::from_millis(self.rd_timeout_ms);
        let wr_timeout = Duration::from_millis(self.wr_timeout_ms);

        let addr = addr.to_socket_addrs()?.next().ok_or(S7Error::TcpConnectionFailed)?;
        let mut stream = TcpStream::connect_timeout(&addr, co_timeout)?;

        stream.set_read_timeout(Some(rd_timeout))?;
        stream.set_write_timeout(Some(wr_timeout))?;
        stream.set_nodelay(true)?;

        // ISO-on-TCP handshake
        let iso_cr: [u8; ISO_CR_LEN] = [
            // TPKT (RFC1006 Header)
            ISO_ID,               // RFC 1006 ID (3)
            0x00,                 // Reserved, always 0
            hi_part!(ISO_CR_LEN), // High part of packet length (entire frame, payload and TPDU included)
            lo_part!(ISO_CR_LEN), // Low part of packet length (entire frame, payload and TPDU included)
            // COTP (ISO 8073 Header)
            0x11,                 // PDU Size Length
            ISO_CONN_REQ,         // CR - Connection Request ID
            0x00,                 // Dst Reference HI
            0x00,                 // Dst Reference LO
            0x00,                 // Src Reference HI
            0x01,                 // Src Reference LO
            0x00,                 // Class + Options Flags
            0xC0,                 // PDU Max Length ID
            0x01,                 // PDU Max Length HI
            0x0A,                 // PDU Max Length LO
            0xC1,                 // Src TSAP Identifier
            0x02,                 // Src TSAP Length (2 bytes)
            hi_part!(local_tsap), // Loc TSAP HI
            lo_part!(local_tsap), // Loc TSAP LO
            0xC2,                 // Rem TSAP Identifier
            0x02,                 // Rem TSAP Length (2 bytes)
            hi_part!(remote_tsap), // Rem TSAP HI
            lo_part!(remote_tsap), // Rem TSAP LO
        ];

        stream.write_all(&iso_cr)?;

        let mut iso_cc = Vec::with_capacity(ISO_CR_LEN);
        recv_cotp_frame(&mut stream, &mut iso_cc)?;
        if iso_cc[5] != ISO_CONN_OK {
            return Err(S7Error::IsoConnectionFailed);
        }

        // S7 PDU Negotiation Telegram (contains also ISO Header and COTP Header)
        let pdu_ref = self.next_ref();
        let s7_pn: [u8; ISO_PN_REQ_LEN] = [
            ISO_ID,
            0x00,
            0x00, 0x19,
            0x02, 0xf0, 0x80,
            S7_ID, 0x01, 0x00, 0x00, hi_part!(pdu_ref), lo_part!(pdu_ref), 0x00, 0x08, 0x00,
            0x00, FN_SETUP_COMM, 0x00, 0x00, 0x01, 0x00, 0x01,
            hi_part!(PDU_LEN_REQ),
            lo_part!(PDU_LEN_REQ),
        ];
        stream.write_all(&s7_pn)?;

        let n = match recv_ack(&mut stream, PDU_LEN_REQ, &mut self.rx_buf, pdu_ref, FN_SETUP_COMM, S7_PN_RES_LEN) {
            Ok(n) => n,
            Err(S7Error::S7InvalidResponse | S7Error::S7Unspecified) => return Err(S7Error::PduNegotiationFailed),
            Err(e) => return Err(e),
        };
        let pn = &self.rx_buf[..n];
        let pdu_length = make_u16!(pn[18], pn[19]);

        if !(PDU_LEN_MIN..=PDU_LEN_REQ).contains(&pdu_length) {
            return Err(S7Error::PduNegotiationFailed);
        }

        Ok((stream, pdu_length))
    }

    /// ### Closes the connection.
    ///
    /// Safe to call even if the client is not currently connected.
    /// After disconnection, calls to read/write will return `S7Error::NotConnected`.
    ///
    /// ### Notes
    /// The client disconnects by itself on every transport or protocol error
    /// (see `S7Error::requires_reconnect()`), so you only need this method to
    /// close a healthy connection.
    ///
    pub fn disconnect(&mut self) {
        if let Some(stream) = self.stream.take() {
            // If we are disconnecting on a low-level error it's better to flush the socket
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// ### Reads a block of data from a specific S7 memory area.
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `start`: Starting element index (byte index for bytes, bit index for bits).
    /// - `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
    /// - `buffer`: Destination buffer to store the read data (at most 65535 bytes).
    ///
    /// ### Values
    /// #### area
    /// - `S7_AREA_PE` (0x81): Process Inputs
    /// - `S7_AREA_PA` (0x82): Process Outputs
    /// - `S7_AREA_MK` (0x83): Merkers
    /// - `S7_AREA_DB` (0x84): Data Block
    /// #### wordlen
    /// - `S7_WL_BIT` (0x01) : Bit access
    /// - `S7_WL_BYTE` (0x02): Byte access
    /// #### Bit access notes
    /// 1. The start must be expressed in bits.
    ///    For example, if you want to access bit `DBX 45.3`, the start value would be 45 * 8 + 3 = 363.
    /// 2. The buffer must hold at least one byte; only the first byte is used, which is considered true if !=0 or false if ==0
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded: `buffer` holds exactly what the CPU sent for this request.
    ///
    /// ### Errors
    /// #### Transport / protocol (the client disconnects, see `S7Error::requires_reconnect()`)
    /// - `S7Error::Io`, `S7Error::Timeout`, `S7Error::ConnectionClosed`: network failure.
    /// - `S7Error::IsoInvalidHeader`, `S7Error::IsoInvalidTelegram`, `S7Error::IsoFragmentedPacket`: bad framing.
    /// - `S7Error::S7InvalidResponse`: the reply does not match the request (reference, function, item size).
    ///
    /// #### Operation level (still connected)
    /// - `S7Error::NotConnected`: An attempt was made to read while the client was not connected.
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    /// - `S7Error::S7NotFound`: The resource was not found (e.g. Inexistent DB).
    /// - `S7Error::S7InvalidAddress`:
    ///   1. Attempt to read beyond the limits.
    ///   2. The DB is optimized.
    /// - `S7Error::S7Unspecified`: the CPU rejected the job with another code.
    ///
    /// ### Notes
    /// - The number of bytes to read will be equal to the size of the buffer passed.
    /// - Large blocks are automatically split into chunks based on the negotiated PDU size.
    /// - In case of error the buffer contents will be inconsistent and should not be considered.
    ///
    pub fn read_area(&mut self, area: u8, db_number: u16, start: u16, wordlen: u8, buffer: &mut [u8]) -> Result<(), S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;

        self.check_area(area)?;
        let datasize = Self::check_transfer_size(wordlen, buffer.len())?;
        if !self.is_connected() {
            return Err(S7Error::NotConnected);
        }

        let start_time = Instant::now();
        let result = self.read_chunks(area, db_number, start, wordlen, datasize, buffer);
        let result = self.settle(result);
        if result.is_ok() {
            self.last_time = start_time.elapsed().as_secs_f64() * 1000.0;
        }
        result
    }

    /// Validates `wordlen` against the buffer and returns the number of elements to transfer.
    fn check_transfer_size(wordlen: u8, buffer_len: usize) -> Result<u16, S7Error> {
        match wordlen {
            S7_WL_BYTE => u16::try_from(buffer_len).map_err(|_| S7Error::InvalidFunParameter),
            S7_WL_BIT if buffer_len >= 1 => Ok(1), // Only 1 element allowed for bit operations
            _ => Err(S7Error::InvalidFunParameter),
        }
    }

    fn read_chunks(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        datasize: u16,
        buffer: &mut [u8],
    ) -> Result<(), S7Error> {
        let mut offset: u16 = 0;
        let mut long_start: u32 = start as u32;

        while offset < datasize {
            let remaining = datasize - offset;
            let chunk_size = remaining.min(self.max_rd_pdu_data);
            self.chunks += 1;
            let pdu_ref = self.next_ref();

            // Read Request Header
            let mut request: [u8; READ_REQ_LEN] = [
                ISO_ID, 0x00,         // RFC 1006 ID (constant)                   0
                0x00, 0x1f,           // Telegram Length (31)                     2
                0x02, 0xf0, 0x80,     // COPT (constant)                          4
                S7_ID,                // S7 Protocol ID                           7
                0x01,                 // Job Type (Data)                          8
                0x00, 0x00,           // Redundancy identification                9
                hi_part!(pdu_ref),    // PDU Reference HI                         11
                lo_part!(pdu_ref),    // PDU Reference LO                         12
                0x00, 0x0e,           // Parameters Length (HI,LO) = 14           13
                0x00, 0x00,           // No write Payload here : 0                15
                FN_READ_VAR,          // Function: 4 Read Var, 5 Write Var        17
                0x01,                 // Items count (used for multivar R/W)      18
                0x12,                 // Var spec.                                19
                0x0a,                 // constant 0x0a                            20
                0x10,                 // Syntax ID                                21
                wordlen,              // WordLen                                  22
                hi_part!(chunk_size), // HI (Read Payload Size)                   23
                lo_part!(chunk_size), // LO (Read Payload Size)                   24
                hi_part!(db_number),  // HI DB Number                             25
                lo_part!(db_number),  // LO DB Number                             26
                area,                 // Area                                     27
                0x00, 0x00, 0x00,     // 24 bit Address (see below)               28
            ];

            let address = if wordlen == S7_WL_BIT { long_start } else { long_start << 3 };

            request[28] = ((address >> 16) & 0xFF) as u8;
            request[29] = ((address >> 8) & 0xFF) as u8;
            request[30] = (address & 0xFF) as u8;

            let pdu_length = self.pdu_length;
            let stream = self.stream.as_mut().ok_or(S7Error::NotConnected)?;
            stream.write_all(&request)?;

            let n = recv_ack(stream, pdu_length, &mut self.rx_buf, pdu_ref, FN_READ_VAR, READ_RES_LEN)?;
            let s7 = &self.rx_buf[..n];

            //  Read item
            //      [14]     Return code
            //      [15]     Transport size
            //      [16..18] Length (bits or bytes, depending on transport size)
            //      [18..]   Payload
            item_result(s7[RW_RES_OFFSET])?;

            let expected = if wordlen == S7_WL_BIT { 1 } else { chunk_size as usize };
            let declared = item_payload_len(s7[15], make_u16!(s7[16], s7[17]));
            if declared != Some(expected) || n < READ_RES_LEN + expected {
                return Err(S7Error::S7InvalidResponse);
            }

            let dst = offset as usize;
            buffer[dst..dst + expected].copy_from_slice(&s7[READ_RES_LEN..READ_RES_LEN + expected]);

            offset += chunk_size;
            long_start += chunk_size as u32;
        }

        Ok(())
    }

    /// ### Writes a block of data to a specific S7 memory area.
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `start`: Starting element index (byte index for bytes, bit index for bits).
    /// - `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
    /// - `buffer`: Source buffer to write (at most 65535 bytes).
    ///
    /// ### Values
    /// #### area
    /// - `S7_AREA_PE` (0x81): Process Inputs
    /// - `S7_AREA_PA` (0x82): Process Outputs
    /// - `S7_AREA_MK` (0x83): Merkers
    /// - `S7_AREA_DB` (0x84): Data Block
    /// #### wordlen
    /// - `S7_WL_BIT` (0x01) : Bit access
    /// - `S7_WL_BYTE` (0x02): Byte access
    /// #### Bit access notes
    /// 1. The start must be expressed in bits.
    ///    For example, if you want to access bit `DBX 45.3`, the start value would be 45 * 8 + 3 = 363.
    /// 2. The buffer must hold at least one byte; only the first byte is used, which is considered true if !=0 or false if ==0
    /// 3. Writing a bit affects **only that bit**, leaving adjacent bits in the byte unchanged.
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded: every byte was acknowledged by the CPU.
    ///
    /// ### Errors
    /// #### Before anything is sent (still connected)
    /// - `S7Error::NotConnected`: An attempt was made to write while the client was not connected.
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    ///
    /// #### CPU rejected the first chunk (nothing applied, still connected)
    /// - `S7Error::S7NotFound`: The resource was not found (e.g. Inexistent DB).
    /// - `S7Error::S7InvalidAddress`:
    ///   1. Attempt to write beyond the limits.
    ///   2. The DB is optimized.
    /// - `S7Error::S7Unspecified`: the CPU rejected the job with another code.
    ///
    /// #### Anything else after data hit the wire
    /// - `S7Error::WriteIncomplete { acknowledged, requested, source }`: `acknowledged`
    ///   leading bytes were confirmed by the CPU. When `source` is a transport error
    ///   (`Io`, `Timeout`, `ConnectionClosed`, `Iso*`, `S7InvalidResponse`) the chunk in
    ///   flight **may or may not** have been applied and the client has disconnected;
    ///   when `source` is a PLC rejection it was not applied and the client stays connected.
    ///
    /// ### Notes
    /// - The number of bytes to write will be equal to the size of the buffer passed.
    /// - Large blocks are automatically split into chunks based on the negotiated PDU size.
    /// - Writing the output buffer (`S7_AREA_PA`) usually does not produce useful results, in fact the output process image
    ///   will be rewritten by OB1 in the next round
    ///
    pub fn write_area(&mut self, area: u8, db_number: u16, start: u16, wordlen: u8, buffer: &[u8]) -> Result<(), S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;

        self.check_area(area)?;
        let datasize = Self::check_transfer_size(wordlen, buffer.len())? as usize;
        if !self.is_connected() {
            return Err(S7Error::NotConnected);
        }

        let start_time = Instant::now();
        let result = self.write_chunks(area, db_number, start, wordlen, datasize, buffer);
        let result = self.settle(result);
        if result.is_ok() {
            self.last_time = start_time.elapsed().as_secs_f64() * 1000.0;
        }
        result
    }

    fn write_chunks(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        datasize: usize,
        buffer: &[u8],
    ) -> Result<(), S7Error> {
        let mut offset = 0;
        let mut long_start: u32 = start as u32;
        let transport: u8 = if wordlen == S7_WL_BIT { TS_RES_BIT } else { TS_RES_BYTE };

        while offset < datasize {
            self.chunks += 1;
            let chunk_size = (datasize - offset).min(self.max_wr_pdu_data as usize);
            let chunk = &buffer[offset..offset + chunk_size];
            let pdu_ref = self.next_ref();

            let bits_payload: u16 = if wordlen == S7_WL_BIT { 1 } else { (chunk_size << 3) as u16 };

            // 35 byte Write Request Header
            let mut request = vec![
                ISO_ID, 0x00,             // RFC 1006 ID (constant)
                0x00, 0x00,               // Telegram Length (HI,LO) = Payload Size + 35
                0x02, 0xf0, 0x80,         // COPT (constant)
                S7_ID,                    // S7 Protocol ID
                0x01,                     // Job Type (Data)
                0x00, 0x00,               // Redundancy identification
                hi_part!(pdu_ref),        // PDU Reference HI
                lo_part!(pdu_ref),        // PDU Reference LO
                0x00, 0x0e,               // Parameters Length (HI,LO) = 14
                hi_part!(chunk_size + 4), // HI (Payload Size + 4)
                lo_part!(chunk_size + 4), // LO (Payload Size + 4)
                FN_WRITE_VAR,             // Function: 4 Read Var, 5 Write Var
                0x01,                     // Items count (used for multivar R/W)
                0x12,                     // Var spec.
                0x0a,                     // constant 0x0a
                0x10,                     // Syntax ID
                wordlen,
                hi_part!(chunk_size),     // HI Payload size
                lo_part!(chunk_size),     // LO Payload size
                hi_part!(db_number),      // HI DB Number
                lo_part!(db_number),      // LO DB Number
                area,                     // Area ID
                0x00, 0x00, 0x00,         // 24 bit Address (see below)
                0x00,                     // Reserved
                transport,                // TS_RES_BIT or TS_RES_BYTE
                hi_part!(bits_payload),   // HI Payload size (bits)
                lo_part!(bits_payload),   // LO Payload size (bits)
            ];

            request.extend_from_slice(chunk); // Append the Payload to the Header

            let total_len = request.len();

            // Set Telegram length
            request[2] = hi_part!(total_len);
            request[3] = lo_part!(total_len);

            // Set Start Address (bits) inside the area
            let address = if wordlen == S7_WL_BIT { long_start } else { long_start << 3 };

            request[28] = ((address >> 16) & 0xFF) as u8;
            request[29] = ((address >> 8) & 0xFF) as u8;
            request[30] = (address & 0xFF) as u8;

            let pdu_length = self.pdu_length;
            let stream = self.stream.as_mut().ok_or(S7Error::NotConnected)?;
            let outcome = match stream.write_all(&request) {
                Err(e) => Err(S7Error::from(e)),
                Ok(()) => recv_ack(stream, pdu_length, &mut self.rx_buf, pdu_ref, FN_WRITE_VAR, WRITE_RES_LEN)
                    .and_then(|_| item_result(self.rx_buf[RW_RES_OFFSET])),
            };

            if let Err(e) = outcome {
                return Err(Self::write_failure(e, offset, datasize));
            }

            // Next Chunk
            offset += chunk_size;
            long_start += chunk_size as u32;
        }

        Ok(())
    }

    /// Wraps a failure that happened after data was put on the wire, unless it
    /// is a definitive rejection of the very first chunk (nothing applied).
    fn write_failure(e: S7Error, acknowledged: usize, requested: usize) -> S7Error {
        if acknowledged == 0 && !e.requires_reconnect() {
            e
        } else {
            S7Error::WriteIncomplete { acknowledged, requested, source: Box::new(e) }
        }
    }

    /// ### Reads a block of byte from a specific Data Block (DB)
    ///
    /// This helper method is same as `read_area()` with:
    /// - area = `S7_AREA_DB`
    /// - wordlen = `S7_WL_BYTE`
    ///
    /// ### Parameters
    /// - `db_number`: DB number
    /// - `start`: Starting byte index
    /// - `buffer`: Destination buffer to store the read data.
    ///
    /// ### Notes
    /// - The number of bytes to read will be equal to the size of the buffer passed.
    /// ---
    /// For further info, please refer to `read_area()`
    ///
    pub fn read_db(&mut self, db_number: u16, start: u16, buffer: &mut [u8]) -> Result<(), S7Error> {
        self.read_area(S7_AREA_DB, db_number, start, S7_WL_BYTE, buffer)
    }

    /// ### Reads a bit from a specific S7 memory area
    ///
    /// This helper method is same as `read_area()` with:
    /// - wordlen = `S7_WL_BIT`
    /// - start = `byte_num * 8 + bit_idx`
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `byte_num`: Byte Number (0..8191, so that the bit address fits 16 bits).
    /// - `bit_idx`: Bit index inside the byte (0..7).
    ///
    /// ### Example
    /// To read DB10.DBX71.4 use:
    ///
    /// ```text
    /// my_bit = read_bit(S7_AREA_DB, 10, 71, 4);
    /// ```
    ///
    /// ### Returns
    /// `Ok(<bool>)` or `Err(<S7Error>)`
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    /// - Other reported by read_area()
    ///
    /// ### Suggestion
    /// Even reading a single bit requires an entire telegram.
    /// Since reading is non-invasive, if you need to read multiple bits
    /// (more or less adjacent in the same area), I recommend reading blocks
    /// of bytes and then unpacking them.
    /// ---
    /// For further info, please refer to `read_area()`
    ///
    pub fn read_bit(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8) -> Result<bool, S7Error> {
        let start = bit_start(byte_num, bit_idx)?;
        let mut buffer = [0u8; 1];

        self.read_area(area, db_number, start, S7_WL_BIT, &mut buffer)?;

        Ok(buffer[0] != 0)
    }

    /// ### Writes a block of byte to a specific Data Block (DB)
    ///
    /// This helper method is same as `write_area()` with:
    /// - area = `S7_AREA_DB`
    /// - wordlen = `S7_WL_BYTE`
    ///
    /// ### Parameters
    /// - `db_number`: DB number
    /// - `start`: Starting byte index
    /// - `buffer`: Source buffer to write.
    ///
    /// ### Notes
    /// - The number of bytes to write will be equal to the size of the buffer passed.
    /// ---
    /// For further info, please refer to `write_area()`
    ///
    pub fn write_db(&mut self, db_number: u16, start: u16, buffer: &[u8]) -> Result<(), S7Error> {
        self.write_area(S7_AREA_DB, db_number, start, S7_WL_BYTE, buffer)
    }

    /// ### Writes a bit to a specific S7 memory area
    ///
    /// This helper method is same as `write_area()` with:
    /// - wordlen = `S7_WL_BIT`
    /// - start = `byte_num * 8 + bit_idx`
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `byte_num`: Byte Number (0..8191, so that the bit address fits 16 bits).
    /// - `bit_idx`: Bit index inside the byte (0..7).
    /// - `value`: Value to write (true | false).
    ///
    /// ### Example
    /// To write **1** into DB10.DBX71.4 use:
    ///
    /// ```text
    /// write_bit(S7_AREA_DB, 10, 71, 4, true);
    /// ```
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded.
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParameter`: Invalid parameter supplied to the function.
    /// - Other reported by write_area()
    ///
    /// ### Notes
    /// Writing a bit affects only that bit, leaving adjacent bits in the byte unchanged.
    /// ---
    /// For further info, please refer to `write_area()`
    ///
    pub fn write_bit(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8, value: bool) -> Result<(), S7Error> {
        let start = bit_start(byte_num, bit_idx)?;
        let data = [value as u8];

        self.write_area(area, db_number, start, S7_WL_BIT, &data)
    }
}

impl Drop for S7Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}

// ---------------------------------------------------------------------------
// Unit tests for the private building blocks (macros, ISO header validation,
// parameter validation). Network behaviour is covered by the integration tests
// in `tests/` against an in-process fake PLC.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ---- byte helpers -----------------------------------------------------

    #[test]
    fn hi_lo_split_and_make_u16_agree_on_known_values() {
        assert_eq!(hi_part!(0x1234u16), 0x12);
        assert_eq!(lo_part!(0x1234u16), 0x34);
        assert_eq!(make_u16!(0x12u8, 0x34u8), 0x1234);
        assert_eq!(make_u16!(0xFFu8, 0xFFu8), u16::MAX);
        assert_eq!(make_u16!(0x00u8, 0x00u8), 0);
        // usize inputs (used for telegram lengths) must work too
        assert_eq!(hi_part!(ISO_CR_LEN), 0x00);
        assert_eq!(lo_part!(ISO_CR_LEN), 22);
        assert_eq!(hi_part!(0x1FFusize), 0x01);
        assert_eq!(lo_part!(0x1FFusize), 0xFF);
    }

    proptest! {
        #[test]
        fn make_u16_inverts_hi_lo_split(x in any::<u16>()) {
            prop_assert_eq!(make_u16!(hi_part!(x), lo_part!(x)), x);
        }

        #[test]
        fn hi_lo_are_the_big_endian_bytes(x in any::<u16>()) {
            let be = x.to_be_bytes();
            prop_assert_eq!(hi_part!(x), be[0]);
            prop_assert_eq!(lo_part!(x), be[1]);
        }
    }

    // ---- check_iso_packet -------------------------------------------------

    fn iso_header(telegram_len: u16) -> [u8; TPKT_ISO_LEN] {
        [ISO_ID, 0x00, hi_part!(telegram_len), lo_part!(telegram_len), 0x02, 0xF0, EOT]
    }

    #[test]
    fn iso_packet_returns_remaining_bytes_for_valid_header() {
        assert_eq!(check_iso_packet(240, &iso_header(25)).unwrap(), 18);
    }

    #[test]
    fn iso_packet_accepts_payload_exactly_filling_pdu() {
        assert_eq!(check_iso_packet(240, &iso_header(TPKT_ISO_LEN as u16 + 240)).unwrap(), 240);
    }

    #[test]
    fn iso_packet_rejects_payload_exceeding_pdu() {
        let pkt = iso_header(TPKT_ISO_LEN as u16 + 241);
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidTelegram)));
    }

    #[test]
    fn iso_packet_rejects_empty_payload() {
        let pkt = iso_header(TPKT_ISO_LEN as u16);
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidTelegram)));
    }

    #[test]
    fn iso_packet_rejects_length_below_header_size_without_underflow() {
        for len in 0..TPKT_ISO_LEN as u16 {
            let pkt = iso_header(len);
            assert!(
                matches!(check_iso_packet(u16::MAX, &pkt), Err(S7Error::IsoInvalidTelegram)),
                "telegram length {len} must be rejected"
            );
        }
    }

    #[test]
    fn iso_packet_rejects_wrong_rfc1006_id() {
        let mut pkt = iso_header(25);
        pkt[0] = 0x04;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidHeader)));
    }

    #[test]
    fn iso_packet_rejects_wrong_cotp_length() {
        let mut pkt = iso_header(25);
        pkt[4] = 0x03;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidHeader)));
    }

    #[test]
    fn iso_packet_rejects_wrong_cotp_pdu_type() {
        let mut pkt = iso_header(25);
        pkt[5] = 0xE0;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidHeader)));
    }

    #[test]
    fn iso_packet_ignores_reserved_byte() {
        let mut pkt = iso_header(25);
        pkt[1] = 0xAB;
        assert_eq!(check_iso_packet(240, &pkt).unwrap(), 18);
    }

    #[test]
    fn iso_packet_reports_fragmentation_when_eot_is_missing() {
        let mut pkt = iso_header(25);
        pkt[6] = 0x00;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoFragmentedPacket)));
    }

    #[test]
    fn iso_packet_header_error_takes_precedence_over_fragmentation() {
        let mut pkt = iso_header(25);
        pkt[0] = 0x00;
        pkt[6] = 0x00;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoInvalidHeader)));
    }

    #[test]
    fn iso_packet_fragmentation_takes_precedence_over_bad_length() {
        let mut pkt = iso_header(0);
        pkt[6] = 0x00;
        assert!(matches!(check_iso_packet(240, &pkt), Err(S7Error::IsoFragmentedPacket)));
    }

    /// Executable specification of `check_iso_packet`, used as an oracle.
    #[derive(Debug, PartialEq, Eq)]
    enum Outcome {
        Ok(usize),
        InvalidHeader,
        Fragmented,
        InvalidTelegram,
    }

    fn model(pdu_length: u16, p: &[u8; TPKT_ISO_LEN]) -> Outcome {
        if p[0] != ISO_ID || p[4] != 0x02 || p[5] != 0xF0 {
            return Outcome::InvalidHeader;
        }
        if p[6] != EOT {
            return Outcome::Fragmented;
        }
        let len = u16::from_be_bytes([p[2], p[3]]) as usize;
        match len.checked_sub(TPKT_ISO_LEN) {
            Some(payload) if payload >= 1 && payload <= pdu_length as usize => Outcome::Ok(payload),
            _ => Outcome::InvalidTelegram,
        }
    }

    fn outcome(r: Result<usize, S7Error>) -> Outcome {
        match r {
            Ok(n) => Outcome::Ok(n),
            Err(S7Error::IsoInvalidHeader) => Outcome::InvalidHeader,
            Err(S7Error::IsoFragmentedPacket) => Outcome::Fragmented,
            Err(S7Error::IsoInvalidTelegram) => Outcome::InvalidTelegram,
            Err(e) => panic!("check_iso_packet must never return {e:?}"),
        }
    }

    /// Mostly-valid headers, so that all branches get exercised instead of
    /// almost always failing on the first check.
    fn iso_packet_strategy() -> impl Strategy<Value = [u8; TPKT_ISO_LEN]> {
        (
            prop_oneof![3 => Just(ISO_ID), 1 => any::<u8>()],
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            prop_oneof![3 => Just(0x02u8), 1 => any::<u8>()],
            prop_oneof![3 => Just(0xF0u8), 1 => any::<u8>()],
            prop_oneof![3 => Just(EOT), 1 => any::<u8>()],
        )
            .prop_map(|(a, b, c, d, e, f, g)| [a, b, c, d, e, f, g])
    }

    proptest! {
        #[test]
        fn iso_packet_matches_reference_model(pdu in any::<u16>(), pkt in iso_packet_strategy()) {
            let expected = model(pdu, &pkt);
            prop_assert_eq!(outcome(check_iso_packet(pdu, &pkt)), expected);
        }

        #[test]
        fn iso_packet_never_panics_on_arbitrary_input(pdu in any::<u16>(), pkt in any::<[u8; TPKT_ISO_LEN]>()) {
            let _ = check_iso_packet(pdu, &pkt);
        }

        #[test]
        fn valid_header_is_ok_iff_payload_within_1_and_pdu(pdu in any::<u16>(), len in any::<u16>()) {
            let pkt = iso_header(len);
            let result = check_iso_packet(pdu, &pkt);
            match (len as usize).checked_sub(TPKT_ISO_LEN) {
                Some(payload) if payload >= 1 && payload <= pdu as usize => {
                    prop_assert_eq!(result.unwrap(), payload);
                }
                _ => prop_assert!(matches!(result, Err(S7Error::IsoInvalidTelegram))),
            }
        }
    }

    // ---- item / address helpers --------------------------------------------

    #[test]
    fn item_payload_len_follows_the_transport_size_semantics() {
        assert_eq!(item_payload_len(TS_RES_BIT, 1), Some(1));
        assert_eq!(item_payload_len(TS_RES_OCTET, 17), Some(17));
        assert_eq!(item_payload_len(TS_RES_BYTE, 32), Some(4));
        assert_eq!(item_payload_len(TS_RES_INT, 16), Some(2));
        assert_eq!(item_payload_len(TS_RES_DINT, 32), Some(4));
        assert_eq!(item_payload_len(TS_RES_REAL, 32), Some(4));
        assert_eq!(item_payload_len(TS_RES_BYTE, 12), None, "bit length not byte aligned");
        assert_eq!(item_payload_len(0x00, 8), None);
        assert_eq!(item_payload_len(0xFF, 8), None);
    }

    #[test]
    fn bit_start_encodes_byte_times_8_plus_bit() {
        assert_eq!(bit_start(45, 3).unwrap(), 363);
        assert_eq!(bit_start(0, 0).unwrap(), 0);
        assert_eq!(bit_start(8191, 7).unwrap(), u16::MAX);
    }

    #[test]
    fn bit_start_rejects_bit_index_above_7() {
        for bit in 8u8..=u8::MAX {
            assert!(matches!(bit_start(0, bit), Err(S7Error::InvalidFunParameter)), "bit {bit}");
        }
    }

    proptest! {
        #[test]
        fn bit_start_never_wraps(byte_num in any::<u16>(), bit in 0u8..8) {
            match bit_start(byte_num, bit) {
                Ok(start) => prop_assert_eq!(start as u32, byte_num as u32 * 8 + bit as u32),
                Err(S7Error::InvalidFunParameter) => prop_assert!(byte_num as u32 * 8 + bit as u32 > u16::MAX as u32),
                Err(e) => prop_assert!(false, "unexpected {e:?}"),
            }
        }
    }

    #[test]
    fn transfer_size_is_buffer_len_for_bytes_and_one_for_bits() {
        assert_eq!(S7Client::check_transfer_size(S7_WL_BYTE, 0).unwrap(), 0);
        assert_eq!(S7Client::check_transfer_size(S7_WL_BYTE, 65535).unwrap(), 65535);
        assert_eq!(S7Client::check_transfer_size(S7_WL_BIT, 1).unwrap(), 1);
        assert_eq!(S7Client::check_transfer_size(S7_WL_BIT, 9).unwrap(), 1);
    }

    #[test]
    fn transfer_size_rejects_oversized_empty_bit_and_unknown_wordlen() {
        assert!(matches!(S7Client::check_transfer_size(S7_WL_BYTE, 65536), Err(S7Error::InvalidFunParameter)));
        assert!(matches!(S7Client::check_transfer_size(S7_WL_BIT, 0), Err(S7Error::InvalidFunParameter)));
        for wordlen in [0x00u8, 0x03, 0x04, 0xFF] {
            assert!(matches!(S7Client::check_transfer_size(wordlen, 4), Err(S7Error::InvalidFunParameter)));
        }
    }

    #[test]
    fn next_ref_increments_and_skips_zero_on_wrap() {
        let mut c = S7Client::new();
        assert_eq!(c.next_ref(), 1);
        assert_eq!(c.next_ref(), 2);
        c.pdu_ref = u16::MAX;
        assert_eq!(c.next_ref(), 1, "0 is skipped so a reference is never 'unset'");
    }

    #[test]
    fn write_failure_wraps_everything_except_a_first_chunk_plc_rejection() {
        assert!(matches!(S7Client::write_failure(S7Error::S7NotFound, 0, 10), S7Error::S7NotFound));
        assert!(matches!(
            S7Client::write_failure(S7Error::S7NotFound, 5, 10),
            S7Error::WriteIncomplete { acknowledged: 5, requested: 10, .. }
        ));
        assert!(matches!(
            S7Client::write_failure(S7Error::Timeout, 0, 10),
            S7Error::WriteIncomplete { acknowledged: 0, requested: 10, .. }
        ));
    }

    // ---- constructor & configuration -------------------------------------

    #[test]
    fn new_client_has_documented_defaults() {
        let c = S7Client::new();
        assert!(!c.is_connected());
        assert!(c.stream.is_none());
        assert_eq!(c.port, 102);
        assert_eq!(c.co_timeout_ms, 3000);
        assert_eq!(c.rd_timeout_ms, 1000);
        assert_eq!(c.wr_timeout_ms, 500);
        assert_eq!(c.conn_type, CT_PG);
        assert_eq!(c.pdu_length(), 0);
        assert_eq!(c.max_rd_pdu_data, 0);
        assert_eq!(c.max_wr_pdu_data, 0);
        assert_eq!(c.last_time(), 0.0);
        assert_eq!(c.chunks(), 0);
        assert_eq!(c.rx_buf.len(), PDU_LEN_REQ as usize);
    }

    #[test]
    fn default_is_the_same_as_new() {
        let c = S7Client::default();
        assert_eq!((c.port, c.co_timeout_ms, c.rd_timeout_ms, c.wr_timeout_ms), (102, 3000, 1000, 500));
    }

    #[test]
    fn set_connection_type_accepts_every_documented_type() {
        let mut c = S7Client::new();
        for ct in [CT_PG, CT_OP, CT_S7] {
            assert!(c.set_connection_type(ct).is_ok());
            assert_eq!(c.conn_type, ct);
        }
    }

    #[test]
    fn set_connection_type_rejects_out_of_range_and_keeps_previous_value() {
        let mut c = S7Client::new();
        c.set_connection_type(CT_OP).unwrap();
        for bad in [0u16, 4, 0x0100, u16::MAX] {
            assert!(
                matches!(c.set_connection_type(bad), Err(S7Error::InvalidFunParameter)),
                "connection type {bad:#06x} must be rejected"
            );
            assert_eq!(c.conn_type, CT_OP);
        }
    }

    #[test]
    fn set_timeout_stores_all_three_values() {
        let mut c = S7Client::new();
        c.set_timeout(1, 2, 3).unwrap();
        assert_eq!((c.co_timeout_ms, c.rd_timeout_ms, c.wr_timeout_ms), (1, 2, 3));
    }

    #[test]
    fn set_timeout_rejects_any_zero_and_changes_nothing() {
        let mut c = S7Client::new();
        for (co, rd, wr) in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (0, 0, 0)] {
            assert!(matches!(c.set_timeout(co, rd, wr), Err(S7Error::InvalidFunParameter)));
            assert_eq!((c.co_timeout_ms, c.rd_timeout_ms, c.wr_timeout_ms), (3000, 1000, 500));
        }
    }

    #[test]
    fn set_connection_port_stores_value() {
        let mut c = S7Client::new();
        c.set_connection_port(1102).unwrap();
        assert_eq!(c.port, 1102);
        c.set_connection_port(u16::MAX).unwrap();
        assert_eq!(c.port, u16::MAX);
    }

    #[test]
    fn set_connection_port_rejects_zero_and_keeps_previous_value() {
        let mut c = S7Client::new();
        assert!(matches!(c.set_connection_port(0), Err(S7Error::InvalidFunParameter)));
        assert_eq!(c.port, 102);
    }

    #[test]
    fn check_area_accepts_only_the_four_documented_areas() {
        let c = S7Client::new();
        for area in u8::MIN..=u8::MAX {
            let expected_ok = [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB].contains(&area);
            assert_eq!(c.check_area(area).is_ok(), expected_ok, "area {area:#04x}");
        }
    }

    proptest! {
        #[test]
        fn set_connection_type_is_valid_iff_between_pg_and_s7(ct in any::<u16>()) {
            let mut c = S7Client::new();
            prop_assert_eq!(c.set_connection_type(ct).is_ok(), (CT_PG..=CT_S7).contains(&ct));
        }

        #[test]
        fn set_timeout_is_valid_iff_all_values_are_nonzero(co in any::<u64>(), rd in any::<u64>(), wr in any::<u64>()) {
            let mut c = S7Client::new();
            prop_assert_eq!(c.set_timeout(co, rd, wr).is_ok(), co != 0 && rd != 0 && wr != 0);
        }

        #[test]
        fn set_connection_port_is_valid_iff_nonzero(port in any::<u16>()) {
            let mut c = S7Client::new();
            prop_assert_eq!(c.set_connection_port(port).is_ok(), port != 0);
        }
    }

    // ---- offline behaviour of the data API --------------------------------

    #[test]
    fn disconnect_on_never_connected_client_is_a_noop() {
        let mut c = S7Client::new();
        c.disconnect();
        c.disconnect();
        assert!(!c.is_connected());
        assert!(c.stream.is_none());
    }

    #[test]
    fn dropping_an_unconnected_client_does_not_panic() {
        drop(S7Client::new());
    }

    #[test]
    fn rack_or_slot_out_of_range_is_rejected_without_resolving_the_address() {
        let mut c = S7Client::new();
        for (rack, slot) in [(8u16, 0u16), (0, 32), (u16::MAX, 0), (0, u16::MAX), (2048, 0)] {
            assert!(
                matches!(c.connect_rack_slot("this host does not exist", rack, slot), Err(S7Error::InvalidFunParameter)),
                "rack {rack} slot {slot}"
            );
        }
    }

    #[test]
    fn read_area_reports_invalid_area_before_checking_connection() {
        let mut c = S7Client::new();
        let mut buf = [0u8; 4];
        assert!(matches!(
            c.read_area(0x00, 1, 0, S7_WL_BYTE, &mut buf),
            Err(S7Error::InvalidFunParameter)
        ));
    }

    #[test]
    fn read_area_reports_invalid_wordlen_before_checking_connection() {
        let mut c = S7Client::new();
        let mut buf = [0u8; 4];
        for wordlen in [0x00u8, 0x03, 0x04, 0xFF] {
            assert!(matches!(
                c.read_area(S7_AREA_DB, 1, 0, wordlen, &mut buf),
                Err(S7Error::InvalidFunParameter)
            ));
        }
    }

    #[test]
    fn read_area_with_valid_parameters_reports_not_connected() {
        let mut c = S7Client::new();
        let mut buf = [0u8; 4];
        for area in [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB] {
            for wordlen in [S7_WL_BIT, S7_WL_BYTE] {
                assert!(matches!(
                    c.read_area(area, 1, 0, wordlen, &mut buf),
                    Err(S7Error::NotConnected)
                ));
            }
        }
    }

    #[test]
    fn write_area_reports_invalid_area_before_checking_connection() {
        let mut c = S7Client::new();
        assert!(matches!(
            c.write_area(0x85, 1, 0, S7_WL_BYTE, &[0u8; 4]),
            Err(S7Error::InvalidFunParameter)
        ));
    }

    #[test]
    fn write_area_reports_invalid_wordlen_before_checking_connection() {
        let mut c = S7Client::new();
        assert!(matches!(
            c.write_area(S7_AREA_DB, 1, 0, 0x03, &[0u8; 4]),
            Err(S7Error::InvalidFunParameter)
        ));
    }

    #[test]
    fn write_area_with_valid_parameters_reports_not_connected() {
        let mut c = S7Client::new();
        assert!(matches!(
            c.write_area(S7_AREA_MK, 0, 0, S7_WL_BYTE, &[0u8; 4]),
            Err(S7Error::NotConnected)
        ));
    }

    #[test]
    fn read_db_and_write_db_delegate_to_area_functions() {
        let mut c = S7Client::new();
        let mut buf = [0u8; 4];
        assert!(matches!(c.read_db(1, 0, &mut buf), Err(S7Error::NotConnected)));
        assert!(matches!(c.write_db(1, 0, &buf), Err(S7Error::NotConnected)));
    }

    #[test]
    fn read_bit_rejects_every_bit_index_above_7() {
        let mut c = S7Client::new();
        for bit in 8u8..=u8::MAX {
            assert!(
                matches!(c.read_bit(S7_AREA_DB, 1, 0, bit), Err(S7Error::InvalidFunParameter)),
                "bit index {bit}"
            );
        }
    }

    #[test]
    fn write_bit_rejects_every_bit_index_above_7() {
        let mut c = S7Client::new();
        for bit in 8u8..=u8::MAX {
            assert!(
                matches!(c.write_bit(S7_AREA_DB, 1, 0, bit, true), Err(S7Error::InvalidFunParameter)),
                "bit index {bit}"
            );
        }
    }

    #[test]
    fn bit_access_with_valid_index_reports_not_connected() {
        let mut c = S7Client::new();
        for bit in 0u8..=7 {
            assert!(matches!(c.read_bit(S7_AREA_DB, 1, 8191, bit), Err(S7Error::NotConnected)));
            assert!(matches!(c.write_bit(S7_AREA_DB, 1, 8191, bit, false), Err(S7Error::NotConnected)));
        }
    }

    #[test]
    fn bit_access_rejects_byte_number_whose_bit_address_overflows_u16() {
        let mut c = S7Client::new();
        assert!(matches!(c.read_bit(S7_AREA_DB, 1, 8192, 0), Err(S7Error::InvalidFunParameter)));
        assert!(matches!(c.write_bit(S7_AREA_DB, 1, u16::MAX, 7, true), Err(S7Error::InvalidFunParameter)));
    }

    #[test]
    fn failed_read_and_write_reset_last_time_and_chunks() {
        let mut c = S7Client::new();
        c.last_time = 5.0;
        c.chunks = 3;
        let mut buf = [0u8; 4];
        assert!(c.read_area(S7_AREA_DB, 1, 0, S7_WL_BYTE, &mut buf).is_err());
        assert_eq!((c.last_time(), c.chunks()), (0.0, 0));

        c.last_time = 5.0;
        c.chunks = 3;
        assert!(c.write_area(S7_AREA_DB, 1, 0, S7_WL_BYTE, &buf).is_err());
        assert_eq!((c.last_time(), c.chunks()), (0.0, 0));
    }
}
