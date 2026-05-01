//! Plugin API for creating LLVM pass plugins.
//!
//! This module provides types for building LLVM plugins that can be loaded by
//! `opt`, `clang`, or other LLVM tools via `--load-pass-plugin`.
//!
//! # Usage
//!
//! Create a `cdylib` crate and use the [`plugin`](crate::plugin!) attribute macro:
//!
//! ```ignore
//! #[llvm_pm::plugin(name = "my_plugin", version = "0.1")]
//! fn plugin_registrar(builder: &mut llvm_pm::plugin::PassBuilder) {
//!     builder.add_module_pipeline_parsing_callback(|name, mpm| {
//!         if name == "my-pass" {
//!             mpm.add_pass(MyPass);
//!             llvm_pm::plugin::PipelineParsing::Parsed
//!         } else {
//!             llvm_pm::plugin::PipelineParsing::NotParsed
//!         }
//!     });
//! }
//! ```

use std::ffi::c_void;

use crate::traits::{LlvmCgsccPass, LlvmFunctionPass, LlvmLoopPass, LlvmModulePass};
use crate::{FunctionAnalysisManager, ModuleAnalysisManager, OptLevel};

// Re-export the proc macro when available
#[cfg(feature = "plugin-macros")]
pub use llvm_pm_macros::plugin;

/// Return the LLVM plugin API version.
pub fn plugin_api_version() -> u32 {
    // SAFETY: No preconditions — returns a compile-time constant.
    unsafe { llvm_pm_sys::llvm_pm_plugin_api_version() }
}

/// Information returned by `llvmGetPassPluginInfo`.
#[repr(C)]
pub struct PassPluginLibraryInfo {
    pub api_version: u32,
    pub plugin_name: *const u8,
    pub plugin_version: *const u8,
    pub plugin_registrar: extern "C" fn(*mut c_void),
}

/// Enum describing whether a pipeline parsing callback successfully parsed
/// its given pipeline element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineParsing {
    /// The pipeline element was successfully parsed.
    Parsed,
    /// The pipeline element was not parsed.
    NotParsed,
}

// =========================================================================
// PassBuilder
// =========================================================================

/// Handle to LLVM's `PassBuilder`, received by plugin registrar functions.
///
/// Provides methods to register callbacks for pipeline parsing, analysis
/// registration, and various extension points.
pub struct PassBuilder {
    inner: *mut c_void,
}

impl PassBuilder {
    /// Construct from a raw PassBuilder pointer.
    ///
    /// # Safety
    /// `pass_builder` must be a valid pointer to LLVM's `PassBuilder`.
    pub unsafe fn from_raw(pass_builder: *mut c_void) -> Self {
        Self {
            inner: pass_builder,
        }
    }

