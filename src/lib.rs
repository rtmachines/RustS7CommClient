#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod client;

pub use client::{
    S7Client, S7Error, CT_OP, CT_PG, CT_S7, S7_AREA_DB, S7_AREA_MK, S7_AREA_PA, S7_AREA_PE,
    S7_WL_BIT, S7_WL_BYTE,
};
