//! Safe Rust wrapper for LLVM's new PassManager.
//!
//! Provides [`ModulePassManager`] and [`FunctionPassManager`] for running optimization
//! passes on LLVM modules and functions using the new PassBuilder-based infrastructure
//! (LLVM 10+).
//!
//! # Example
//!
//! ```ignore
//! use inkwell::context::Context;
//! use llvm_pm::{ModulePassManager, OptLevel};
//!
//! let context = Context::create();
//! let module = context.create_module("my_module");
//!
//! let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O2, None)
//!     .expect("Failed to create pass manager");
//! pm.run(&module).expect("Pass execution failed");
//! ```

#[cfg(any(
    feature = "llvm10-0",
    feature = "llvm11-0",
    feature = "llvm12-0",
    feature = "llvm13-0",
    feature = "llvm14-0",
    feature = "llvm15-0",
    feature = "llvm16-0",
    feature = "llvm17-0",
    feature = "llvm18-0"
))]
pub extern crate inkwell_05 as inkwell;
#[cfg(any(
    feature = "llvm19-1",
    feature = "llvm20-1",
    feature = "llvm21-1",
    feature = "llvm22-1"
))]
pub extern crate inkwell_09 as inkwell;

pub mod traits;

#[cfg(feature = "llvm-plugin-crate")]
mod llvm_plugin_harness;

use inkwell::values::AsValueRef;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::fmt;
use std::marker::PhantomData;
#[cfg(feature = "llvm-plugin-crate")]
use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::{Mutex, Once};

pub use llvm_pm_sys::{LLVMBasicBlockRef, LLVMModuleRef, LLVMValueRef};
use traits::{
    LlvmCgsccAnalysis, LlvmCgsccPass, LlvmFunctionPass, LlvmLoopAnalysis, LlvmLoopPass,
    LlvmModulePass, PreservedAnalyses,
};
pub type AnalysisKey = *const u8;

#[cfg(all(
    feature = "llvm-plugin-crate",
    any(
        feature = "llvm19-1",
        feature = "llvm20-1",
        feature = "llvm21-1",
        feature = "llvm22-1"
    )
))]
compile_error!(
    "`llvm-plugin-crate` supports llvm-plugin-compatible LLVM features (llvm10-0 .. llvm18-0)."
);

pub struct ModuleAnalysisManager {
    inner: *mut c_void,
    from_analysis_id: Option<AnalysisKey>,
}

impl ModuleAnalysisManager {
    /// # Safety
    /// `inner` must be a valid pointer to LLVM's `ModuleAnalysisManager`.
    pub unsafe fn from_raw(inner: *mut c_void, from_analysis_id: Option<AnalysisKey>) -> Self {
        Self {
            inner,
            from_analysis_id,
        }
    }
    pub fn as_raw(&self) -> *mut c_void {
        self.inner
    }
    pub fn from_analysis_id(&self) -> Option<AnalysisKey> {
        self.from_analysis_id
    }
}

pub struct FunctionAnalysisManager {
    inner: *mut c_void,
    from_analysis_id: Option<AnalysisKey>,
}

impl FunctionAnalysisManager {
    /// # Safety
    /// `inner` must be a valid pointer to LLVM's `FunctionAnalysisManager`.
    pub unsafe fn from_raw(inner: *mut c_void, from_analysis_id: Option<AnalysisKey>) -> Self {
        Self {
            inner,
            from_analysis_id,
        }
    }
    pub fn as_raw(&self) -> *mut c_void {
        self.inner
    }
    pub fn from_analysis_id(&self) -> Option<AnalysisKey> {
        self.from_analysis_id
    }
}

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
///
/// # Safety
/// `ptr` must be a non-null pointer to a C string allocated by the C++ stubs
/// (via `malloc`/`copyString`). Ownership is transferred to this function.
unsafe fn consume_c_error(ptr: *mut std::ffi::c_char) -> Error {
    // SAFETY: Caller guarantees ptr is a valid, non-null, malloc'd C string.
    let msg = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    llvm_pm_sys::llvm_pm_dispose_message(ptr);
    Error { message: msg }
}

// =========================================================================
// Options
// =========================================================================

/// Builder for pass manager options.
///
/// Controls debug logging, IR verification, and extension point pipeline
/// additions. Pass to [`ModulePassManager`] or [`FunctionPassManager`]
/// constructors via `Option<&Options>`.
pub struct Options {
    raw: llvm_pm_sys::LlvmPmOptionsRef,
}

