//! Tokio I/O for the protocol logic in `protocol`: the same frames, checks and
//! chunking as the blocking client, with timeouts from `tokio::time`.

use super::{
    accept_negotiation, negotiation_request, timed_out, tpkt_length, PduLimits, Progress, Transfer,
    TransferPlan, NEGOTIATION_FRAME_MAX,
};
use crate::error::{self, Result};
use std::io;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Read limit for one complete response frame and write limit for one request.
#[derive(Clone, Copy, Debug)]
pub(in crate::client) struct Timeouts {
    pub(in crate::client) read: Duration,
    pub(in crate::client) write: Duration,
}

/// Writes and flushes a whole request within `timeout`.
pub(in crate::client) async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &[u8],
    timeout: Duration,
) -> Result<()> {
    let sent = async {
        writer.write_all(request).await?;
        writer.flush().await
    };
    match tokio::time::timeout(timeout, sent).await {
        Ok(result) => result.map_err(error::io),
        Err(_) => Err(error::timed_out(format!(
            "request not sent within {} ms",
            timeout.as_millis()
        ))),
    }
}

/// Reads one TPKT frame; `timeout` bounds the whole frame.
pub(in crate::client) async fn read_tpkt<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_total: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let frame = async {
        let mut header = [0; 4];
        read_full(reader, &mut header).await?;
        let len = tpkt_length(&header, max_total)?;
        let mut frame = vec![0; len];
        frame[..4].copy_from_slice(&header);
        read_full(reader, &mut frame[4..]).await?;
        Ok(frame)
    };
    // A timeout beyond the clock range waits without a deadline instead of panicking.
    tokio::time::timeout(timeout, frame)
        .await
        .unwrap_or_else(|_| Err(timed_out(timeout)))
}

async fn read_full<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]).await {
            Ok(0) => return Err(error::connection_closed()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(error::io(e)),
        }
    }
    Ok(())
}

pub(in crate::client) async fn negotiate<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    tpdu_limit: u16,
    timeouts: Timeouts,
) -> Result<PduLimits> {
    let (request, requested_pdu) = negotiation_request(tpdu_limit);
    send(io, &request, timeouts.write).await?;
    let frame = read_tpkt(io, NEGOTIATION_FRAME_MAX, timeouts.read).await?;
    accept_negotiation(&frame, requested_pdu)
}

pub(in crate::client) async fn read_transfer<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &mut [u8],
    progress: &mut Progress,
    timeouts: Timeouts,
) -> Result<()> {
    let mut plan = TransferPlan::read(limits, transfer, buffer.len())?;
    while let Some(chunk) = plan.next_read(reference) {
        progress.chunks += 1;
        send(io, &chunk.request, timeouts.write).await?;
        let frame = read_tpkt(io, plan.frame_limit(), timeouts.read).await?;
        plan.accept_read(&chunk, &frame, buffer, progress)?;
    }
    Ok(())
}

pub(in crate::client) async fn write_transfer<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &[u8],
    progress: &mut Progress,
    timeouts: Timeouts,
) -> Result<()> {
    let mut plan = TransferPlan::write(limits, transfer, buffer)?;
    while let Some(chunk) = plan.next_write(reference, buffer) {
        progress.chunks += 1;
        send(io, &chunk.request, timeouts.write).await?;
        let frame = read_tpkt(io, plan.frame_limit(), timeouts.read).await?;
        plan.accept_write(&chunk, &frame, progress)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
