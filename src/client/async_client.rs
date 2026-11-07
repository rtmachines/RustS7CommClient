use super::protocol::async_io::{self, Timeouts};
use super::protocol::{self, Progress};
use super::shared::{self, ctx, Session};
use super::CT_PG;
use crate::error::{self, Result};
use anyerror::AddContext;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

/// Default `wr_timeout_ms`, also the session write timeout before any connect.
const DEFAULT_WR_TIMEOUT_MS: u64 = 500;

/// ### Asynchronous S7 client on tokio (feature `tokio`).
///
/// The async counterpart of [`S7Client`](crate::S7Client): the same settings, operations,
/// public status fields, error categories, contexts and session rules. Requests,
/// response checks, telegram splitting and address validation are shared with
/// the blocking client, so every method documented on `S7Client` behaves the same
/// way here; only the differences are described below.
///
/// ```no_run
/// # async fn example() -> rust7::Result<()> {
/// let mut client = rust7::AsyncS7Client::new();
/// client.connect_s71200_1500("192.168.0.100").await?;
/// let mut buffer = [0u8; 64];
/// client.read_db(100, 0, &mut buffer).await?;
/// client.disconnect();
/// # Ok(())
/// # }
/// ```
///
/// # Runtime
/// The futures must run on a tokio 1 runtime with I/O and time drivers enabled
/// (`Builder::enable_all()` or `#[tokio::main]`). A host name (not an IP literal)
/// is resolved on tokio's blocking pool, which needs tokio's `rt` feature; every
/// application running a tokio 1 runtime has it.
///
/// Methods take `&mut self`: one request is in flight at a time and the client
/// has no internal locks. The returned futures are `Send`.
///
/// # Cancel safety
/// The I/O methods (`connect_*`, `read_*`, `write_*`) are **not** cancel-safe in
/// the tokio sense: an abandoned call cannot be resumed. A future that was never
/// polled has no effect. If a future is dropped after it was polled and before it
/// completed (for example by `tokio::select!` or `tokio::time::timeout`), the
/// request may already be on the wire, so the client disconnects itself:
/// `connected` is `false`, `pdu_length` is 0, the socket is closed and the next
/// read or write returns `kind::NOT_CONNECTED`. `chunks` and `confirmed` keep the
/// values reached so far and have the same meaning as after an error: for a
/// write, bytes `[0, confirmed)` were acknowledged and the chunk starting at
/// `confirmed` may or may not have been written. Writes are never retried.
///
/// # Differences from `S7Client`
/// - `wr_timeout_ms` bounds sending (and flushing) a whole request instead of each
///   socket send; expiry is `kind::IO` with `request not sent within <wr_timeout_ms> ms`.
///   As in `S7Client`, the value in force at connect applies to the whole session,
///   while a changed `rd_timeout_ms` applies to the next response.
/// - `co_timeout_ms` bounds the TCP connect; expiry is `kind::IO` with
///   `TCP connection not established within <co_timeout_ms> ms`.
/// - `disconnect()` is synchronous: it drops (closes) the socket without an
///   asynchronous shutdown, so it also runs on drop and on cancellation.
pub struct AsyncS7Client {
    session: Option<Session<TcpStream>>,
    port: u16,
    co_timeout_ms: u64,
    rd_timeout_ms: u64,
    wr_timeout_ms: u64,
    /// Write timeout of the open session, fixed at connect like the socket send
    /// timeout of `S7Client`.
    session_wr_timeout: Duration,
    conn_type: u16,
    reference: u16,
    /// S7 PDU length negotiated by the CPU, excluding COTP (diagnostic value).
    /// Changing this field does not change the internal negotiated limits.
    pub pdu_length: u16,
    /// Client connection status. Changing this field does not establish a connection.
    pub connected: bool,
    /// ### Last Job time (ms).
    ///
    /// If an error occurred, or a started call was cancelled, the value will be 0;
    /// a future that was never polled leaves it unchanged
    pub last_time: f64,
    /// Number of read/write chunks attempted in the last operation.
    /// Same meaning as [`S7Client::chunks`](crate::S7Client::chunks).
    pub chunks: usize,
    /// Number of buffer bytes confirmed in the last read/write, also after a cancelled call.
    /// Same meaning as [`S7Client::confirmed`](crate::S7Client::confirmed).
    pub confirmed: usize,
}

/// Disconnects the client unless the I/O it guards ran to completion, so a
/// future dropped mid-exchange cannot leave the stream out of step. The
/// session is out of the client during the exchange and is dropped, closing
/// its socket, with such a future.
struct InFlight<'a> {
    client: &'a mut AsyncS7Client,
    progress: Progress,
    done: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.client.chunks = self.progress.chunks;
        self.client.confirmed = self.progress.confirmed;
        if !self.done {
            self.client.disconnect();
        }
    }
}

