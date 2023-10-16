# Async CBOR Reader and Writer

[![CI](https://github.com/rmja/minicbor-embedded-io/actions/workflows/ci.yaml/badge.svg)](https://github.com/rmja/minicbor-embedded-io/actions/workflows/ci.yaml)
[![crates.io](https://img.shields.io/crates/v/minicbor-embedded-io.svg)](https://crates.io/crates/minicbor-embedded-io)
[![docs.rs](https://docs.rs/minicbor-embedded-io/badge.svg)](https://docs.rs/minicbor-embedded-io)

The `minicbor-embedded-io` crate implements async read and write for the `minicbor` crate on top of the `embedded-io-async` `Read` and `Write` traits.

The library is inspired by the way the [Dahomey.Cbor](https://github.com/dahomey-technologies/Dahomey.Cbor) library does asynchronous read and write.
For example, to read an array, one must implement the `CborArrayReader` trait, which is called for each array item.
The callback can either actually read the item, or return an error indicating that it needs more bytes to fully decode - in this case the reader will be called again whenever more bytes become available.
