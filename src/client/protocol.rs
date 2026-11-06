use crate::error::{self, AnyError, Result};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// A transport whose next blocking read can be bounded.
pub(super) trait FrameRead: Read {
    fn limit_next_read(&mut self, timeout: Duration) -> io::Result<()>;
}

impl FrameRead for TcpStream {
    fn limit_next_read(&mut self, timeout: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(timeout))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PduLimits {
    pub(super) pdu_length: u16,
    pub(super) read_payload: usize,
    pub(super) write_payload: usize,
}

impl PduLimits {
    pub(super) fn new(pdu_length: u16) -> Result<Self> {
        if !(29..=480).contains(&pdu_length) {
            return Err(error::pdu_negotiation_failed());
        }
        Ok(Self {
            pdu_length,
            read_payload: usize::from(pdu_length) - 18,
            write_payload: usize::from(pdu_length) - 28,
        })
    }
}

pub(super) fn negotiate<T: FrameRead + Write>(
    io: &mut T,
    tpdu_limit: u16,
    timeout: Duration,
) -> Result<PduLimits> {
    // CC validation guarantees at least 128 bytes, including the COTP header.
    let requested_pdu = 480.min(tpdu_limit - 3);
    let mut request = [
        3, 0, 0, 25, 2, 0xf0, 0x80, 0x32, 1, 0, 0, 4, 0, 0, 8, 0, 0, 0xf0, 0, 0, 1, 0, 1, 1, 0xe0,
    ];
    request[23..25].copy_from_slice(&requested_pdu.to_be_bytes());
    io.write_all(&request).map_err(error::io)?;
    let frame = read_tpkt(io, 487, timeout)?;
    let pdu = data_payload(&frame).map_err(error::pdu_negotiation_failed_by)?;
    // One outstanding job in each direction was requested; all these fields
    // belong to the Setup Communication ACK_DATA, not to the negotiated size.
    if pdu.len() != 20 || pdu[..18] != [0x32, 3, 0, 0, 4, 0, 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1, 0, 1]
    {
        return Err(error::pdu_negotiation_failed());
    }
    let pdu_length = u16::from_be_bytes([pdu[18], pdu[19]]);
    if pdu_length > requested_pdu {
        return Err(error::pdu_negotiation_failed());
    }
    PduLimits::new(pdu_length)
}

pub(super) fn read_tpkt<R: FrameRead>(
    reader: &mut R,
    max_total: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    // The timeout bounds the whole frame. A deadline beyond the clock range
    // falls back to bounding each read.
    let deadline = Instant::now().checked_add(timeout);
    let mut header = [0; 4];
    read_full(reader, &mut header, deadline, timeout)?;
    if header[0] != 3 || header[1] != 0 {
        return Err(error::iso_invalid_header());
    }
    let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if len < 4 || len > max_total {
        return Err(error::iso_invalid_telegram());
    }
    let mut frame = vec![0; len];
    frame[..4].copy_from_slice(&header);
    read_full(reader, &mut frame[4..], deadline, timeout)?;
    Ok(frame)
}

fn read_full<R: FrameRead>(
    reader: &mut R,
    buf: &mut [u8],
    deadline: Option<Instant>,
    timeout: Duration,
) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let limit = match deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => timeout,
        };
        if limit.is_zero() {
            return Err(timed_out(timeout));
        }
        reader.limit_next_read(limit).map_err(error::io)?;
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Err(error::connection_closed()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(timed_out(timeout));
            }
            Err(e) => return Err(error::io(e)),
        }
    }
    Ok(())
}

fn timed_out(timeout: Duration) -> AnyError {
    error::io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "no complete response frame within {} ms",
            timeout.as_millis()
        ),
    ))
}

fn valid_tpkt(frame: &[u8]) -> bool {
    frame.len() >= 4
        && frame[0] == 3
        && frame[1] == 0
        && usize::from(u16::from_be_bytes([frame[2], frame[3]])) == frame.len()
}

pub(super) fn data_payload(frame: &[u8]) -> Result<&[u8]> {
    if !valid_tpkt(frame) || frame.len() < 8 || frame[4] != 2 || frame[5] != 0xf0 {
        return Err(error::iso_invalid_header());
    }
    if frame[6] != 0x80 {
        return Err(error::iso_fragmented_packet());
    }
    Ok(&frame[7..])
}

