#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod client;
pub mod error;

pub use client::{
    S7Client,
    CT_PG, CT_OP, CT_S7,
    S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB,
    S7_WL_BIT, S7_WL_BYTE,
};
pub use error::S7Error;