impl Options {
    /// Create default options (debug logging off, verify each off).
    pub fn new() -> Self {
        // SAFETY: llvm_pm_options_create always returns a valid handle.
        let raw = unsafe { llvm_pm_sys::llvm_pm_options_create() };
        Self { raw }
    }

    /// Enable or disable debug logging output during pass execution.
    pub fn debug_logging(&mut self, val: bool) -> &mut Self {
        // SAFETY: self.raw is a valid options handle created by llvm_pm_options_create.
        unsafe {
            llvm_pm_sys::llvm_pm_options_set_debug_logging(self.raw, val as i32);
        }
        self
    }

    /// Enable or disable IR verification after each pass.
    pub fn verify_each(&mut self, val: bool) -> &mut Self {
        // SAFETY: self.raw is a valid options handle created by llvm_pm_options_create.
        unsafe {
            llvm_pm_sys::llvm_pm_options_set_verify_each(self.raw, val as i32);
        }
        self
    }

    /// Add a pass pipeline at the peephole extension point (after instcombine, function-level).
    pub fn add_peephole_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_peephole_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the optimizer-early extension point (module-level).
    pub fn add_optimizer_early_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_optimizer_early_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the optimizer-last extension point (module-level).
    pub fn add_optimizer_last_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_optimizer_last_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the vectorizer-start extension point (function-level).
    pub fn add_vectorizer_start_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_vectorizer_start_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the scalar-optimizer-late extension point (function-level).
    pub fn add_scalar_optimizer_late_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_scalar_optimizer_late_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the pipeline-start extension point (module-level).
    pub fn add_pipeline_start_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_pipeline_start_ep(self.raw, c.as_ptr());
        }
        self
    }

    /// Add a pass pipeline at the pipeline-early-simplification extension point (module-level).
    pub fn add_pipeline_early_simplification_ep(&mut self, pipeline: &str) -> &mut Self {
        let c = CString::new(pipeline).expect("pipeline contains null byte");
        // SAFETY: self.raw is valid; c.as_ptr() is a valid null-terminated C string.
        unsafe {
            llvm_pm_sys::llvm_pm_options_add_pipeline_early_simplification_ep(self.raw, c.as_ptr());
        }
        self
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        // SAFETY: self.raw is a valid options handle, and Drop runs at most once.
        unsafe {
            llvm_pm_sys::llvm_pm_options_dispose(self.raw);
        }
    }
}

/// Analysis manager handle for CGSCC passes and analyses.
pub struct CgsccAnalysisManager {
    raw: *mut c_void,
    from_analysis_id: Option<AnalysisKey>,
}

impl CgsccAnalysisManager {
    /// Construct from an opaque manager pointer passed by the C++ layer.
    ///
    /// # Safety
    /// `raw` must be a valid pointer to LLVM's `CGSCCAnalysisManager`.
    pub unsafe fn from_raw(raw: *mut c_void) -> Self {
        Self {
            raw,
            from_analysis_id: None,
        }
    }

    unsafe fn from_raw_with_analysis_id(
        raw: *mut c_void,
        from_analysis_id: Option<AnalysisKey>,
    ) -> Self {
        Self {
            raw,
            from_analysis_id,
        }
    }

    /// Get the underlying opaque pointer.
    pub fn as_raw(&self) -> *mut c_void {
        self.raw
    }

    /// Register a CGSCC analysis pass.
    pub fn add_analysis<P>(&self, pass: P)
    where
        P: LlvmCgsccAnalysis,
        P: 'static,
        <P as LlvmCgsccAnalysis>::Result: 'static,
    {
        extern "C" fn pass_deleter<T: LlvmCgsccAnalysis>(pass: *mut c_void) {
            // SAFETY: Pointer was created by Box::into_raw in add_pass.
            drop(unsafe { Box::<T>::from_raw(pass.cast()) });
        }

        fn pass_entrypoint<T>(
            pass: *mut c_void,
            function: &inkwell::values::FunctionValue<'_>,
            manager: *mut c_void,
        ) -> *mut c_void
        where
            T: LlvmCgsccAnalysis,
            T::Result: 'static,
        {
            // SAFETY: pass is a valid pointer to T stored in registry.
            let pass = unsafe { &*(pass as *const T) };
            // SAFETY: manager is valid for callback duration.
            let manager =
                unsafe { CgsccAnalysisManager::from_raw_with_analysis_id(manager, Some(T::id())) };
            let result = pass.run_analysis(function, &manager);
            Box::into_raw(Box::new(result)) as *mut c_void
        }

        let manager_key = self.raw as usize;
        let analysis_key = <P as LlvmCgsccAnalysis>::id() as usize;
        let mut passes = cgscc_analysis_passes()
            .lock()
            .expect("poisoned cgscc analysis registry");
        let entry_map = passes.entry(manager_key).or_default();
        assert!(
            !entry_map.contains_key(&analysis_key),
            "analysis already registered"
        );
        entry_map.insert(
            analysis_key,
            CgsccAnalysisEntry {
                pass: Box::into_raw(Box::new(pass)) as *mut c_void,
                pass_deleter: pass_deleter::<P>,
                pass_entrypoint: pass_entrypoint::<P>,
            },
        );
    }