impl AsyncS7Client {
    /// ### Creates a new `AsyncS7Client` instance with default settings.
    ///
    /// The defaults are those of [`S7Client::new`](crate::S7Client::new). The client starts
    /// disconnected; use one of the `connect_XXX` methods to open a connection to a PLC.
    pub fn new() -> Self {
        AsyncS7Client {
            session: None,
            port: 102,
            co_timeout_ms: 3000,
            rd_timeout_ms: 1000,
            wr_timeout_ms: DEFAULT_WR_TIMEOUT_MS,
            session_wr_timeout: Duration::from_millis(DEFAULT_WR_TIMEOUT_MS),
            conn_type: CT_PG,
            reference: 0,
            pdu_length: 0,
            connected: false,
            last_time: 0.0,
            chunks: 0,
            confirmed: 0,
        }
    }

    /// ### Changes the S7 connection type to the PLC
    ///
    /// See [`S7Client::set_connection_type`](crate::S7Client::set_connection_type).
    pub fn set_connection_type(&mut self, connection_type: u16) -> Result<()> {
        shared::check_connection_type(connection_type)
            .add_context(|| ctx::set_connection_type(connection_type))?;
        self.conn_type = connection_type;
        Ok(())
    }

    /// ### Sets operations timeout
    ///
    /// See [`S7Client::set_timeout`](crate::S7Client::set_timeout); `wr_timeout_ms` bounds
    /// sending a whole request and `co_timeout_ms` the TCP connect. As in `S7Client`, a new
    /// `wr_timeout_ms` takes effect at the next connect, a new `rd_timeout_ms` at once.
    pub fn set_timeout(
        &mut self,
        co_timeout_ms: u64,
        rd_timeout_ms: u64,
        wr_timeout_ms: u64,
    ) -> Result<()> {
        shared::check_timeouts(co_timeout_ms, rd_timeout_ms, wr_timeout_ms)
            .add_context(|| ctx::set_timeout(co_timeout_ms, rd_timeout_ms, wr_timeout_ms))?;
        self.co_timeout_ms = co_timeout_ms;
        self.rd_timeout_ms = rd_timeout_ms;
        self.wr_timeout_ms = wr_timeout_ms;
        Ok(())
    }

    /// ### Sets the TCP Connection Port
    ///
    /// See [`S7Client::set_connection_port`](crate::S7Client::set_connection_port).
    pub fn set_connection_port(&mut self, port: u16) -> Result<()> {
        shared::check_port(port).add_context(|| ctx::set_connection_port(port))?;
        self.port = port;
        Ok(())
    }

    /// ### Connects to the S71200 or S71500 families
    ///
    /// See [`S7Client::connect_s71200_1500`](crate::S7Client::connect_s71200_1500) and
    /// the cancel safety notes on [`AsyncS7Client`].
    pub async fn connect_s71200_1500(&mut self, ip: &str) -> Result<()> {
        let port = self.port;
        self.connect_rack_slot_impl(ip, 0, 0)
            .await
            .add_context(|| ctx::connect_s71200_1500(ip, port))
    }

    /// ### Connects to the S7300 family
    ///
    /// See [`S7Client::connect_s7300`](crate::S7Client::connect_s7300) and
    /// the cancel safety notes on [`AsyncS7Client`].
    pub async fn connect_s7300(&mut self, ip: &str) -> Result<()> {
        let port = self.port;
        self.connect_rack_slot_impl(ip, 0, 2)
            .await
            .add_context(|| ctx::connect_s7300(ip, port))
    }

    /// ### Connects to a Siemens PLC/Drive using Rack and Slot
    ///
    /// See [`S7Client::connect_rack_slot`](crate::S7Client::connect_rack_slot) and
    /// the cancel safety notes on [`AsyncS7Client`].
    pub async fn connect_rack_slot(&mut self, ip: &str, rack: u16, slot: u16) -> Result<()> {
        let port = self.port;
        self.connect_rack_slot_impl(ip, rack, slot)
            .await
            .add_context(|| ctx::connect_rack_slot(ip, port, rack, slot))
    }

    async fn connect_rack_slot_impl(&mut self, ip: &str, rack: u16, slot: u16) -> Result<()> {
        let local_tsap: u16 = 0x0100;
        let remote_tsap = protocol::remote_tsap(self.conn_type, rack, slot)?;
        self.connect_tsap_impl(ip, local_tsap, remote_tsap).await
    }