    /// Register a callback for parsing module pipeline elements.
    ///
    /// When LLVM encounters an unknown pass name in a module pipeline, it
    /// calls registered callbacks. If the callback recognizes the name, it
    /// should add the pass to the given [`PluginModulePassManager`] and return
    /// [`PipelineParsing::Parsed`].
    pub fn add_module_pipeline_parsing_callback<T>(&mut self, cb: T)
    where
        T: Fn(&str, &mut PluginModulePassManager) -> PipelineParsing + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            // SAFETY: cb was created by Box::into_raw below.
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(
            cb: *const c_void,
            name_ptr: *const std::ffi::c_char,
            name_len: usize,
            manager: *mut c_void,
        ) -> std::ffi::c_int
        where
            T: Fn(&str, &mut PluginModulePassManager) -> PipelineParsing + 'static,
        {
            // SAFETY: cb is a valid pointer kept alive by shared_ptr in C++.
            let cb = unsafe { &*(cb as *const T) };
            // SAFETY: LLVM provides valid UTF-8 pass names via StringRef.
            let name = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    name_ptr as *const u8,
                    name_len,
                ))
            };
            let mut manager = PluginModulePassManager { inner: manager };
            let result = cb(name, &mut manager);
            matches!(result, PipelineParsing::Parsed) as std::ffi::c_int
        }

        // SAFETY: inner is a valid PassBuilder*. The boxed closure is handed to
        // C++ via shared_ptr and will be freed by callback_deleter.
        unsafe {
            llvm_pm_sys::llvm_pm_pb_add_module_pipeline_parsing_callback(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }

    /// Register a callback for parsing function pipeline elements.
    pub fn add_function_pipeline_parsing_callback<T>(&mut self, cb: T)
    where
        T: Fn(&str, &mut PluginFunctionPassManager) -> PipelineParsing + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(
            cb: *const c_void,
            name_ptr: *const std::ffi::c_char,
            name_len: usize,
            manager: *mut c_void,
        ) -> std::ffi::c_int
        where
            T: Fn(&str, &mut PluginFunctionPassManager) -> PipelineParsing + 'static,
        {
            let cb = unsafe { &*(cb as *const T) };
            let name = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    name_ptr as *const u8,
                    name_len,
                ))
            };
            let mut manager = PluginFunctionPassManager { inner: manager };
            let result = cb(name, &mut manager);
            matches!(result, PipelineParsing::Parsed) as std::ffi::c_int
        }

        unsafe {
            llvm_pm_sys::llvm_pm_pb_add_function_pipeline_parsing_callback(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }

    /// Register a callback for module analysis registration.
    pub fn add_module_analysis_registration_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut ModuleAnalysisManager) + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(cb: *const c_void, manager: *mut c_void)
        where
            T: Fn(&mut ModuleAnalysisManager) + 'static,
        {
            let cb = unsafe { &*(cb as *const T) };
            // SAFETY: manager is a valid ModuleAnalysisManager* from LLVM.
            let mut manager = unsafe { ModuleAnalysisManager::from_raw(manager, None) };
            cb(&mut manager);
        }

        unsafe {
            llvm_pm_sys::llvm_pm_pb_add_module_analysis_registration_callback(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }

    /// Register a callback for function analysis registration.
    pub fn add_function_analysis_registration_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut FunctionAnalysisManager) + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(cb: *const c_void, manager: *mut c_void)
        where
            T: Fn(&mut FunctionAnalysisManager) + 'static,
        {
            let cb = unsafe { &*(cb as *const T) };
            let mut manager = unsafe { FunctionAnalysisManager::from_raw(manager, None) };
            cb(&mut manager);
        }

        unsafe {
            llvm_pm_sys::llvm_pm_pb_add_function_analysis_registration_callback(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }

    /// Register a peephole extension point callback (function-level).
    pub fn add_peephole_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginFunctionPassManager, OptLevel) + 'static,
    {
        self.add_fpm_ep_callback(cb, llvm_pm_sys::llvm_pm_pb_add_peephole_ep_callback);
    }

    /// Register a scalar-optimizer-late extension point callback (function-level).
    pub fn add_scalar_optimizer_late_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginFunctionPassManager, OptLevel) + 'static,
    {
        self.add_fpm_ep_callback(
            cb,
            llvm_pm_sys::llvm_pm_pb_add_scalar_optimizer_late_ep_callback,
        );
    }

    /// Register a vectorizer-start extension point callback (function-level).
    pub fn add_vectorizer_start_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginFunctionPassManager, OptLevel) + 'static,
    {
        self.add_fpm_ep_callback(cb, llvm_pm_sys::llvm_pm_pb_add_vectorizer_start_ep_callback);
    }

    /// Register an optimizer-last extension point callback (module-level).
    ///
    /// Available on LLVM 11+. On older versions, this is a no-op.
    pub fn add_optimizer_last_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
    {
        self.add_mpm_ep_callback(cb, llvm_pm_sys::llvm_pm_pb_add_optimizer_last_ep_callback);
    }

    /// Register a pipeline-start extension point callback (module-level).
    ///
    /// Available on LLVM 12+. On older versions, this is a no-op.
    pub fn add_pipeline_start_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
    {
        self.add_mpm_ep_callback(cb, llvm_pm_sys::llvm_pm_pb_add_pipeline_start_ep_callback);
    }

    /// Register a pipeline-early-simplification extension point callback (module-level).
    ///
    /// Available on LLVM 12+. On older versions, this is a no-op.
    pub fn add_pipeline_early_simplification_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
    {
        self.add_mpm_ep_callback(
            cb,
            llvm_pm_sys::llvm_pm_pb_add_pipeline_early_simplification_ep_callback,
        );
    }

    /// Register an optimizer-early extension point callback (module-level).
    ///
    /// Available on LLVM 15+. On older versions, this is a no-op.
    pub fn add_optimizer_early_ep_callback<T>(&mut self, cb: T)
    where
        T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
    {
        self.add_mpm_ep_callback(cb, llvm_pm_sys::llvm_pm_pb_add_optimizer_early_ep_callback);
    }

    // --- Internal helpers for extension point registration ---

    #[allow(clippy::type_complexity)]
    fn add_mpm_ep_callback<T>(
        &mut self,
        cb: T,
        register_fn: unsafe extern "C" fn(
            *mut c_void,
            *const c_void,
            Option<unsafe extern "C" fn(*const c_void)>,
            Option<unsafe extern "C" fn(*const c_void, *mut c_void, llvm_pm_sys::LlvmPmOptLevel)>,
        ),
    ) where
        T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(
            cb: *const c_void,
            manager: *mut c_void,
            opt: llvm_pm_sys::LlvmPmOptLevel,
        ) where
            T: Fn(&mut PluginModulePassManager, OptLevel) + 'static,
        {
            let cb = unsafe { &*(cb as *const T) };
            let mut manager = PluginModulePassManager { inner: manager };
            cb(&mut manager, opt_level_from_c(opt));
        }

        unsafe {
            register_fn(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }

    #[allow(clippy::type_complexity)]
    fn add_fpm_ep_callback<T>(
        &mut self,
        cb: T,
        register_fn: unsafe extern "C" fn(
            *mut c_void,
            *const c_void,
            Option<unsafe extern "C" fn(*const c_void)>,
            Option<unsafe extern "C" fn(*const c_void, *mut c_void, llvm_pm_sys::LlvmPmOptLevel)>,
        ),
    ) where
        T: Fn(&mut PluginFunctionPassManager, OptLevel) + 'static,
    {
        let cb = Box::new(cb);

        unsafe extern "C" fn callback_deleter<T>(cb: *const c_void) {
            drop(unsafe { Box::<T>::from_raw(cb as *mut T) });
        }

        unsafe extern "C" fn callback_entrypoint<T>(
            cb: *const c_void,
            manager: *mut c_void,
            opt: llvm_pm_sys::LlvmPmOptLevel,
        ) where
            T: Fn(&mut PluginFunctionPassManager, OptLevel) + 'static,
        {
            let cb = unsafe { &*(cb as *const T) };
            let mut manager = PluginFunctionPassManager { inner: manager };
            cb(&mut manager, opt_level_from_c(opt));
        }

        unsafe {
            register_fn(
                self.inner,
                Box::into_raw(cb) as *const c_void,
                Some(callback_deleter::<T>),
                Some(callback_entrypoint::<T>),
            );
        }
    }
}

