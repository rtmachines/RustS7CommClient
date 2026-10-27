use super::S7Error;
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug)]
pub(super) struct PduLimits {
    pub(super) pdu_length: u16,
    pub(super) read_payload: usize,
    pub(super) write_payload: usize,
}

impl PduLimits {
    pub(super) fn new(pdu_length: u16) -> Result<Self, S7Error> {
        if !(29..=480).contains(&pdu_length) {
            return Err(S7Error::PduNegotiationFailed);
        }
        Ok(Self {
            pdu_length,
            read_payload: usize::from(pdu_length) - 18,
            write_payload: usize::from(pdu_length) - 28,
        })
    }
}

pub(super) fn negotiate<T: Read + Write>(io: &mut T) -> Result<PduLimits, S7Error> {
    io.write_all(&[
        3, 0, 0, 25, 2, 0xf0, 0x80, 0x32, 1, 0, 0, 4, 0, 0, 8, 0, 0, 0xf0, 0, 0, 1, 0, 1, 1, 0xe0,
    ])?;
    let frame = read_tpkt(io, 487)?;
    let pdu = data_payload(&frame).map_err(|_| S7Error::PduNegotiationFailed)?;
    // One outstanding job in each direction was requested; all these fields
    // belong to the Setup Communication ACK_DATA, not to the negotiated size.
    if pdu.len() != 20 || pdu[..18] != [0x32, 3, 0, 0, 4, 0, 0, 8, 0, 0, 0, 0, 0xf0, 0, 0, 1, 0, 1]
    {
        return Err(S7Error::PduNegotiationFailed);
    }
    PduLimits::new(u16::from_be_bytes([pdu[18], pdu[19]]))
}

pub(super) fn read_tpkt<R: Read>(reader: &mut R, max_total: usize) -> Result<Vec<u8>, S7Error> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    if header[0] != 3 || header[1] != 0 {
        return Err(S7Error::IsoInvalidHeader);
    }
    let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if len < 4 || len > max_total {
        return Err(S7Error::IsoInvalidTelegram);
    }
    let mut frame = vec![0; len];
    frame[..4].copy_from_slice(&header);
    reader.read_exact(&mut frame[4..])?;
    Ok(frame)
}

fn valid_tpkt(frame: &[u8]) -> bool {
    frame.len() >= 4
        && frame[0] == 3
        && frame[1] == 0
        && usize::from(u16::from_be_bytes([frame[2], frame[3]])) == frame.len()
}

pub(super) fn data_payload(frame: &[u8]) -> Result<&[u8], S7Error> {
    if !valid_tpkt(frame) || frame.len() < 8 || frame[4] != 2 || frame[5] != 0xf0 {
        return Err(S7Error::IsoInvalidHeader);
    }
    if frame[6] != 0x80 {
        return Err(S7Error::IsoFragmentedPacket);
    }
    Ok(&frame[7..])
}

pub(super) fn validate_connection_confirm(frame: &[u8]) -> Result<(), S7Error> {
    if !valid_tpkt(frame)
        || frame.len() < 11
        || usize::from(frame[4]) + 5 != frame.len()
        || frame[5] != 0xd0
        || frame[6..8] != [0, 1]
        || frame[10] != 0
    {
        return Err(S7Error::IsoConnectionFailed);
    }
    let mut offset = 11;
    while offset < frame.len() {
        if offset + 2 > frame.len() {
            return Err(S7Error::IsoConnectionFailed);
        }
        let len = usize::from(frame[offset + 1]);
        if offset + 2 + len > frame.len() {
            return Err(S7Error::IsoConnectionFailed);
        }
        offset += 2 + len;
    }
    Ok(())
}

fn ack_data(pdu: &[u8], reference: u16, function: u8) -> Result<&[u8], S7Error> {
    if pdu.len() < 12
        || pdu[..4] != [0x32, 3, 0, 0]
        || u16::from_be_bytes([pdu[4], pdu[5]]) != reference
    {
        return Err(S7Error::IsoInvalidTelegram);
    }
    let params = usize::from(u16::from_be_bytes([pdu[6], pdu[7]]));
    let data = usize::from(u16::from_be_bytes([pdu[8], pdu[9]]));
    if 12 + params + data != pdu.len() {
        return Err(S7Error::IsoInvalidTelegram);
    }
    if pdu[10..12] != [0, 0] {
        return Err(S7Error::S7Unspecified);
    }
    if params != 2 || pdu.len() < 14 || pdu[12..14] != [function, 1] {
        return Err(S7Error::IsoInvalidTelegram);
    }
    Ok(&pdu[14..])
}

