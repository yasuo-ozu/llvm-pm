//! Safe Rust wrapper for LLVM's new PassManager.
//!
//! Provides [`ModulePassManager`] for running optimization passes on LLVM modules
//! using the new PassBuilder-based infrastructure (LLVM 18+).
//!
//! # Example
//!
//! ```ignore
//! use llvm_pm::{ModulePassManager, OptLevel};
//!
//! // Assume `context` and `module` are valid LLVM-C handles.
//! unsafe {
//!     let pm = ModulePassManager::with_opt_level(context, None, OptLevel::O2)
//!         .expect("Failed to create pass manager");
//!     pm.run(module).expect("Pass execution failed");
//! }
//! ```

use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr;

pub use llvm_pm_sys::{LLVMContextRef, LLVMModuleRef, LLVMTargetMachineRef};

/// Optimization level for the default pass pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
    Os,
    Oz,
}

impl OptLevel {
    fn to_c(self) -> llvm_pm_sys::LlvmPmOptLevel {
        match self {
            OptLevel::O0 => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O0,
            OptLevel::O1 => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O1,
            OptLevel::O2 => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O2,
            OptLevel::O3 => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O3,
            OptLevel::Os => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_Os,
            OptLevel::Oz => llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_Oz,
        }
    }
}

/// Error type for pass manager operations.
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    /// Returns the error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

/// Consume a C error string into a Rust [`Error`], freeing the C string.
unsafe fn consume_c_error(ptr: *mut std::ffi::c_char) -> Error {
    let msg = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    llvm_pm_sys::llvm_pm_dispose_message(ptr);
    Error { message: msg }
}

/// A configured LLVM module pass manager using the new PassManager infrastructure.
///
/// Bundles all analysis managers, PassBuilder, StandardInstrumentations, and the
/// ModulePassManager into a single object with correct lifetime management.
///
/// # Safety
///
/// The `LLVMContextRef` and optional `LLVMTargetMachineRef` passed to
/// construction must remain valid for the lifetime of this object.
/// The `LLVMModuleRef` passed to [`run()`](ModulePassManager::run) must be valid
/// and belong to the same context.
pub struct ModulePassManager {
    raw: llvm_pm_sys::LlvmPmPassManagerRef,
}

impl fmt::Debug for ModulePassManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModulePassManager")
            .field("raw", &self.raw)
            .finish()
    }
}

impl ModulePassManager {
    /// Create a pass manager with a standard optimization pipeline.
    ///
    /// # Arguments
    /// * `context` - The LLVM context (must outlive this pass manager).
    /// * `target_machine` - Optional target machine for target-specific passes.
    ///   Pass `None` for target-independent optimizations.
    /// * `level` - The optimization level.
    ///
    /// # Safety
    /// `context` and optional `target_machine` must be valid LLVM handles that
    /// outlive this `ModulePassManager`.
    pub unsafe fn with_opt_level(
        context: LLVMContextRef,
        target_machine: Option<LLVMTargetMachineRef>,
        level: OptLevel,
    ) -> Result<Self, Error> {
        let tm = target_machine.unwrap_or(ptr::null_mut());
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        let raw =
            llvm_pm_sys::llvm_pm_create_with_opt_level(context, tm, level.to_c(), &mut err_msg);

        if raw.is_null() {
            Err(consume_c_error(err_msg))
        } else {
            Ok(Self { raw })
        }
    }

    /// Create a pass manager from a textual pipeline description.
    ///
    /// The format matches `opt -passes=...` syntax, e.g.:
    /// - `"instcombine,dce,sroa"`
    /// - `"default<O2>"`
    /// - `"module(function(instcombine,sroa))"`
    ///
    /// # Safety
    /// `context` and optional `target_machine` must be valid LLVM handles that
    /// outlive this `ModulePassManager`.
    pub unsafe fn with_pipeline(
        context: LLVMContextRef,
        target_machine: Option<LLVMTargetMachineRef>,
        pipeline: &str,
    ) -> Result<Self, Error> {
        let tm = target_machine.unwrap_or(ptr::null_mut());
        let c_pipeline = CString::new(pipeline).map_err(|e| Error {
            message: format!("Pipeline string contains null byte: {}", e),
        })?;
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        let raw = llvm_pm_sys::llvm_pm_create_with_pipeline(
            context,
            tm,
            c_pipeline.as_ptr(),
            &mut err_msg,
        );

        if raw.is_null() {
            Err(consume_c_error(err_msg))
        } else {
            Ok(Self { raw })
        }
    }

    /// Run the optimization passes on the given module.
    ///
    /// # Safety
    /// `module` must be a valid `LLVMModuleRef` belonging to the same
    /// `LLVMContextRef` used to create this pass manager.
    pub unsafe fn run(&self, module: LLVMModuleRef) -> Result<(), Error> {
        let err = llvm_pm_sys::llvm_pm_run(self.raw, module);
        if err.is_null() {
            Ok(())
        } else {
            Err(consume_c_error(err))
        }
    }
}

impl Drop for ModulePassManager {
    fn drop(&mut self) {
        unsafe {
            llvm_pm_sys::llvm_pm_dispose(self.raw);
        }
    }
}