// =========================================================================
// PluginModulePassManager
// =========================================================================

/// A borrowed module pass manager received in plugin pipeline parsing callbacks.
///
/// Unlike [`ModulePassManager`](crate::ModulePassManager), this does not own
/// the pass manager — it wraps a raw `ModulePassManager*` from LLVM. Passes
/// added to it are owned by the C++ side.
pub struct PluginModulePassManager {
    inner: *mut c_void,
}

impl PluginModulePassManager {
    /// Add a custom module pass.
    pub fn add_pass<P: LlvmModulePass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        // SAFETY: inner is a valid ModulePassManager*. The trampoline is
        // monomorphized for P. C++ takes ownership via shared_ptr + pass_deleter.
        unsafe {
            llvm_pm_sys::llvm_pm_raw_mpm_add_module_pass(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::module_pass_trampoline::<P>),
            );
        }
    }

    /// Add a custom CGSCC pass (adapted into the module pipeline).
    pub fn add_cgscc_pass<P: LlvmCgsccPass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        unsafe {
            llvm_pm_sys::llvm_pm_raw_mpm_add_cgscc_pass(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::cgscc_pass_trampoline::<P>),
            );
        }
    }

    /// Add a custom function pass adapted through the CGSCC level.
    pub fn add_function_pass_via_cgscc<P: LlvmFunctionPass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        unsafe {
            llvm_pm_sys::llvm_pm_raw_mpm_add_function_pass_via_cgscc(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::function_pass_trampoline::<P>),
            );
        }
    }

    /// Add a custom loop pass adapted through the CGSCC and function levels.
    pub fn add_loop_pass_via_cgscc<P: LlvmLoopPass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        unsafe {
            llvm_pm_sys::llvm_pm_raw_mpm_add_loop_pass_via_cgscc(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::loop_pass_trampoline::<P>),
            );
        }
    }
}

// =========================================================================
// PluginFunctionPassManager
// =========================================================================

/// A borrowed function pass manager received in plugin pipeline parsing callbacks.
///
/// Unlike [`FunctionPassManager`](crate::FunctionPassManager), this does not
/// own the pass manager. Passes added to it are owned by the C++ side.
pub struct PluginFunctionPassManager {
    inner: *mut c_void,
}

impl PluginFunctionPassManager {
    /// Add a custom function pass.
    pub fn add_pass<P: LlvmFunctionPass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        unsafe {
            llvm_pm_sys::llvm_pm_raw_fpm_add_function_pass(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::function_pass_trampoline::<P>),
            );
        }
    }

    /// Add a custom loop pass (adapted into the function pipeline).
    pub fn add_loop_pass<P: LlvmLoopPass + 'static>(&mut self, pass: P) {
        let boxed = Box::into_raw(Box::new(pass));

        unsafe {
            llvm_pm_sys::llvm_pm_raw_fpm_add_loop_pass(
                self.inner,
                boxed as *mut c_void,
                Some(pass_deleter::<P>),
                Some(crate::loop_pass_trampoline::<P>),
            );
        }
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Generic pass data deleter. Frees a `Box<T>` that was `Box::into_raw`'d.
unsafe extern "C" fn pass_deleter<T>(ptr: *mut c_void) {
    // SAFETY: ptr was created by Box::into_raw in the add_pass methods.
    drop(unsafe { Box::<T>::from_raw(ptr as *mut T) });
}

/// Convert the C enum to our Rust OptLevel.
fn opt_level_from_c(level: llvm_pm_sys::LlvmPmOptLevel) -> OptLevel {
    match level {
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O0 => OptLevel::O0,
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O1 => OptLevel::O1,
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O2 => OptLevel::O2,
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_O3 => OptLevel::O3,
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_Os => OptLevel::Os,
        llvm_pm_sys::LlvmPmOptLevel_LlvmPmOptLevel_Oz => OptLevel::Oz,
        _ => OptLevel::O2,
    }
}