pub(super) fn validate_connection_confirm(frame: &[u8]) -> Result<u16> {
    if !valid_tpkt(frame)
        || frame.len() < 11
        || usize::from(frame[4]) + 5 != frame.len()
        || frame[5] != 0xd0
        || frame[6..8] != [0, 1]
        || frame[10] != 0
    {
        return Err(error::iso_connection_failed());
    }
    let mut offset = 11;
    let mut tpdu_limit = None;
    while offset < frame.len() {
        if offset + 2 > frame.len() {
            return Err(error::iso_connection_failed());
        }
        let len = usize::from(frame[offset + 1]);
        if offset + 2 + len > frame.len() {
            return Err(error::iso_connection_failed());
        }
        if frame[offset] == 0xc0 {
            if tpdu_limit.is_some() || len != 1 || !(7..=10).contains(&frame[offset + 2]) {
                return Err(error::iso_connection_failed());
            }
            tpdu_limit = Some(1u16 << frame[offset + 2]);
        }
        offset += 2 + len;
    }
    // Without C0, retain our local 1024-byte send cap for compatibility.
    Ok(tpdu_limit.unwrap_or(1024))
}

fn ack_data(pdu: &[u8], reference: u16, function: u8) -> Result<&[u8]> {
    if pdu.len() < 12
        || pdu[0] != 0x32
        || !matches!(pdu[1], 2 | 3)
        || pdu[2..4] != [0, 0]
        || u16::from_be_bytes([pdu[4], pdu[5]]) != reference
    {
        return Err(error::iso_invalid_telegram());
    }
    let params = usize::from(u16::from_be_bytes([pdu[6], pdu[7]]));
    let data = usize::from(u16::from_be_bytes([pdu[8], pdu[9]]));
    if 12 + params + data != pdu.len() {
        return Err(error::iso_invalid_telegram());
    }
    if pdu[1] == 2 {
        // Only a header-only error ACK completes the job. An empty success ACK
        // does not confirm that a read or write was performed.
        return if params == 0 && data == 0 && pdu[10..12] != [0, 0] {
            Err(error::s7_header_error(pdu[10], pdu[11]))
        } else {
            Err(error::iso_invalid_telegram())
        };
    }
    if pdu[10..12] != [0, 0] {
        return Err(error::s7_header_error(pdu[10], pdu[11]));
    }
    if params != 2 || pdu.len() < 14 || pdu[12..14] != [function, 1] {
        return Err(error::iso_invalid_telegram());
    }
    Ok(&pdu[14..])
}

fn item_status(status: u8) -> Result<()> {
    match status {
        0xff => Ok(()),
        5 => Err(error::s7_invalid_address()),
        10 => Err(error::s7_not_found()),
        code => Err(error::s7_item_error(code)),
    }
}

pub(super) fn validate_read_response(
    pdu: &[u8],
    reference: u16,
    wordlen: u8,
    expected_payload: usize,
) -> Result<&[u8]> {
    let data = ack_data(pdu, reference, 4)?;
    if data.len() < 4 {
        return Err(error::iso_invalid_telegram());
    }
    item_status(data[0])?;
    let (transport, bits) = match wordlen {
        1 if expected_payload == 1 => (3, 1),
        2 => (
            4,
            expected_payload
                .checked_mul(8)
                .ok_or_else(error::iso_invalid_telegram)?,
        ),
        _ => return Err(error::iso_invalid_telegram()),
    };
    if data[1] != transport
        || usize::from(u16::from_be_bytes([data[2], data[3]])) != bits
        || data.len() - 4 != expected_payload
    {
        return Err(error::iso_invalid_telegram());
    }
    Ok(&data[4..])
}

pub(super) fn validate_write_response(pdu: &[u8], reference: u16) -> Result<()> {
    let data = ack_data(pdu, reference, 5)?;
    if data.len() != 1 {
        return Err(error::iso_invalid_telegram());
    }
    item_status(data[0])
}

pub(super) fn next_reference(counter: &mut u16) -> u16 {
    *counter = counter.wrapping_add(1);
    *counter
}

pub(super) struct Transfer {
    pub(super) area: u8,
    pub(super) db_number: u16,
    pub(super) start: u32,
    pub(super) wordlen: u8,
}

pub(super) fn bit_start(byte_num: u16, bit_idx: u8) -> Result<u32> {
    if bit_idx > 7 {
        return Err(error::invalid_fun_parameter());
    }
    Ok(u32::from(byte_num) * 8 + u32::from(bit_idx))
}

pub(super) fn remote_tsap(conn_type: u16, rack: u16, slot: u16) -> Result<u16> {
    if rack > 7 || slot > 31 {
        return Err(error::invalid_fun_parameter());
    }
    Ok((conn_type << 8) | (rack << 5) | slot)
}

