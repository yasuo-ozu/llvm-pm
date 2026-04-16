//! Raw FFI bindings to LLVM new PassManager C++ stubs.
//!
//! This crate is not intended for direct use. Use the `llvm-pm` crate instead.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