    /// Get (or compute) analysis result for a function in current SCC.
    pub fn get_result<A>(&self, function: &inkwell::values::FunctionValue<'_>) -> &A::Result
    where
        A: LlvmCgsccAnalysis + 'static,
        A::Result: 'static,
    {
        let id = A::id();
        assert!(
            !matches!(self.from_analysis_id, Some(n) if n == id),
            "Analysis cannot request its own result"
        );

        let manager_key = self.raw as usize;
        let analysis_key = id as usize;
        let function_key = function.as_value_ref() as usize;

        if let Some(result) = self.get_cached_result::<A>(function) {
            return result;
        }

        let passes = cgscc_analysis_passes()
            .lock()
            .expect("poisoned cgscc analysis registry");
        let entry = passes
            .get(&manager_key)
            .and_then(|m| m.get(&analysis_key))
            .expect("analysis was not registered");
        let result_ptr = (entry.pass_entrypoint)(entry.pass, function, self.raw);
        drop(passes);

        let mut cache = cgscc_analysis_cache()
            .lock()
            .expect("poisoned cgscc analysis cache");
        cache
            .entry(manager_key)
            .or_default()
            .insert((analysis_key, function_key), result_ptr as usize);
        // SAFETY: result_ptr points to a leaked Box<A::Result> stored in cache.
        unsafe { &*(result_ptr as *const A::Result) }
    }

    /// Get cached analysis result if present.
    pub fn get_cached_result<A>(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
    ) -> Option<&A::Result>
    where
        A: LlvmCgsccAnalysis + 'static,
        A::Result: 'static,
    {
        let id = A::id();
        assert!(
            !matches!(self.from_analysis_id, Some(n) if n == id),
            "Analysis cannot request its own result"
        );

        let manager_key = self.raw as usize;
        let analysis_key = id as usize;
        let function_key = function.as_value_ref() as usize;
        let cache = cgscc_analysis_cache()
            .lock()
            .expect("poisoned cgscc analysis cache");
        cache
            .get(&manager_key)
            .and_then(|m| m.get(&(analysis_key, function_key)))
            // SAFETY: cached pointer values are inserted only from `Box<A::Result>` in
            // `get_result`, keyed by the same analysis type `A`.
            .map(|ptr| unsafe { &*((*ptr as *const c_void) as *const A::Result) })
    }
}

/// Analysis manager handle for loop passes and analyses.
pub struct LoopAnalysisManager {
    raw: *mut c_void,
    from_analysis_id: Option<AnalysisKey>,
}

impl LoopAnalysisManager {
    /// Construct from an opaque manager pointer passed by the C++ layer.
    ///
    /// # Safety
    /// `raw` must be a valid pointer to LLVM's `LoopAnalysisManager`.
    pub unsafe fn from_raw(raw: *mut c_void) -> Self {
        Self {
            raw,
            from_analysis_id: None,
        }
    }

    unsafe fn from_raw_with_analysis_id(
        raw: *mut c_void,
        from_analysis_id: Option<AnalysisKey>,
    ) -> Self {
        Self {
            raw,
            from_analysis_id,
        }
    }

    /// Get the underlying opaque pointer.
    pub fn as_raw(&self) -> *mut c_void {
        self.raw
    }

