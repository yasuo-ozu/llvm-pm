//! Raw FFI bindings to LLVM new PassManager C++ stubs.
//!
//! This crate is not intended for direct use. Use the `llvm-pm` crate instead.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

// Re-export the correct llvm-sys version based on selected feature.
#[cfg(feature = "llvm10-0")]
pub use llvm_sys_100 as llvm_sys;
#[cfg(feature = "llvm11-0")]
pub use llvm_sys_110 as llvm_sys;
#[cfg(feature = "llvm12-0")]
pub use llvm_sys_120 as llvm_sys;
#[cfg(feature = "llvm13-0")]
pub use llvm_sys_130 as llvm_sys;
#[cfg(feature = "llvm14-0")]
pub use llvm_sys_140 as llvm_sys;
#[cfg(feature = "llvm15-0")]
pub use llvm_sys_150 as llvm_sys;
#[cfg(feature = "llvm16-0")]
pub use llvm_sys_160 as llvm_sys;
#[cfg(feature = "llvm17-0")]
pub use llvm_sys_170 as llvm_sys;
#[cfg(feature = "llvm18-1")]
pub use llvm_sys_181 as llvm_sys;
#[cfg(feature = "llvm19-1")]
pub use llvm_sys_191 as llvm_sys;
#[cfg(feature = "llvm20-1")]
pub use llvm_sys_201 as llvm_sys;
#[cfg(feature = "llvm21-1")]
pub use llvm_sys_211 as llvm_sys;
#[cfg(feature = "llvm22-1")]
pub use llvm_sys_221 as llvm_sys;

// Import LLVM types from llvm-sys. These are used by the bindgen-generated
// function signatures below (LLVM types are blocklisted from bindgen).
pub use self::llvm_sys::prelude::{
    LLVMBasicBlockRef, LLVMBool, LLVMContextRef, LLVMModuleRef, LLVMValueRef,
};
pub use self::llvm_sys::target_machine::LLVMTargetMachineRef;

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