fn item_status(status: u8) -> Result<(), S7Error> {
    match status {
        0xff => Ok(()),
        5 => Err(S7Error::S7InvalidAddress),
        10 => Err(S7Error::S7NotFound),
        _ => Err(S7Error::S7Unspecified),
    }
}

pub(super) fn validate_read_response(
    pdu: &[u8],
    reference: u16,
    wordlen: u8,
    expected_payload: usize,
) -> Result<&[u8], S7Error> {
    let data = ack_data(pdu, reference, 4)?;
    if data.len() < 4 {
        return Err(S7Error::IsoInvalidTelegram);
    }
    item_status(data[0])?;
    let (transport, bits) = match wordlen {
        1 if expected_payload == 1 => (3, 1),
        2 => (
            4,
            expected_payload
                .checked_mul(8)
                .ok_or(S7Error::IsoInvalidTelegram)?,
        ),
        _ => return Err(S7Error::IsoInvalidTelegram),
    };
    if data[1] != transport
        || usize::from(u16::from_be_bytes([data[2], data[3]])) != bits
        || data.len() - 4 != expected_payload
    {
        return Err(S7Error::IsoInvalidTelegram);
    }
    Ok(&data[4..])
}

pub(super) fn validate_write_response(pdu: &[u8], reference: u16) -> Result<(), S7Error> {
    let data = ack_data(pdu, reference, 5)?;
    if data.len() != 1 {
        return Err(S7Error::IsoInvalidTelegram);
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

pub(super) fn bit_start(byte_num: u16, bit_idx: u8) -> Result<u32, S7Error> {
    if bit_idx > 7 {
        return Err(S7Error::InvalidFunParameter);
    }
    Ok(u32::from(byte_num) * 8 + u32::from(bit_idx))
}

pub(super) fn validate_transfer(start: u32, wordlen: u8, len: usize) -> Result<usize, S7Error> {
    match wordlen {
        1 if len > 0 && start <= 0xffffff => Ok(1),
        2 if start <= 0x1fffff => {
            let last = (start as usize)
                .checked_add(len.saturating_sub(1))
                .ok_or(S7Error::InvalidFunParameter)?;
            if last > 0x1fffff {
                return Err(S7Error::InvalidFunParameter);
            }
            Ok(len)
        }
        _ => Err(S7Error::InvalidFunParameter),
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

fn valid_area(area: u8) -> Result<(), S7Error> {
    if ![0x81, 0x82, 0x83, 0x84].contains(&area) {
        return Err(S7Error::InvalidFunParameter);
    }
    Ok(())
}

pub(super) fn read_transfer<T: Read + Write>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &mut [u8],
    chunks: &mut usize,
) -> Result<(), S7Error> {
    valid_area(transfer.area)?;
    let len = validate_transfer(transfer.start, transfer.wordlen, buffer.len())?;
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(limits.read_payload);
        let reference = next_reference(reference);
        let req = request(&transfer, offset, count, reference, None);
        *chunks += 1;
        io.write_all(&req)?;
        let frame = read_tpkt(io, usize::from(limits.pdu_length) + 7)?;
        let data =
            validate_read_response(data_payload(&frame)?, reference, transfer.wordlen, count)?;
        buffer[offset..offset + count].copy_from_slice(data);
        offset += count;
    }
    Ok(())
}

pub(super) fn write_transfer<T: Read + Write>(
    io: &mut T,
    limits: &PduLimits,
    reference: &mut u16,
    transfer: Transfer,
    buffer: &[u8],
    chunks: &mut usize,
) -> Result<(), S7Error> {
    valid_area(transfer.area)?;
    let len = validate_transfer(transfer.start, transfer.wordlen, buffer.len())?;
    let mut offset = 0;
    while offset < len {
        let count = (len - offset).min(limits.write_payload);
        let reference = next_reference(reference);
        let req = request(
            &transfer,
            offset,
            count,
            reference,
            Some(&buffer[offset..offset + count]),
        );
        *chunks += 1;
        io.write_all(&req)?;
        let frame = read_tpkt(io, usize::from(limits.pdu_length) + 7)?;
        validate_write_response(data_payload(&frame)?, reference)?;
        offset += count;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