    /// Register a loop analysis pass.
    pub fn add_analysis<P>(&self, pass: P)
    where
        P: LlvmLoopAnalysis,
        P: 'static,
        <P as LlvmLoopAnalysis>::Result: 'static,
    {
        extern "C" fn pass_deleter<T: LlvmLoopAnalysis>(pass: *mut c_void) {
            // SAFETY: Pointer was created by Box::into_raw in add_pass.
            drop(unsafe { Box::<T>::from_raw(pass.cast()) });
        }

        fn pass_entrypoint<T>(
            pass: *mut c_void,
            loop_header: LLVMBasicBlockRef,
            manager: *mut c_void,
        ) -> *mut c_void
        where
            T: LlvmLoopAnalysis,
            T::Result: 'static,
        {
            // SAFETY: pass is a valid pointer to T stored in registry.
            let pass = unsafe { &*(pass as *const T) };
            // SAFETY: manager is valid for callback duration.
            let manager =
                unsafe { LoopAnalysisManager::from_raw_with_analysis_id(manager, Some(T::id())) };
            let result = pass.run_analysis(loop_header, &manager);
            Box::into_raw(Box::new(result)) as *mut c_void
        }

        let manager_key = self.raw as usize;
        let analysis_key = <P as LlvmLoopAnalysis>::id() as usize;
        let mut passes = loop_analysis_passes()
            .lock()
            .expect("poisoned loop analysis registry");
        let entry_map = passes.entry(manager_key).or_default();
        assert!(
            !entry_map.contains_key(&analysis_key),
            "analysis already registered"
        );
        entry_map.insert(
            analysis_key,
            LoopAnalysisEntry {
                pass: Box::into_raw(Box::new(pass)) as *mut c_void,
                pass_deleter: pass_deleter::<P>,
                pass_entrypoint: pass_entrypoint::<P>,
            },
        );
    }

    /// Get (or compute) analysis result for a loop header.
    pub fn get_result<A>(&self, loop_header: LLVMBasicBlockRef) -> &A::Result
    where
        A: LlvmLoopAnalysis + 'static,
        A::Result: 'static,
    {
        let id = A::id();
        assert!(
            !matches!(self.from_analysis_id, Some(n) if n == id),
            "Analysis cannot request its own result"
        );

        let manager_key = self.raw as usize;
        let analysis_key = id as usize;
        let loop_key = loop_header as usize;

        if let Some(result) = self.get_cached_result::<A>(loop_header) {
            return result;
        }

        let passes = loop_analysis_passes()
            .lock()
            .expect("poisoned loop analysis registry");
        let entry = passes
            .get(&manager_key)
            .and_then(|m| m.get(&analysis_key))
            .expect("analysis was not registered");
        let result_ptr = (entry.pass_entrypoint)(entry.pass, loop_header, self.raw);
        drop(passes);

        let mut cache = loop_analysis_cache()
            .lock()
            .expect("poisoned loop analysis cache");
        cache
            .entry(manager_key)
            .or_default()
            .insert((analysis_key, loop_key), result_ptr as usize);
        // SAFETY: result_ptr points to a leaked Box<A::Result> stored in cache.
        unsafe { &*(result_ptr as *const A::Result) }
    }

