//! Regression tests that verify the audit findings recorded in `SAFETY_AUDIT.md`
//! have been *fixed*.
//!
//! The library has been corrected, so every test below asserts the CORRECT
//! post-fix behavior. Each test pins one finding so that a future regression
//! re-introducing the bug breaks loudly. The verified fixes are:
//!
//! | Finding | Test | Kind |
//! |---------|------|------|
//! | #1 / N3 `Send` impl now matched by a `Send` bound on passes | `finding1_*` | compile + positive |
//! | #3 panic in a pass no longer aborts across `extern "C"` | `finding3_*` | subprocess |
//! | #4 registered analysis pass is freed on PM dispose | `finding4_*` | deterministic |
//! | N1 `id()` collision no longer causes type confusion | `finding_new_analysis_id_*` | deterministic |
//! | N2 dependent-analysis request no longer deadlocks | `finding_new_dependent_*` | subprocess |

use llvm_pm::inkwell;
use llvm_pm::inkwell::context::Context;
use llvm_pm::traits::{LlvmCgsccAnalysis, LlvmCgsccPass, LlvmModulePass, PreservedAnalyses};
use llvm_pm::{CgsccAnalysisManager, ModuleAnalysisManager, ModulePassManager};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Build a leaked-context module with `i32 @f(i32, i32)` and return a stable
/// `FunctionValue`. The context is leaked so the value stays `'static` for the
/// duration of the test (cleaned up at process exit).
fn make_add_function() -> (
    inkwell::module::Module<'static>,
    inkwell::values::FunctionValue<'static>,
) {
    let ctx: &'static Context = Box::leak(Box::new(Context::create()));
    let module = ctx.create_module("audit");
    let i32_ty = ctx.i32_type();
    let fn_ty = i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false);
    let func = module.add_function("f", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    let b = ctx.create_builder();
    b.position_at_end(bb);
    let a = func.get_nth_param(0).unwrap().into_int_value();
    let c = func.get_nth_param(1).unwrap().into_int_value();
    let s = b.build_int_add(a, c, "s").unwrap();
    b.build_return(Some(&s)).unwrap();
    (module, func)
}

/// A unique, permanently-reserved pointer value to use as an analysis-manager key.
///
/// `CgsccAnalysisManager`/`LoopAnalysisManager` use the manager pointer purely as a
/// `usize` map key for their per-manager registry and cache — they never
/// dereference it for bookkeeping. Leaking a `Box` gives a unique address that no
/// real LLVM analysis manager can ever alias, so this drives the exact same
/// bookkeeping path a genuine manager would, deterministically.
fn unique_manager_key() -> *mut c_void {
    Box::into_raw(Box::new(0u8)) as *mut c_void
}

// =========================================================================
// Finding #1 / N3 — `unsafe impl Send` on the pass managers is now matched by a
// `P: Send` bound on the `add_pass` family. A `Send` pass works and the pass
// manager remains `Send`; a `!Send` pass no longer compiles.
// =========================================================================

#[test]
fn finding1_send_pass_works_and_pm_is_send() {
    // A pass holding `Arc<AtomicU32>` is `Send`.
    struct SendPass {
        ran: Arc<AtomicU32>,
    }
    impl LlvmModulePass for SendPass {
        fn run_pass(
            &self,
            _module: &mut inkwell::module::Module<'_>,
            _manager: &ModuleAnalysisManager,
        ) -> PreservedAnalyses {
            self.ran.fetch_add(1, Ordering::SeqCst);
            PreservedAnalyses::All
        }
    }

    // `ModulePassManager` is `Send`, and now that `add_pass` requires `P: Send`,
    // that `Send` impl is sound.
    fn assert_send<T: Send>() {}
    assert_send::<ModulePassManager>();

    // The inkwell `Module` is `!Send`, so we build the module and run the PM on
    // the same thread; the assertion above already pins the `Send`-ness of the PM.
    let ran = Arc::new(AtomicU32::new(0));
    let (module, _f) = make_add_function();
    let mut pm = ModulePassManager::new(None, None).expect("create empty PM");
    pm.add_pass(SendPass { ran: ran.clone() });
    pm.run(&module).expect("run PM");
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "the Send pass must have run exactly once"
    );

    // REGRESSION MARKER (SAFETY_AUDIT #1 / N3): a `!Send` pass now FAILS to
    // compile because `add_pass` requires `P: Send`. The snippet below is kept
    // commented out — uncommenting it must break the build, which is the fix
    // landing:
    //
    //     use std::cell::Cell;
    //     use std::rc::Rc;
    //     struct NonSendPass { shared: Rc<Cell<u32>> } // Rc<Cell<..>> is !Send
    //     impl LlvmModulePass for NonSendPass {
    //         fn run_pass(&self, _m: &mut inkwell::module::Module<'_>,
    //                     _am: &ModuleAnalysisManager) -> PreservedAnalyses {
    //             self.shared.set(self.shared.get() + 1);
    //             PreservedAnalyses::All
    //         }
    //     }
    //     let mut pm = ModulePassManager::new(None, None).unwrap();
    //     pm.add_pass(NonSendPass { shared: Rc::new(Cell::new(0)) }); // E0277: !Send
}