    /// ### Connects to a Siemens ISO-Hardware using TSAP records
    ///
    /// See [`S7Client::connect_tsap`](crate::S7Client::connect_tsap) for the errors and
    /// negotiated limits. Any previous connection is closed first. The new connection
    /// becomes available only after both handshakes succeed, so a cancelled connect
    /// leaves the client disconnected.
    pub async fn connect_tsap(
        &mut self,
        ip: &str,
        local_tsap: u16,
        remote_tsap: u16,
    ) -> Result<()> {
        let port = self.port;
        self.connect_tsap_impl(ip, local_tsap, remote_tsap)
            .await
            .add_context(|| ctx::connect_tsap(ip, port, local_tsap, remote_tsap))
    }

    async fn connect_tsap_impl(
        &mut self,
        ip: &str,
        local_tsap: u16,
        remote_tsap: u16,
    ) -> Result<()> {
        self.disconnect();
        self.last_time = 0.0;
        let start_time = Instant::now();
        let co_timeout = Duration::from_millis(self.co_timeout_ms);
        let timeouts = self.connect_timeouts();

        let addr = tokio::net::lookup_host(format!("{}:{}", ip, self.port))
            .await
            .map_err(error::io)?
            .next()
            .ok_or_else(error::tcp_connection_failed)?;
        let mut stream = match tokio::time::timeout(co_timeout, TcpStream::connect(addr)).await {
            Ok(stream) => stream.map_err(error::io)?,
            Err(_) => {
                return Err(error::timed_out(format!(
                    "TCP connection not established within {} ms",
                    co_timeout.as_millis()
                )))
            }
        };
        stream.set_nodelay(true).map_err(error::io)?;

        // ISO-on-TCP handshake
        let iso_cr = protocol::connection_request(local_tsap, remote_tsap);
        async_io::send(&mut stream, &iso_cr, timeouts.write).await?;
        let iso_resp =
            async_io::read_tpkt(&mut stream, protocol::CONFIRM_FRAME_MAX, timeouts.read).await?;
        let tpdu_limit = protocol::validate_connection_confirm(&iso_resp)?;

        let limits = async_io::negotiate(&mut stream, tpdu_limit, timeouts).await?;
        self.pdu_length = limits.pdu_length;
        self.reference = 0;
        self.session_wr_timeout = timeouts.write;

        self.session = Some(Session { stream, limits });
        self.connected = true;
        self.last_time = start_time.elapsed().as_secs_f64() * 1000.0;

        Ok(())
    }

    /// ### Closes the connection.
    ///
    /// Synchronous: drops (closes) the socket. Safe to call even if the client is not
    /// connected. After disconnection, calls to read/write return `kind::NOT_CONNECTED`.
    pub fn disconnect(&mut self) {
        self.session = None;
        self.connected = false;
        self.pdu_length = 0;
    }

    /// Timeouts of a new connection: the configured values.
    fn connect_timeouts(&self) -> Timeouts {
        Timeouts {
            read: Duration::from_millis(self.rd_timeout_ms),
            write: Duration::from_millis(self.wr_timeout_ms),
        }
    }

    /// Timeouts of the open session. As in `S7Client`, the read timeout follows
    /// `set_timeout` at once, the write timeout is the one in force at connect.
    fn session_timeouts(&self) -> Timeouts {
        Timeouts {
            read: Duration::from_millis(self.rd_timeout_ms),
            write: self.session_wr_timeout,
        }
    }

    /// ### Reads a block of data from a specific S7 memory area.
    ///
    /// See [`S7Client::read_area`](crate::S7Client::read_area) and
    /// the cancel safety notes on [`AsyncS7Client`].
    pub async fn read_area(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        buffer: &mut [u8],
    ) -> Result<()> {
        let len = buffer.len();
        self.read_area_impl(
            protocol::Transfer::area(area, db_number, start, wordlen),
            buffer,
        )
        .await
        .add_context(|| ctx::read_area(area, db_number, start, wordlen, len))
    }

    async fn read_area_impl(
        &mut self,
        transfer: protocol::Transfer,
        buffer: &mut [u8],
    ) -> Result<()> {
        let mut session = self.begin(transfer, buffer.len())?;
        let time = Instant::now();
        let timeouts = self.session_timeouts();
        let mut guard = InFlight::new(self);
        let result = async_io::read_transfer(
            &mut session.stream,
            &session.limits,
            &mut guard.client.reference,
            transfer,
            buffer,
            &mut guard.progress,
            timeouts,
        )
        .await;
        guard.done = true;
        drop(guard);
        self.session = Some(session);
        self.finish(result, time)
    }

    /// ### Writes a block of data to a specific S7 memory area.
    ///
    /// See [`S7Client::write_area`](crate::S7Client::write_area) and
    /// the cancel safety notes on [`AsyncS7Client`]. Writes are not atomic and never retried.
    pub async fn write_area(
        &mut self,
        area: u8,
        db_number: u16,
        start: u16,
        wordlen: u8,
        buffer: &[u8],
    ) -> Result<()> {
        self.write_area_impl(
            protocol::Transfer::area(area, db_number, start, wordlen),
            buffer,
        )
        .await
        .add_context(|| ctx::write_area(area, db_number, start, wordlen, buffer.len()))
    }