    /// Get cached analysis result if present.
    pub fn get_cached_result<A>(&self, loop_header: LLVMBasicBlockRef) -> Option<&A::Result>
    where
        A: LlvmLoopAnalysis + 'static,
        A::Result: 'static,
    {
        let id = A::id();
        assert!(
            !matches!(self.from_analysis_id, Some(n) if n == id),
            "Analysis cannot request its own result"
        );

        let manager_key = self.raw as usize;
        let analysis_key = id as usize;
        let loop_key = loop_header as usize;
        let cache = loop_analysis_cache()
            .lock()
            .expect("poisoned loop analysis cache");
        cache
            .get(&manager_key)
            .and_then(|m| m.get(&(analysis_key, loop_key)))
            // SAFETY: cached pointer values are inserted only from `Box<A::Result>` in
            // `get_result`, keyed by the same analysis type `A`.
            .map(|ptr| unsafe { &*((*ptr as *const c_void) as *const A::Result) })
    }
}

struct CgsccAnalysisEntry {
    pass: *mut c_void,
    pass_deleter: extern "C" fn(*mut c_void),
    pass_entrypoint:
        fn(*mut c_void, &inkwell::values::FunctionValue<'_>, *mut c_void) -> *mut c_void,
}

struct LoopAnalysisEntry {
    pass: *mut c_void,
    pass_deleter: extern "C" fn(*mut c_void),
    pass_entrypoint: fn(*mut c_void, LLVMBasicBlockRef, *mut c_void) -> *mut c_void,
}

impl Drop for CgsccAnalysisEntry {
    fn drop(&mut self) {
        (self.pass_deleter)(self.pass);
    }
}

impl Drop for LoopAnalysisEntry {
    fn drop(&mut self) {
        (self.pass_deleter)(self.pass);
    }
}

// SAFETY: Entries are only accessed under a mutex and contain raw pointers to heap
// allocations owned by this process.
unsafe impl Send for CgsccAnalysisEntry {}
unsafe impl Send for LoopAnalysisEntry {}

struct SyncOnceCell<T>(OnceCell<T>);

impl<T> SyncOnceCell<T> {
    const fn new() -> Self {
        Self(OnceCell::new())
    }
}

// SAFETY: Access to initialization is synchronized externally via `Once`.
// After initialization, only shared references are handed out and `T: Sync`.
unsafe impl<T: Sync> Sync for SyncOnceCell<T> {}

fn cgscc_analysis_passes() -> &'static Mutex<HashMap<usize, HashMap<usize, CgsccAnalysisEntry>>> {
    static INIT: Once = Once::new();
    static CELL: SyncOnceCell<Mutex<HashMap<usize, HashMap<usize, CgsccAnalysisEntry>>>> =
        SyncOnceCell::new();
    INIT.call_once(|| {
        let _ = CELL.0.set(Mutex::new(HashMap::new()));
    });
    CELL.0.get().expect("cgscc registry initialized")
}

fn cgscc_analysis_cache() -> &'static Mutex<HashMap<usize, HashMap<(usize, usize), usize>>> {
    static INIT: Once = Once::new();
    static CELL: SyncOnceCell<Mutex<HashMap<usize, HashMap<(usize, usize), usize>>>> =
        SyncOnceCell::new();
    INIT.call_once(|| {
        let _ = CELL.0.set(Mutex::new(HashMap::new()));
    });
    CELL.0.get().expect("cgscc cache initialized")
}

fn loop_analysis_passes() -> &'static Mutex<HashMap<usize, HashMap<usize, LoopAnalysisEntry>>> {
    static INIT: Once = Once::new();
    static CELL: SyncOnceCell<Mutex<HashMap<usize, HashMap<usize, LoopAnalysisEntry>>>> =
        SyncOnceCell::new();
    INIT.call_once(|| {
        let _ = CELL.0.set(Mutex::new(HashMap::new()));
    });
    CELL.0.get().expect("loop registry initialized")
}

fn loop_analysis_cache() -> &'static Mutex<HashMap<usize, HashMap<(usize, usize), usize>>> {
    static INIT: Once = Once::new();
    static CELL: SyncOnceCell<Mutex<HashMap<usize, HashMap<(usize, usize), usize>>>> =
        SyncOnceCell::new();
    INIT.call_once(|| {
        let _ = CELL.0.set(Mutex::new(HashMap::new()));
    });
    CELL.0.get().expect("loop cache initialized")
}

fn preserved_to_c(pa: traits::PreservedAnalyses) -> std::ffi::c_int {
    match pa {
        PreservedAnalyses::All => 0,
        PreservedAnalyses::None => 1,
    }
}

/// Generic trampoline for module passes. Monomorphized per `T`.
///
/// # Safety
/// `user_data` must be a valid pointer to a `T` that was stored in
/// [`ModulePassManager::_passes`]. `module` must be a valid `LLVMModuleRef`.
unsafe extern "C" fn module_pass_trampoline<T: LlvmModulePass>(
    module: LLVMModuleRef,
    manager: *mut c_void,
    user_data: *mut c_void,
) -> std::ffi::c_int {
    // SAFETY: user_data was cast from &T stored in a Box inside _passes.
    let pass = &*(user_data as *const T);
    let mut module = inkwell::module::Module::new(module);
    let manager = ModuleAnalysisManager::from_raw(manager, None);
    let result = pass.run_pass(&mut module, &manager);
    std::mem::forget(module);
    preserved_to_c(result)
}

/// Generic trampoline for function passes. Monomorphized per `T`.
///
/// # Safety
/// `user_data` must be a valid pointer to a `T` that was stored in
/// [`FunctionPassManager::_passes`]. `function` must be a valid `LLVMValueRef`.
unsafe extern "C" fn function_pass_trampoline<T: LlvmFunctionPass>(
    function: LLVMValueRef,
    manager: *mut c_void,
    user_data: *mut c_void,
) -> std::ffi::c_int {
    let pass = &*(user_data as *const T);
    let mut function = inkwell::values::FunctionValue::new(function).expect("invalid function");
    let manager = FunctionAnalysisManager::from_raw(manager, None);
    let result = pass.run_pass(&mut function, &manager);
    preserved_to_c(result)
}

