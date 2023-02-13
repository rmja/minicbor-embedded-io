#![cfg_attr(not(test), no_std)]
#![feature(async_fn_in_trait)]
#![allow(incomplete_features)]
#![cfg_attr(feature = "alloc", feature(allocator_api))]

pub mod reader;

#[cfg(feature = "alloc")]
extern crate alloc;
