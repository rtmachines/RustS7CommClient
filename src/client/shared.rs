//! Parameter checks and error contexts shared by the sync and async clients.

use super::protocol;
use super::{CT_PG, CT_S7};
use crate::error::{self, Result};

pub(super) fn check_connection_type(connection_type: u16) -> Result<()> {
    if !(CT_PG..=CT_S7).contains(&connection_type) {
        return Err(error::invalid_fun_parameter());
    }
    Ok(())
}

pub(super) fn check_timeouts(
    co_timeout_ms: u64,
    rd_timeout_ms: u64,
    wr_timeout_ms: u64,
) -> Result<()> {
    if co_timeout_ms == 0 || rd_timeout_ms == 0 || wr_timeout_ms == 0 {
        return Err(error::invalid_fun_parameter());
    }
    Ok(())
}

pub(super) fn check_port(port: u16) -> Result<()> {
    if port == 0 {
        return Err(error::invalid_fun_parameter());
    }
    Ok(())
}

pub(super) fn check_area(area: u8) -> Result<()> {
    protocol::valid_area(area)
}

/// Context strings: method name and parameters, never buffer contents.
pub(super) mod ctx {
    pub(in crate::client) fn set_connection_type(connection_type: u16) -> String {
        format!("set_connection_type connection_type={connection_type}")
    }

    pub(in crate::client) fn set_timeout(co: u64, rd: u64, wr: u64) -> String {
        format!("set_timeout co_timeout_ms={co} rd_timeout_ms={rd} wr_timeout_ms={wr}")
    }

    pub(in crate::client) fn set_connection_port(port: u16) -> String {
        format!("set_connection_port port={port}")
    }

    pub(in crate::client) fn connect_s71200_1500(ip: &str, port: u16) -> String {
        format!("connect_s71200_1500 ip={ip} port={port}")
    }

    pub(in crate::client) fn connect_s7300(ip: &str, port: u16) -> String {
        format!("connect_s7300 ip={ip} port={port}")
    }

    pub(in crate::client) fn connect_rack_slot(
        ip: &str,
        port: u16,
        rack: u16,
        slot: u16,
    ) -> String {
        format!("connect_rack_slot ip={ip} port={port} rack={rack} slot={slot}")
    }

    pub(in crate::client) fn connect_tsap(ip: &str, port: u16, local: u16, remote: u16) -> String {
        format!(
            "connect_tsap ip={ip} port={port} local_tsap={local:#06x} remote_tsap={remote:#06x}"
        )
    }

    pub(in crate::client) fn read_area(
        area: u8,
        db: u16,
        start: u16,
        wordlen: u8,
        len: usize,
    ) -> String {
        format!("read_area area={area:#04x} db={db} start={start} wordlen={wordlen} len={len}")
    }

    pub(in crate::client) fn write_area(
        area: u8,
        db: u16,
        start: u16,
        wordlen: u8,
        len: usize,
    ) -> String {
        format!("write_area area={area:#04x} db={db} start={start} wordlen={wordlen} len={len}")
    }

    pub(in crate::client) fn read_db(db: u16, start: u16, len: usize) -> String {
        format!("read_db db={db} start={start} len={len}")
    }

    pub(in crate::client) fn write_db(db: u16, start: u16, len: usize) -> String {
        format!("write_db db={db} start={start} len={len}")
    }

    pub(in crate::client) fn read_bit(area: u8, db: u16, byte_num: u16, bit_idx: u8) -> String {
        format!("read_bit area={area:#04x} db={db} byte_num={byte_num} bit_idx={bit_idx}")
    }

    pub(in crate::client) fn write_bit(area: u8, db: u16, byte_num: u16, bit_idx: u8) -> String {
        format!("write_bit area={area:#04x} db={db} byte_num={byte_num} bit_idx={bit_idx}")
    }
}