// =========================================================================
// Finding #3 — a panic inside a user pass used to unwind across the `extern "C"`
// trampoline and abort the process. After the fix the trampoline catches the
// unwind, treats it as `PreservedAnalyses::None`, and `run()` returns normally.
// Realized in a re-executed child process.
// =========================================================================

const ABORT_CHILD_ENV: &str = "LLVM_PM_AUDIT_ABORT_CHILD";

#[test]
fn finding3_panic_in_pass_no_longer_aborts_across_ffi() {
    // --- Child role: provoke a panic inside a pass. ---
    if std::env::var(ABORT_CHILD_ENV).is_ok() {
        struct PanicPass;
        impl LlvmModulePass for PanicPass {
            fn run_pass(
                &self,
                _module: &mut inkwell::module::Module<'_>,
                _manager: &ModuleAnalysisManager,
            ) -> PreservedAnalyses {
                panic!("audit: panic inside a pass, caught by the trampoline");
            }
        }
        let (module, _f) = make_add_function();
        let mut pm = ModulePassManager::new(None, None).expect("create PM");
        pm.add_pass(PanicPass);
        // After the fix (catch_unwind in the trampoline) this returns normally;
        // the panic is swallowed as PreservedAnalyses::None.
        let _ = pm.run(&module);
        // Reaching this point means the panic did NOT abort the process.
        std::process::exit(0);
    }

    // --- Parent role: re-exec this exact test in a child with the env var set. ---
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "--nocapture",
            "finding3_panic_in_pass_no_longer_aborts_across_ffi",
        ])
        .env(ABORT_CHILD_ENV, "1")
        .output()
        .expect("spawn child test process");

    // After the fix the child must reach `exit(0)` cleanly.
    assert!(
        output.status.success(),
        "SAFETY_AUDIT #3: a panic in a pass must be caught at the extern \"C\" \
         boundary and the child must exit successfully, but it did not.\n\
         --- child stderr ---\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // On Unix, an abort would terminate the process via a signal; a graceful exit
    // carries no signal. The absence of any termination signal confirms no abort.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            output.status.signal().is_none(),
            "SAFETY_AUDIT #3: the child must NOT be killed by an abort signal, \
             but it was terminated by signal {:?}.\n--- child stderr ---\n{}",
            output.status.signal(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// =========================================================================
// Finding #4 — the per-manager analysis registry/cache used to be process-global
// and survive the drop of the pass manager that created them, leaking the
// registered pass `Box`. After the fix, dropping the PM clears its per-manager
// registry/cache, so the registered pass is freed.
// =========================================================================

/// A drop-flag whose `Drop` records that it ran, so we can observe whether the
/// registered analysis pass `Box` is ever freed.
struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A CGSCC analysis whose pass value carries a drop-flag.
struct LeakProbeAnalysis {
    _flag: DropFlag,
}

impl LlvmCgsccAnalysis for LeakProbeAnalysis {
    type Result = u32;
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        0
    }
    fn id() -> *const u8 {
        static ID: u8 = 0;
        &ID
    }
}

/// A CGSCC pass that, on its first invocation, registers `LeakProbeAnalysis`
/// against the live analysis manager and caches a result for it.
struct RegisterOnce {
    flag: Arc<AtomicBool>,
    registered: Arc<AtomicU32>,
}

impl LlvmCgsccPass for RegisterOnce {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        // Register exactly once, even though the pass may run for several SCCs.
        if self
            .registered
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(LeakProbeAnalysis {
                _flag: DropFlag(self.flag.clone()),
            });
            // Also cache a result so the per-manager cache holds an entry.
            let _ = manager.get_result::<LeakProbeAnalysis>(function);
        }
        PreservedAnalyses::All
    }
}

#[test]
fn finding4_registered_analysis_pass_is_freed_on_pm_dispose() {
    let dropped = Arc::new(AtomicBool::new(false));
    {
        let (module, _f) = make_add_function();
        let mut pm = ModulePassManager::new(None, None).unwrap();
        pm.add_cgscc_pass(RegisterOnce {
            flag: dropped.clone(),
            registered: Arc::new(AtomicU32::new(0)),
        });
        pm.run(&module).unwrap();
        assert!(
            !dropped.load(Ordering::SeqCst),
            "pass still alive while PM lives"
        );
    } // PM dropped -> clear_analysis_state frees the registered pass Box.

    assert!(
        dropped.load(Ordering::SeqCst),
        "registered analysis pass must be freed on PM dispose"
    );
}

// =========================================================================
// Finding N1 — analyses used to be keyed on the user-supplied `id()`, so two
// distinct Rust types that returned the SAME `id()` aliased each other's cached
// results (type confusion). After the fix the registry/cache are keyed by
// `TypeId`, so requesting an unregistered type — even one with a colliding
// `id()` — is rejected ("analysis was not registered") rather than served the
// other type's bytes.
// =========================================================================