    async fn write_area_impl(&mut self, transfer: protocol::Transfer, buffer: &[u8]) -> Result<()> {
        let mut session = self.begin(transfer, buffer.len())?;
        let time = Instant::now();
        let timeouts = self.session_timeouts();
        let mut guard = InFlight::new(self);
        let result = async_io::write_transfer(
            &mut session.stream,
            &session.limits,
            &mut guard.client.reference,
            transfer,
            buffer,
            &mut guard.progress,
            timeouts,
        )
        .await;
        guard.done = true;
        drop(guard);
        self.session = Some(session);
        self.finish(result, time)
    }

    /// Resets the statistics, runs the checks that precede any I/O and takes
    /// the session out of the client for the exchange.
    fn begin(&mut self, transfer: protocol::Transfer, len: usize) -> Result<Session<TcpStream>> {
        self.last_time = 0.0;
        self.chunks = 0;
        self.confirmed = 0;
        shared::check_area(transfer.area)?;
        protocol::validate_transfer(transfer.start, transfer.wordlen, len)?;
        match self.session.take() {
            Some(session) if self.connected => Ok(session),
            session => {
                self.session = session;
                Err(error::not_connected())
            }
        }
    }

    /// Records the job time on success and drops a connection whose stream
    /// may be out of step with the PLC.
    fn finish(&mut self, result: Result<()>, time: Instant) -> Result<()> {
        match result {
            Ok(()) => {
                self.last_time = time.elapsed().as_secs_f64() * 1000.0;
                Ok(())
            }
            Err(e) => {
                if error::breaks_session(&e) {
                    self.disconnect();
                }
                Err(e)
            }
        }
    }

    /// ### Reads a block of byte from a specific Data Block (DB)
    ///
    /// See [`S7Client::read_db`](crate::S7Client::read_db).
    pub async fn read_db(&mut self, db_number: u16, start: u16, buffer: &mut [u8]) -> Result<()> {
        let len = buffer.len();
        self.read_area_impl(protocol::Transfer::db(db_number, start), buffer)
            .await
            .add_context(|| ctx::read_db(db_number, start, len))
    }

    /// ### Reads a bit from a specific S7 memory area
    ///
    /// See [`S7Client::read_bit`](crate::S7Client::read_bit).
    pub async fn read_bit(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
    ) -> Result<bool> {
        self.read_bit_impl(area, db_number, byte_num, bit_idx)
            .await
            .add_context(|| ctx::read_bit(area, db_number, byte_num, bit_idx))
    }

    async fn read_bit_impl(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
    ) -> Result<bool> {
        self.last_time = 0.0;
        self.chunks = 0;
        self.confirmed = 0;
        let transfer = protocol::Transfer::bit(area, db_number, byte_num, bit_idx)?;
        let mut buffer = [0u8; 1];
        self.read_area_impl(transfer, &mut buffer).await?;
        Ok(protocol::bit_value(buffer[0]))
    }

    /// ### Writes a block of byte to a specific Data Block (DB)
    ///
    /// See [`S7Client::write_db`](crate::S7Client::write_db). Writes are not atomic and never retried.
    pub async fn write_db(&mut self, db_number: u16, start: u16, buffer: &[u8]) -> Result<()> {
        self.write_area_impl(protocol::Transfer::db(db_number, start), buffer)
            .await
            .add_context(|| ctx::write_db(db_number, start, buffer.len()))
    }

    /// ### Writes a bit to a specific S7 memory area
    ///
    /// See [`S7Client::write_bit`](crate::S7Client::write_bit). After an error or a cancelled
    /// call `confirmed` is 0 and the request may or may not have been applied by the PLC.
    pub async fn write_bit(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
        value: bool,
    ) -> Result<()> {
        self.write_bit_impl(area, db_number, byte_num, bit_idx, value)
            .await
            .add_context(|| ctx::write_bit(area, db_number, byte_num, bit_idx))
    }

    async fn write_bit_impl(
        &mut self,
        area: u8,
        db_number: u16,
        byte_num: u16,
        bit_idx: u8,
        value: bool,
    ) -> Result<()> {
        self.last_time = 0.0;
        self.chunks = 0;
        self.confirmed = 0;
        let transfer = protocol::Transfer::bit(area, db_number, byte_num, bit_idx)?;
        self.write_area_impl(transfer, &protocol::bit_payload(value))
            .await
    }
}

impl InFlight<'_> {
    fn new(client: &mut AsyncS7Client) -> InFlight<'_> {
        InFlight {
            client,
            progress: Progress::default(),
            done: false,
        }
    }
}

impl Default for AsyncS7Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AsyncS7Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests;
