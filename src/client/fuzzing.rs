//! Entry points for the cargo-fuzz targets in `fuzz/`; compiled only with `--cfg fuzzing`.
//! Not part of the public API.

use super::protocol::{self, FrameRead, PduLimits, Progress, Transfer};
use crate::error::Result;
use std::io::{self, Read, Write};
use std::time::Duration;

pub fn tpkt_length(header: &[u8; 4], max_total: usize) -> Result<usize> {
    protocol::tpkt_length(header, max_total)
}

pub fn data_payload(frame: &[u8]) -> Result<&[u8]> {
    protocol::data_payload(frame)
}

/// The TPDU size limit of an accepted Connection Confirm.
pub fn validate_connection_confirm(frame: &[u8]) -> Result<u16> {
    protocol::validate_connection_confirm(frame)
}

/// The negotiated PDU length of an accepted Setup Communication reply.
pub fn accept_negotiation(frame: &[u8], requested_pdu: u16) -> Result<u16> {
    protocol::accept_negotiation(frame, requested_pdu).map(|limits| limits.pdu_length)
}

pub fn validate_read_response(
    pdu: &[u8],
    reference: u16,
    wordlen: u8,
    expected_payload: usize,
) -> Result<&[u8]> {
    protocol::validate_read_response(pdu, reference, wordlen, expected_payload)
}

pub fn validate_write_response(pdu: &[u8], reference: u16) -> Result<()> {
    protocol::validate_write_response(pdu, reference)
}

/// Replies come from a fixed byte string; requests are discarded.
struct Replies<'a>(&'a [u8]);

impl Read for Replies<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Replies<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl FrameRead for Replies<'_> {
    fn limit_next_read(&mut self, _timeout: Duration) -> io::Result<()> {
        Ok(())
    }
}

/// What a transfer did with a scripted reply stream.
pub struct TransferOutcome {
    pub result: Result<()>,
    pub chunks: usize,
    pub confirmed: usize,
}

/// Runs a read (or write) of `buffer` from DB 1 at `start` against `replies`.
/// `wordlen` is `S7_WL_BIT` or `S7_WL_BYTE`; `pdu_length` must be in 29..=480.
pub fn transfer(
    pdu_length: u16,
    start: u16,
    wordlen: u8,
    writing: bool,
    buffer: &mut [u8],
    replies: &[u8],
) -> Result<TransferOutcome> {
    let limits = PduLimits::new(pdu_length)?;
    let transfer = Transfer::area(super::S7_AREA_DB, 1, start, wordlen);
    let mut io = Replies(replies);
    let mut reference = 0;
    let mut progress = Progress::default();
    let timeout = Duration::from_secs(1);
    let result = if writing {
        protocol::write_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer,
            buffer,
            &mut progress,
            timeout,
        )
    } else {
        protocol::read_transfer(
            &mut io,
            &limits,
            &mut reference,
            transfer,
            buffer,
            &mut progress,
            timeout,
        )
    };
    Ok(TransferOutcome {
        result,
        chunks: progress.chunks,
        confirmed: progress.confirmed,
    })
}
