// Rust7 - Native Rust S7 client (Snap7‑style) for Siemens PLCs.
// Copyright 2025 - Davide Nardella

use std::fmt;
use std::io;
use std::io::Write;
use std::net::Shutdown;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use std::time::Instant;

mod protocol;

#[cfg(test)]
mod tests;

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

// ISO connection request constants.
const ISO_CR_LEN: usize = 22;
const ISO_CONN_REQ: u8 = 0xe0;
const ISO_ID: u8 = 3;

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

#[derive(Debug)]
pub enum S7Error {
    Io(io::Error),
    NotConnected,
    TcpConnectionFailed,
    ConnectionClosed,
    IsoConnectionFailed,
    IsoFragmentedPacket,
    IsoInvalidHeader,
    IsoInvalidTelegram,
    PduNegotiationFailed,
    InvalidFunParameter,
    S7NotFound,
    S7InvalidAddress,
    S7Unspecified,
    Other(String),
}

impl fmt::Display for S7Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            S7Error::Io(e) => write!(f, "IO error: {}", e),
            S7Error::NotConnected => write!(f, "Not connected"),
            S7Error::TcpConnectionFailed => write!(f, "TCP connection failed"),
            S7Error::ConnectionClosed => write!(f, "TCP connection closed by the peer"),
            S7Error::IsoConnectionFailed => write!(f, "ISO-on-TCP connection failed"),
            S7Error::IsoFragmentedPacket => write!(f, "Fragmented ISO Packet"),
            S7Error::IsoInvalidHeader => write!(f, "Invalid ISO Header"),
            S7Error::IsoInvalidTelegram => write!(f, "Invalid ISO Telegram"),
            S7Error::PduNegotiationFailed => write!(f, "S7 PDU negotiation failed"),
            S7Error::InvalidFunParameter => write!(f, "Invalid parameter supplied to the function"),
            S7Error::S7NotFound => write!(f, "S7 Resource not found in the CPU"),
            S7Error::S7InvalidAddress => write!(f, "S7 Invalid address"),
            S7Error::S7Unspecified => write!(f, "S7 unspecified error"),
            S7Error::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl From<io::Error> for S7Error {
    fn from(err: io::Error) -> S7Error {
        S7Error::Io(err)
    }
}
pub struct S7Client {
    stream: Option<TcpStream>,
    port: u16,
    co_timeout_ms: u64,
    rd_timeout_ms: u64,
    wr_timeout_ms: u64,
    conn_type: u16,
    limits: Option<protocol::PduLimits>,
    reference: u16,
    /// PDU length negotiated by the CPU (diagnostic value).
    /// Changing this field does not change the internal negotiated limits.
    pub pdu_length: u16,
    /// Client connection status. Changing this field does not establish a connection.
    pub connected: bool,
    /// ### Last Job time (ms).
    ///
    /// If an error occurred the value will be 0
    pub last_time: f64,
    /// Number of read/write chunks attempted in the last operation.
    /// Reset before validation; includes the failing chunk if an I/O or PLC error occurs.
    pub chunks: usize,
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
            limits: None,
            reference: 0,
            pdu_length: 0x0000,
            connected: false,
            last_time: 0.0,
            chunks: 0,
        }
    }

    /// ### Check S7 Area validity
    ///
    fn check_area(&mut self, area: u8) -> Result<(), S7Error> {
        const AREAS: [u8; 4] = [S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB];
        if !AREAS.contains(&area) {
            return Err(S7Error::InvalidFunParameter);
        }
        Ok(())
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
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
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
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
    ///
    /// ### Notes
    /// 1. Values must be > 0, otherwise they are ignored
    /// 2. The client must not be connected (that is, call this method before connecting).
    ///
    pub fn set_timeout(
        &mut self,
        co_timeout_ms: u64,
        rd_timeout_ms: u64,
        wr_timeout_ms: u64,
    ) -> Result<(), S7Error> {
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
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
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
    /// - `rack` : CPU/CU Rack.
    /// - `slot` : CPU/CU Slot.
    ///
    /// ---
    /// For Notes, Return and Errors look at `connect_tsap()`
    ///
    pub fn connect_rack_slot(&mut self, ip: &str, rack: u16, slot: u16) -> Result<(), S7Error> {
        let local_tsap: u16 = 0x0100;
        let remote_tsap: u16 = (self.conn_type << 8) + (rack * 0x20) + slot;
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
    /// The connection port used is 102 (S7Protocol Port) unless you
    /// changed it via set_connection_port()
    ///
    /// ### Returns
    /// `Ok(())` on success, or an `S7Error` on failure.
    ///
    /// ### Errors
    /// - `S7Error::TcpConnectionFailed`: TCP connection could not be established.
    /// - `S7Error::IsoConnectionFailed`: ISO connection failed
    /// - `S7Error::PduNegotiationFailed`: PDU negotiation failed.
    /// - `S7Error::Io`: network I/O error.
    ///
    pub fn connect_tsap(
        &mut self,
        ip: &str,
        local_tsap: u16,
        remote_tsap: u16,
    ) -> Result<(), S7Error> {
        self.disconnect();
        self.last_time = 0.0;
        let start_time = Instant::now();

        let addr = format!("{}:{}", ip, self.port);
        let co_timeout = Duration::from_millis(self.co_timeout_ms);
        let rd_timeout = Duration::from_millis(self.rd_timeout_ms);
        let wr_timeout = Duration::from_millis(self.wr_timeout_ms);

        let mut stream = TcpStream::connect_timeout(
            &addr
                .to_socket_addrs()?
                .next()
                .ok_or(S7Error::TcpConnectionFailed)?,
            co_timeout,
        )?;

        stream.set_read_timeout(Some(rd_timeout))?;
        stream.set_write_timeout(Some(wr_timeout))?;
        stream.set_nodelay(true)?;

        // ISO-on-TCP handshake
        let iso_cr: [u8; ISO_CR_LEN] = [
            // TPKT (RFC1006 Header)
            ISO_ID,               // RFC 1006 ID (3)
            0x00,                 // Reserved, always 0
            hi_part!(ISO_CR_LEN), // High part of packet lenght (entire frame, payload and TPDU included)
            lo_part!(ISO_CR_LEN), // Low part of packet lenght (entire frame, payload and TPDU included)
            // COTP (ISO 8073 Header)
            0x11,                  // PDU Size Length
            ISO_CONN_REQ,          // CR - Connection Request ID
            0x00,                  // Dst Reference HI
            0x00,                  // Dst Reference LO
            0x00,                  // Src Reference HI
            0x01,                  // Src Reference LO
            0x00,                  // Class + Options Flags
            0xC0,                  // PDU Max Length ID
            0x01,                  // PDU Max Length HI
            0x0A,                  // PDU Max Length LO
            0xC1,                  // Src TSAP Identifier
            0x02,                  // Src TSAP Length (2 bytes)
            hi_part!(local_tsap),  // Loc TSAP HI
            lo_part!(local_tsap),  // Loc TSAP LO
            0xC2,                  // Rem TSAP Identifier
            0x02,                  // Rem TSAP Length (2 bytes)
            hi_part!(remote_tsap), // Rem TSAP HI
            lo_part!(remote_tsap), // Rem TSAP LO
        ];

        stream.write_all(&iso_cr)?;

        let iso_resp = protocol::read_tpkt(&mut stream, 1028)?;
        protocol::validate_connection_confirm(&iso_resp)?;

        let limits = protocol::negotiate(&mut stream)?;
        self.pdu_length = limits.pdu_length;
        self.limits = Some(limits);
        self.reference = 0;

        self.stream = Some(stream);
        self.connected = true;
        self.last_time = start_time.elapsed().as_secs_f64() * 1000.0;

        Ok(())
    }

    /// ### Closes the connection.
    ///
    /// Safe to call even if the client is not currently connected.
    /// After disconnection, calls to read/write will return `S7Error::NotConnected`.
    ///
    /// ### Notes
    /// A Client should be disconnected on low-level error (see read_area() and write_area() suggestion)
    ///
    pub fn disconnect(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.limits = None;
        self.connected = false;
        self.pdu_length = 0;
    }

    /// ### Reads a block of data from a specific S7 memory area.
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `start`: Starting element index (byte index for bytes, bit index for bits).
    /// - `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
    /// - `buffer`: Destination buffer to store the read data.
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
    /// 2. Whatever buffer is passed, only the first byte will be used, which is considered true if !=0 or false if ==0
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded.
    ///
    /// ### Errors
    /// #### Low level
    /// - `S7Error::NotConnected`: An attempt was made to read while the client was not connected.
    /// - `S7Error::IsoInvalidHeader`: Invalid ISO Header
    /// - `S7Error::IsoInvalidTelegram`: Inconsistent expected telegram length.
    /// - `S7Error::IsoFragmentedPacket`: ISO Packet fragmented.
    /// - `S7Error::S7Unspecified`: Unknown S7 Error.
    /// - `S7Error::Io`: network I/O error.
    ///
    /// #### Suggestion
    /// In case of a low-level error, it is **highly recommended** to disconnect and reconnect the Client (as WinCC or other SCADA do)
    ///
    /// #### High level
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
    /// - `S7Error::NotFound`: The resource was not found (e.g. Inexistent DB).
    /// - `S7Error::S7InvalidAddress`:
    /// 1. Attempt to read beyond the limits.
    /// 2. The DB is optimized.
    ///
    /// ### Notes
    /// - The number of bytes to read will be equal to the size of the buffer passed.
    /// - Large blocks, including buffers larger than 65535 bytes, are split into chunks.
    /// - A byte transfer must fit in the 24-bit bit address (last byte at most 0x1fffff).
    ///   An invalid range is rejected before sending any request.
    /// - Bit operations require a nonempty buffer and use only its first byte.
    /// - Empty byte buffers send no telegram after parameter and connection checks.
    /// - On error, earlier chunks may already be copied; discard the buffer.
    /// - Each response is checked against the request and expected payload length.
    ///
    pub fn read_area(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        buffer: &mut [u8],
    ) -> Result<(), S7Error> {
        self.read_area_impl(area, db_number, u32::from(start), wordlen, buffer)
    }

    fn read_area_impl(
        &mut self,
        area: u8,
        db_number: u16,
        start: u32,
        wordlen: u8,
        buffer: &mut [u8],
    ) -> Result<(), S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;
        self.check_area(area)?;
        protocol::validate_transfer(start, wordlen, buffer.len())?;
        if !self.connected {
            return Err(S7Error::NotConnected);
        }
        let limits = self.limits.as_ref().ok_or(S7Error::NotConnected)?;
        let stream = self.stream.as_mut().ok_or(S7Error::NotConnected)?;
        let time = Instant::now();
        protocol::read_transfer(
            stream,
            limits,
            &mut self.reference,
            protocol::Transfer {
                area,
                db_number,
                start,
                wordlen,
            },
            buffer,
            &mut self.chunks,
        )?;
        self.last_time = time.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    /// ### Writes a block of data to a specific S7 memory area.
    ///
    /// ### Parameters
    /// - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
    /// - `db_number`: DB number (ignored for non-DB areas).
    /// - `start`: Starting element index (byte index for bytes, bit index for bits).
    /// - `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
    /// - `buffer`: Source buffer to write.
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
    /// 2. Whatever buffer is passed, only the first byte will be used, which is considered true if !=0 or false if ==0
    /// 3. Writing a bit affects **only that bit**, leaving adjacent bits in the byte unchanged.
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded.
    ///
    /// ### Errors
    /// #### Low level
    /// - `S7Error::NotConnected`: An attempt was made to write while the client was not connected.
    /// - `S7Error::IsoInvalidHeader`: Invalid ISO Header
    /// - `S7Error::IsoInvalidTelegram`: Inconsistent expected telegram length.
    /// - `S7Error::IsoFragmentedPacket`: ISO Packet fragmented.
    /// - `S7Error::S7Unspecified`: Unknown S7 Error.
    /// - `S7Error::Io`: network I/O error.
    ///
    /// #### Suggestion
    /// In case of a low-level error, it is **highly recommended** to disconnect and reconnect the Client (as WinCC or other SCADA do)
    ///
    /// #### High level
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
    /// - `S7Error::NotFound`: The resource was not found (e.g. Inexistent DB).
    /// - `S7Error::S7InvalidAddress`:
    /// 1. Attempt to write beyond the limits.
    /// 2. The DB is optimized.
    ///
    /// ### Notes
    /// - A successful byte write transfers the entire buffer.
    /// - Writes are not atomic across chunks. On error, earlier chunks may already be written.
    ///   The client never automatically retries a write.
    /// - Large blocks, including buffers larger than 65535 bytes, are split into chunks.
    /// - A byte transfer must fit in the 24-bit bit address (last byte at most 0x1fffff).
    ///   An invalid range is rejected before sending any request.
    /// - Bit operations require a nonempty buffer and use only its first byte.
    /// - Empty byte buffers send no telegram after parameter and connection checks.
    /// - Writing the output buffer (`S7_AREA_PA`) usually does not produce useful results:
    ///   the output process image will be rewritten by OB1 in the next round.
    ///
    pub fn write_area(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        buffer: &[u8],
    ) -> Result<(), S7Error> {
        self.write_area_impl(area, db_number, u32::from(start), wordlen, buffer)
    }

    fn write_area_impl(
        &mut self,
        area: u8,
        db_number: u16,
        start: u32,
        wordlen: u8,
        buffer: &[u8],
    ) -> Result<(), S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;
        self.check_area(area)?;
        protocol::validate_transfer(start, wordlen, buffer.len())?;
        if !self.connected {
            return Err(S7Error::NotConnected);
        }
        let limits = self.limits.as_ref().ok_or(S7Error::NotConnected)?;
        let stream = self.stream.as_mut().ok_or(S7Error::NotConnected)?;
        let time = Instant::now();
        protocol::write_transfer(
            stream,
            limits,
            &mut self.reference,
            protocol::Transfer {
                area,
                db_number,
                start,
                wordlen,
            },
            buffer,
            &mut self.chunks,
        )?;
        self.last_time = time.elapsed().as_secs_f64() * 1000.0;
        Ok(())
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
    pub fn read_db(
        &mut self,
        db_number: u16,
        start: u16,
        buffer: &mut [u8],
    ) -> Result<(), S7Error> {
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
    /// - `byte_num`: Byte Number.
    /// - `bit_idx`: Bit index inside the byte (0..7).
    ///
    /// ### Example
    /// To read DB10.DBX71.4 use:
    ///
    /// ```my_bit = read_bit(S7_AREA_DB, 10, 71, 4);```
    ///
    /// ### Returns
    /// `Ok(<bool>)` or `Err(<S7Error>)`
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
    /// - Other reported by read_area()
    ///
    /// ### Suggestion
    ///   
    /// Even reading a single bit requires an entire telegram.
    /// Since reading is non-invasive, if you need to read multiple bits
    /// (more or less adjacent in the same area), I recommend reading blocks
    /// of bytes and then unpacking them.
    /// ---
    /// For further info, please refer to `read_area()`
    ///
    pub fn read_bit(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
    ) -> Result<bool, S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;
        let start = protocol::bit_start(byte_num, bit_idx)?;
        let mut buffer = [0u8; 1];
        self.read_area_impl(area, db_number, start, S7_WL_BIT, &mut buffer)?;

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
    /// - A successful byte write transfers the entire buffer.
    /// - Writes are not atomic across chunks. On error, earlier chunks may already be written.
    ///   The client never automatically retries a write.
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
    /// - `byte_num`: Byte Number.
    /// - `bit_idx`: Bit index inside the byte (0..7).
    /// - `value`: Value to write (true | false).
    ///
    /// ### Example
    /// To write **1** into DB10.DBX71.4 use:
    ///
    /// ```write_bit(S7_AREA_DB, 10, 71, 4, true);```
    ///
    /// ### Returns
    /// `Ok(())` Operation succeeded.
    ///
    /// ### Errors
    /// - `S7Error::InvalidFunParam`: Invalid parameter supplied to the function.
    /// - Other reported by read_area()
    ///
    /// ### Notes
    /// Writing a bit affects only that bit, leaving adjacent bits in the byte unchanged.
    /// ---
    /// For further info, please refer to `write_area()`
    ///
    pub fn write_bit(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
        value: bool,
    ) -> Result<(), S7Error> {
        self.last_time = 0.0;
        self.chunks = 0;
        let start = protocol::bit_start(byte_num, bit_idx)?;
        self.write_area_impl(area, db_number, start, S7_WL_BIT, &[u8::from(value)])
    }
}

impl Default for S7Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for S7Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}