/// Generic trampoline for CGSCC passes. Monomorphized per `T`.
///
/// # Safety
/// `user_data` must be a valid pointer to a `T` that was stored in
/// [`ModulePassManager::_passes`]. `function` must be a valid `LLVMValueRef`.
unsafe extern "C" fn cgscc_pass_trampoline<T: LlvmCgsccPass>(
    function: LLVMValueRef,
    manager: *mut c_void,
    user_data: *mut c_void,
) -> std::ffi::c_int {
    let pass = &*(user_data as *const T);
    let mut function = inkwell::values::FunctionValue::new(function).expect("invalid function");
    let manager = CgsccAnalysisManager::from_raw(manager);
    let result = pass.run_pass(&mut function, &manager);
    preserved_to_c(result)
}

/// Generic trampoline for loop passes. Monomorphized per `T`.
///
/// # Safety
/// `user_data` must be a valid pointer to a `T` that was stored in
/// [`FunctionPassManager::_passes`]. `header` must be a valid `LLVMBasicBlockRef`.
unsafe extern "C" fn loop_pass_trampoline<T: LlvmLoopPass>(
    header: LLVMBasicBlockRef,
    manager: *mut c_void,
    user_data: *mut c_void,
) -> std::ffi::c_int {
    let pass = &*(user_data as *const T);
    let manager = LoopAnalysisManager::from_raw(manager);
    let result = pass.run_pass(header, &manager);
    preserved_to_c(result)
}

// =========================================================================
// ModulePassManager
// =========================================================================

/// A configured LLVM module pass manager using the new PassManager infrastructure.
///
/// Bundles all analysis managers, PassBuilder, StandardInstrumentations, and the
/// ModulePassManager into a single object with correct lifetime management.
///
/// The lifetime `'a` is tied to the borrowed [`inkwell::targets::TargetMachine`],
/// ensuring the target machine outlives the pass manager.
pub struct ModulePassManager<'a> {
    raw: llvm_pm_sys::LlvmPmPassManagerRef,
    _passes: Vec<Box<dyn std::any::Any>>,
    _phantom: PhantomData<&'a ()>,
}

impl<'a> fmt::Debug for ModulePassManager<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModulePassManager")
            .field("raw", &self.raw)
            .finish()
    }
}