pub(super) fn validate_transfer(start: u32, wordlen: u8, len: usize) -> Result<usize> {
    match wordlen {
        1 if len > 0 && start <= 0xffffff => Ok(1),
        2 if start <= 0x1fffff => {
            let last = (start as usize)
                .checked_add(len.saturating_sub(1))
                .ok_or_else(error::invalid_fun_parameter)?;
            if last > 0x1fffff {
                return Err(error::invalid_fun_parameter());
            }
            Ok(len)
        }
        _ => Err(error::invalid_fun_parameter()),
    }
}

fn request(
    transfer: &Transfer,
    offset: usize,
    count: usize,
    reference: u16,
    payload: Option<&[u8]>,
) -> Vec<u8> {
    // The transfer range is checked before the first request. A chunk is at
    // most 462 bytes, so these protocol fields cannot truncate.
    let mut req = vec![
        3,
        0,
        0,
        31,
        2,
        0xf0,
        0x80,
        0x32,
        1,
        0,
        0,
        0,
        0,
        0,
        14,
        0,
        0,
        if payload.is_some() { 5 } else { 4 },
        1,
        0x12,
        0x0a,
        0x10,
        transfer.wordlen,
        0,
        0,
        0,
        0,
        transfer.area,
        0,
        0,
        0,
    ];
    req[11..13].copy_from_slice(&reference.to_be_bytes());
    req[23..25].copy_from_slice(&(count as u16).to_be_bytes());
    let db = if transfer.area == 0x84 {
        transfer.db_number
    } else {
        0
    };
    req[25..27].copy_from_slice(&db.to_be_bytes());
    let start = transfer.start + offset as u32;
    let address = if transfer.wordlen == 1 {
        start
    } else {
        start * 8
    };
    req[28..31].copy_from_slice(&address.to_be_bytes()[1..]);
    if let Some(payload) = payload {
        req[15..17].copy_from_slice(&((count + 4) as u16).to_be_bytes());
        let bits = if transfer.wordlen == 1 { 1 } else { count * 8 };
        req.extend_from_slice(&[0, if transfer.wordlen == 1 { 3 } else { 4 }]);
        req.extend_from_slice(&(bits as u16).to_be_bytes());
        req.extend_from_slice(payload);
        let total = req.len() as u16;
        req[2..4].copy_from_slice(&total.to_be_bytes());
    }
    req
}

fn valid_area(area: u8) -> Result<()> {
    if ![0x81, 0x82, 0x83, 0x84].contains(&area) {
        return Err(error::invalid_fun_parameter());
    }
    Ok(())
}

/// Transfer counters shared with the caller so they survive an error.
#[derive(Default)]
pub(super) struct Progress {
    pub(super) chunks: usize,
    pub(super) confirmed: usize,
}

pub(super) fn read_transfer<T: FrameRead + Write>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &mut [u8],
    progress: &mut Progress,
    timeout: Duration,
) -> Result<()> {
    valid_area(transfer.area)?;
    let len = validate_transfer(transfer.start, transfer.wordlen, buffer.len())?;
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(limits.read_payload);
        let reference = next_reference(reference);
        let req = request(&transfer, offset, count, reference, None);
        progress.chunks += 1;
        io.write_all(&req).map_err(error::io)?;
        let frame = read_tpkt(io, usize::from(limits.pdu_length) + 7, timeout)?;
        let data =
            validate_read_response(data_payload(&frame)?, reference, transfer.wordlen, count)?;
        buffer[offset..offset + count].copy_from_slice(data);
        offset += count;
        progress.confirmed = offset;
    }
    Ok(())
}

pub(super) fn write_transfer<T: FrameRead + Write>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &[u8],
    progress: &mut Progress,
    timeout: Duration,
) -> Result<()> {
    valid_area(transfer.area)?;
    let len = validate_transfer(transfer.start, transfer.wordlen, buffer.len())?;
    // validate_transfer guarantees a nonempty buffer for bit access.
    let bit = [u8::from(transfer.wordlen == 1 && buffer[0] != 0)];
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(limits.write_payload);
        let reference = next_reference(reference);
        let payload = if transfer.wordlen == 1 {
            &bit[..]
        } else {
            &buffer[offset..offset + count]
        };
        let req = request(&transfer, offset, count, reference, Some(payload));
        progress.chunks += 1;
        io.write_all(&req).map_err(error::io)?;
        let frame = read_tpkt(io, usize::from(limits.pdu_length) + 7, timeout)?;
        validate_write_response(data_payload(&frame)?, reference)?;
        offset += count;
        progress.confirmed = offset;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