// Both analyses deliberately return the SAME id() by pointing at this static,
// yet they are DISTINCT Rust types with distinct `TypeId`s.
static SHARED_ANALYSIS_ID: u8 = 0;

struct RegisteredAnalysis;
impl LlvmCgsccAnalysis for RegisteredAnalysis {
    type Result = u32;
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        0xCAFE_B0BA
    }
    fn id() -> *const u8 {
        &SHARED_ANALYSIS_ID
    }
}

struct UnregisteredImpostor;
impl LlvmCgsccAnalysis for UnregisteredImpostor {
    type Result = u32; // same layout as RegisteredAnalysis::Result
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        unreachable!("never registered, so never actually computed")
    }
    fn id() -> *const u8 {
        &SHARED_ANALYSIS_ID // COLLIDES with RegisteredAnalysis::id()
    }
}

#[test]
fn finding_new_analysis_id_collision_no_longer_causes_type_confusion() {
    let (_module, func) = make_add_function();
    let key = unique_manager_key();
    // SAFETY: opaque key only.
    let m = unsafe { CgsccAnalysisManager::from_raw(key) };

    m.add_analysis(RegisteredAnalysis); // only this analysis is registered
    assert_eq!(*m.get_result::<RegisteredAnalysis>(&func), 0xCAFE_B0BA);

    // `UnregisteredImpostor` was never registered. Because the registry/cache are
    // now keyed by `TypeId` (not by the colliding `id()`), requesting it must
    // PANIC with "analysis was not registered" rather than handing back
    // `RegisteredAnalysis`'s bytes. After the fix this panic happens with NO lock
    // held, so it does not poison the global registry/cache for other tests.
    let confused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = m.get_result::<UnregisteredImpostor>(&func);
    }));
    assert!(
        confused.is_err(),
        "an unregistered analysis with a colliding id() must be rejected \
         (no cross-type cache hit) — the request must panic"
    );
}

// =========================================================================
// Finding N2 — `get_result` used to invoke the user `run_analysis` callback while
// still holding the global analysis-passes `Mutex`. A dependent analysis that
// requested a *different* analysis re-entered that non-reentrant lock on the same
// thread and deadlocked. After the fix the lock is released before the callback
// runs, so dependent analyses complete. Realized in a child process under a
// watchdog so a regression cannot wedge the test runner.
// =========================================================================

const DEADLOCK_CHILD_ENV: &str = "LLVM_PM_AUDIT_DEADLOCK_CHILD";

struct DependencyAnalysis;
struct DependentAnalysis;

impl LlvmCgsccAnalysis for DependencyAnalysis {
    type Result = u32;
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        0
    }
    fn id() -> *const u8 {
        static ID: u8 = 0;
        &ID
    }
}

impl LlvmCgsccAnalysis for DependentAnalysis {
    type Result = u32;
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        // Requesting a *different* analysis here used to re-lock the held mutex.
        // After the fix the lock is released before this callback runs, so this
        // nested request completes normally.
        *manager.get_result::<DependencyAnalysis>(function)
    }
    fn id() -> *const u8 {
        static ID: u8 = 0;
        &ID
    }
}

#[test]
fn finding_new_dependent_analysis_request_no_longer_deadlocks() {
    // --- Child role: request a dependent analysis. ---
    if std::env::var(DEADLOCK_CHILD_ENV).is_ok() {
        let (_module, func) = make_add_function();
        let key = unique_manager_key();
        // SAFETY: opaque key only.
        let m = unsafe { CgsccAnalysisManager::from_raw(key) };
        // Register both analyses against the SAME manager.
        m.add_analysis(DependencyAnalysis);
        m.add_analysis(DependentAnalysis);
        // After the fix this completes without deadlock.
        let _ = m.get_result::<DependentAnalysis>(&func);
        std::process::exit(0); // reached only if NO deadlock occurs
    }

    // --- Parent role: run the child under a watchdog and require it to finish. ---
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            "--nocapture",
            "finding_new_dependent_analysis_request_no_longer_deadlocks",
        ])
        .env(DEADLOCK_CHILD_ENV, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn child");

    // The child does almost no work; if it does NOT deadlock it finishes well
    // within this window. ~3s is ample headroom.
    let mut deadlocked = true;
    let mut child_succeeded = false;
    for _ in 0..30 {
        if let Some(status) = child.try_wait().expect("try_wait") {
            deadlocked = false;
            child_succeeded = status.success();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if deadlocked {
        let _ = child.kill();
        let _ = child.wait();
    }

    // After the fix the dependent-analysis request releases the lock before the
    // callback runs, so the child completes and exits successfully.
    assert!(
        !deadlocked && child_succeeded,
        "a dependent-analysis request from within run_analysis must complete \
         without deadlock (deadlocked = {deadlocked}, succeeded = {child_succeeded})"
    );
}
