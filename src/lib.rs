#![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]
#![cfg_attr(feature = "allocator_api", feature(allocator_api))]

pub mod reader;
pub mod writer;

pub use minicbor;

#[cfg(feature = "alloc")]
extern crate alloc;