impl<'a> ModulePassManager<'a> {
    /// Create a pass manager with a standard optimization pipeline.
    pub fn with_opt_level(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        level: OptLevel,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or a valid LLVMTargetMachineRef from inkwell.
        // opts is either null or a valid options handle. err_msg is a valid out-pointer.
        let raw = unsafe {
            llvm_pm_sys::llvm_pm_create_with_opt_level(tm, level.to_c(), opts, &mut err_msg)
        };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create a pass manager from a textual pipeline description.
    ///
    /// The format matches `opt -passes=...` syntax, e.g.:
    /// - `"instcombine,dce,sroa"`
    /// - `"default<O2>"`
    /// - `"module(function(instcombine,sroa))"`
    pub fn with_pipeline(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        pipeline: &str,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let c_pipeline = CString::new(pipeline).map_err(|e| Error {
            message: format!("Pipeline string contains null byte: {}", e),
        })?;
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. c_pipeline is a valid null-terminated string.
        let raw = unsafe {
            llvm_pm_sys::llvm_pm_create_with_pipeline(tm, c_pipeline.as_ptr(), opts, &mut err_msg)
        };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create a pass manager with the full-LTO default pipeline.
    pub fn with_lto(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        level: OptLevel,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. opts is either null or valid.
        let raw = unsafe { llvm_pm_sys::llvm_pm_create_lto(tm, level.to_c(), opts, &mut err_msg) };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create a pass manager with the full-LTO pre-link pipeline.
    pub fn with_lto_pre_link(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        level: OptLevel,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. opts is either null or valid.
        let raw = unsafe {
            llvm_pm_sys::llvm_pm_create_lto_pre_link(tm, level.to_c(), opts, &mut err_msg)
        };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create a pass manager with the ThinLTO pre-link pipeline.
    pub fn with_thin_lto_pre_link(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        level: OptLevel,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. opts is either null or valid.
        let raw = unsafe {
            llvm_pm_sys::llvm_pm_create_thin_lto_pre_link(tm, level.to_c(), opts, &mut err_msg)
        };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create an empty module pass manager with no built-in passes.
    ///
    /// Use [`add_pass()`](ModulePassManager::add_pass) to add custom passes.
    pub fn new(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. opts is either null or valid.
        let raw = unsafe { llvm_pm_sys::llvm_pm_create_empty_module(tm, opts, &mut err_msg) };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Add a custom module pass to this pass manager.
    ///
    /// The pass is moved into the pass manager and kept alive until the pass
    /// manager is dropped. The pass is appended after any existing passes in
    /// the pipeline.
    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: LlvmModulePass + 'static,
    {
        let mut boxed = Box::new(pass);
        let ptr = &mut *boxed as *mut P as *mut c_void;
        // SAFETY: self.raw is a valid PM handle. module_pass_trampoline::<T> is the
        // correct monomorphized trampoline. The pass pointer is valid for the
        // lifetime of the PM because the Box is stored in _passes.
        unsafe {
            llvm_pm_sys::llvm_pm_add_module_pass(self.raw, Some(module_pass_trampoline::<P>), ptr);
        }
        self._passes.push(boxed);
    }

    /// Add a custom CGSCC pass to this pass manager.
    ///
    /// The pass is adapted into the module pipeline and invoked for each
    /// function in each SCC.
    pub fn add_cgscc_pass<P>(&mut self, pass: P)
    where
        P: LlvmCgsccPass + 'static,
    {
        let mut boxed = Box::new(pass);
        let ptr = &mut *boxed as *mut P as *mut c_void;
        // SAFETY: self.raw is a valid PM handle. cgscc_pass_trampoline::<T> is the
        // correct monomorphized trampoline. The pass pointer is valid for the
        // lifetime of the PM because the Box is stored in _passes.
        unsafe {
            llvm_pm_sys::llvm_pm_add_cgscc_pass(self.raw, Some(cgscc_pass_trampoline::<P>), ptr);
        }
        self._passes.push(boxed);
    }

    /// Run the optimization passes on the given module.
    pub fn run(&mut self, module: &inkwell::module::Module<'_>) -> Result<(), Error> {
        // SAFETY: self.raw is a valid PM handle. module.as_mut_ptr() returns a valid
        // LLVMModuleRef. &mut self ensures exclusive access to stored passes.
        let err = unsafe { llvm_pm_sys::llvm_pm_run(self.raw, module.as_mut_ptr()) };
        if err.is_null() {
            Ok(())
        } else {
            // SAFETY: On error, llvm_pm_run returns a valid malloc'd error string.
            Err(unsafe { consume_c_error(err) })
        }
    }
}

impl<'a> Drop for ModulePassManager<'a> {
    fn drop(&mut self) {
        // SAFETY: self.raw is a valid PM handle, and Drop runs at most once.
        // The C++ side is disposed first; _passes (stored Box<dyn Any>) are
        // dropped automatically afterward.
        unsafe {
            llvm_pm_sys::llvm_pm_dispose(self.raw);
        }
    }
}

// SAFETY: The bundled pass manager owns all its internal state (analysis managers,
// PassBuilder, etc.) and does not share mutable state with other instances.
// Stored passes are only accessed via &mut self methods.
// Moving between threads is safe; concurrent access is prevented by requiring
// exclusive access for operations.
unsafe impl<'a> Send for ModulePassManager<'a> {}

// =========================================================================
// FunctionPassManager
// =========================================================================

/// A configured LLVM function pass manager using the new PassManager infrastructure.
///
/// Similar to [`ModulePassManager`] but runs function-level passes on individual
/// functions rather than entire modules.
///
/// The lifetime `'a` is tied to the borrowed [`inkwell::targets::TargetMachine`],
/// ensuring the target machine outlives the pass manager.
pub struct FunctionPassManager<'a> {
    raw: llvm_pm_sys::LlvmPmPassManagerRef,
    _passes: Vec<Box<dyn std::any::Any>>,
    _phantom: PhantomData<&'a ()>,
}

impl<'a> fmt::Debug for FunctionPassManager<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionPassManager")
            .field("raw", &self.raw)
            .finish()
    }
}

impl<'a> FunctionPassManager<'a> {
    /// Create a function pass manager from a textual pipeline description.
    ///
    /// The pipeline should contain function-level passes, e.g. `"instcombine,dce"`.
    pub fn with_pipeline(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        pipeline: &str,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let c_pipeline = CString::new(pipeline).map_err(|e| Error {
            message: format!("Pipeline string contains null byte: {}", e),
        })?;
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. c_pipeline is a valid null-terminated string.
        let raw = unsafe {
            llvm_pm_sys::llvm_pm_create_function_with_pipeline(
                tm,
                c_pipeline.as_ptr(),
                opts,
                &mut err_msg,
            )
        };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Create an empty function pass manager with no built-in passes.
    ///
    /// Use [`add_pass()`](FunctionPassManager::add_pass) to add custom passes.
    pub fn new(
        target_machine: Option<&'a inkwell::targets::TargetMachine>,
        options: Option<&Options>,
    ) -> Result<Self, Error> {
        let tm = target_machine.map_or(ptr::null_mut(), |t| t.as_mut_ptr());
        let opts = options.map_or(ptr::null_mut(), |o| o.raw);
        let mut err_msg: *mut std::ffi::c_char = ptr::null_mut();

        // SAFETY: tm is either null or valid. opts is either null or valid.
        let raw = unsafe { llvm_pm_sys::llvm_pm_create_empty_function(tm, opts, &mut err_msg) };

        if raw.is_null() {
            // SAFETY: On error, the C++ stub sets err_msg to a valid malloc'd string.
            Err(unsafe { consume_c_error(err_msg) })
        } else {
            Ok(Self {
                raw,
                _passes: Vec::new(),
                _phantom: PhantomData,
            })
        }
    }

    /// Add a custom function pass to this pass manager.
    ///
    /// The pass is moved into the pass manager and kept alive until the pass
    /// manager is dropped. The pass is appended after any existing passes in
    /// the pipeline.
    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: LlvmFunctionPass + 'static,
    {
        let mut boxed = Box::new(pass);
        let ptr = &mut *boxed as *mut P as *mut c_void;
        // SAFETY: self.raw is a valid PM handle. function_pass_trampoline::<T> is the
        // correct monomorphized trampoline. The pass pointer is valid for the
        // lifetime of the PM because the Box is stored in _passes.
        unsafe {
            llvm_pm_sys::llvm_pm_add_function_pass(
                self.raw,
                Some(function_pass_trampoline::<P>),
                ptr,
            );
        }
        self._passes.push(boxed);
    }

    /// Add a custom loop pass to this pass manager.
    ///
    /// The pass is adapted into the function pipeline and invoked with each
    /// loop header basic block.
    pub fn add_loop_pass<P>(&mut self, pass: P)
    where
        P: traits::LlvmLoopPass + 'static,
    {
        let mut boxed = Box::new(pass);
        let ptr = &mut *boxed as *mut P as *mut c_void;
        // SAFETY: self.raw is a valid PM handle. loop_pass_trampoline::<T> is the
        // correct monomorphized trampoline. The pass pointer is valid for the
        // lifetime of the PM because the Box is stored in _passes.
        unsafe {
            llvm_pm_sys::llvm_pm_add_loop_pass(self.raw, Some(loop_pass_trampoline::<P>), ptr);
        }
        self._passes.push(boxed);
    }

    /// Run the function passes on the given function.
    pub fn run(&mut self, function: inkwell::values::FunctionValue<'_>) -> Result<(), Error> {
        use inkwell::values::AsValueRef;
        // SAFETY: self.raw is a valid PM handle. function.as_value_ref() returns a valid
        // LLVMValueRef. &mut self ensures exclusive access to stored passes.
        let err =
            unsafe { llvm_pm_sys::llvm_pm_run_on_function(self.raw, function.as_value_ref()) };
        if err.is_null() {
            Ok(())
        } else {
            // SAFETY: On error, llvm_pm_run_on_function returns a valid malloc'd error string.
            Err(unsafe { consume_c_error(err) })
        }
    }
}

impl<'a> Drop for FunctionPassManager<'a> {
    fn drop(&mut self) {
        // SAFETY: self.raw is a valid PM handle, and Drop runs at most once.
        // The C++ side is disposed first; _passes (stored Box<dyn Any>) are
        // dropped automatically afterward.
        unsafe {
            llvm_pm_sys::llvm_pm_dispose(self.raw);
        }
    }
}

// SAFETY: The bundled pass manager owns all its internal state (analysis managers,
// PassBuilder, etc.) and does not share mutable state with other instances.
// Stored passes are only accessed via &mut self methods.
// Moving between threads is safe; concurrent access is prevented by requiring
// exclusive (&mut) access for operations.
unsafe impl<'a> Send for FunctionPassManager<'a> {}
