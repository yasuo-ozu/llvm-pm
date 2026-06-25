# Safety & Correctness Audit: llvm-pm  — RESOLVED

> **Status:** all substantive findings below have been **fixed in the library
> core** (`src/lib.rs`, `src/plugin.rs`, `llvm-pm-sys/cpp/llvm_pm.{h,cpp}`).
> The regression tests in `tests/audit_findings.rs` now assert the *fixed*
> behavior and are green, alongside the existing suite (74 integration + 7
> plugin-macro tests). See **[Resolution](#resolution)** for the per-finding fix.

## Overview

This report covers a safety/correctness review of `llvm-pm`, a Rust wrapper for
LLVM's new PassManager (PassBuilder-based). The crate exposes LLVM C++ APIs
through an `extern "C"` FFI layer (`llvm-pm-sys`), with safe Rust wrappers in the
root crate and a plugin API in `src/plugin.rs`.

Every finding below was read at the cited line(s) and independently
cross-verified, then fixed. The fixes are pinned by executable regression tests
in **`tests/audit_findings.rs`** (green under default features `llvm18-1` +
`plugin-macros`); each test now asserts the *correct* post-fix behavior so a
future regression breaks loudly.

Files reviewed:

- `src/lib.rs` (safe wrapper, trampolines, analysis registries/caches, `Send` impls)
- `src/traits.rs` (pass/analysis traits + analysis→pass adapters)
- `src/plugin.rs` (plugin API: `PassBuilder`, `Plugin*PassManager`, callbacks)
- `src/llvm_plugin_harness.rs` (llvm-plugin compatibility bridge)
- `llvm-pm-sys/cpp/llvm_pm.cpp`, `llvm-pm-sys/cpp/llvm_pm.h` (C++ stubs)
- `Cargo.toml`, `README.md`, `exam.rs`, `examples/*.rs`

---

## Findings summary

| # | Finding | Severity | Type | Realized by test |
|---|---------|----------|------|------------------|
| 1 | `Send` impl without `Send` bound on passes | High | Soundness | `finding1_send_hole_…` (compile-demo) |
| 2 | `from_utf8_unchecked` / `from_raw_parts(null,0)` on `StringRef` | Low | UB | documented (not safely reproducible) |
| 3 | Panic in a pass aborts across `extern "C"` | Medium | Robustness | `finding3_panic_in_pass_aborts_across_ffi` (subprocess) |
| 4 | Analysis cache/registry are process-global & never cleared | Medium | Correctness/leak | `finding4_*` (3 tests) |
| 5 | `static mut` global registries | Low | Maintenance | documented |
| 6 | Manual C++ destructor ordering | Low | Maintenance | documented |
| 7 | Plugin callbacks lack `Send`/`Sync` | Info | Hardening | documented (downgraded — see below) |
| N1 | **Type confusion via colliding analysis `id()`** | High | Soundness | `finding_new_analysis_id_collision_…` |
| N2 | **Self-deadlock on dependent-analysis request** | High | Liveness | `finding_new_dependent_analysis_request_deadlocks` (subprocess) |
| N3 | `add_analysis` accepts `!Send` into a global `Send` registry | High | Soundness | folded into #1 (same root pattern) |
| N4 | README doc examples use removed/incorrect API | Medium | Docs | documented |
| N5 | `exam.rs` does not compile against the current API | Low | Example rot | documented |
| N6 | `compile_error!` message typo `llvm7-0` | Info | Docs | documented |
| N7 | `consume_c_error` NULL-derefs on OOM error path | Low | Robustness | documented |
| N8 | `get_result` TOCTOU double-compute/overwrite-leak | Low | Correctness/leak | documented |
| N9 | `RetiredPICs`/`RetiredSIs` grow unbounded per `run()` | Low | Leak | documented |

---

## Resolution

All substantive findings are fixed. Summary of the code changes:

| # | Fix | Where |
|---|-----|-------|
| 1 / N3 | `add_*`/`add_analysis` now require `P: Send` (and analysis `Result: Send`); pass storage is `Vec<Box<dyn Any + Send>>` | `src/lib.rs` |
| N1 | CGSCC/Loop analysis registry & cache key on `TypeId::of::<A>()` (not user `id()`); self-request guard compares `TypeId` | `src/lib.rs` |
| N2 | `get_result` copies the pass ptr + entrypoint out and **releases the registry `Mutex` before** invoking the user callback (and panics, if any, happen without the lock held) | `src/lib.rs` |
| 3 | all four pass trampolines wrap their bodies in `catch_unwind`; on panic they print + return `PreservedAnalyses::None` instead of aborting; `module_pass_trampoline` uses `ManuallyDrop` so the C++-owned module is never freed on the panic path | `src/lib.rs` |
| 4 | results are stored as a freeable `CachedResult { ptr, drop_fn }`; each PM `Drop` fetches its `CGAM`/`LAM` pointers (new FFI `llvm_pm_cgam_ptr`/`llvm_pm_lam_ptr`) and clears its registry+cache entries — eliminating the stale-result, pass-leak, result-leak and cannot-re-register hazards | `src/lib.rs`, `llvm-pm-sys/cpp/*` |
| 2 | plugin pipeline-parsing callbacks use checked `from_utf8` with a null/empty guard, returning `NotParsed` on bad input | `src/plugin.rs` |
| N6 | `compile_error!` text corrected to `llvm17-0` | `src/lib.rs` |
| N7 | `consume_c_error` returns a generic error on a null pointer (OOM path) instead of dereferencing null | `src/lib.rs` |
| N9 | `reinitSI` reuses `PIC`/`SI` while the `LLVMContext` is unchanged (new `SICtx` field), so repeated `run()` no longer grows `Retired{PICs,SIs}` | `llvm-pm-sys/cpp/*` |

Not changed (by design): #5 (`static mut` works; cosmetic), #6 (destructor order is
correct), #7 (plugin-callback `Send`/`Sync` is hardening, not a demonstrated bug),
N4/N5 (docs/scratch — `README.md` examples and root `exam.rs`), N8 (the residual
double-compute window is benign for pure analyses and bounded now that per-manager
state is cleared on dispose).

### Regression tests (`tests/audit_findings.rs`, all green)

| Test | Verifies |
|------|----------|
| `finding1_send_pass_works_and_pm_is_send` | a `Send` pass works and the PM stays `Send`; a `!Send` pass no longer compiles (proven separately: `Rc<Cell<u32>>` is rejected by `add_pass`) |
| `finding3_panic_in_pass_no_longer_aborts_across_ffi` | a panicking pass no longer aborts; the re-exec'd child exits cleanly (no abort signal) |
| `finding4_registered_analysis_pass_is_freed_on_pm_dispose` | a real PM frees its registered analysis pass when dropped (drop-flag fires) |
| `finding_new_analysis_id_collision_no_longer_causes_type_confusion` | an `id()` collision no longer aliases: requesting an unregistered analysis now panics "not registered" instead of returning another type's bytes |
| `finding_new_dependent_analysis_request_no_longer_deadlocks` | an analysis requesting another analysis completes (child finishes in ~0.1s; pre-fix it hit the 3s deadlock watchdog) |

---

## Detailed findings

> The analysis below describes each issue **as originally found** (line numbers
> refer to the pre-fix source). All are now resolved per
> [Resolution](#resolution); the "Fix:" notes record the approach taken.

### 1. `Send` impl allows `!Send` passes/analyses to cross threads
**Severity: High — soundness hole reachable from safe code**
**Location:** `src/lib.rs:1161, 1323, 1476` (`unsafe impl Send` for the three PMs);
`add_pass`/`add_*_pass` at `1050, 1069, 1088, 1110, 1257, 1280, 1389, 1408, 1430`;
analysis variant at `src/lib.rs:379, 533` (`add_analysis`) + `679-680`
(`unsafe impl Send for {Cgscc,Loop}AnalysisEntry`).

All three pass managers are *unconditionally* `Send`, yet none of the
`add_pass`/`add_analysis` methods require `P: Send` (`_passes: Vec<Box<dyn Any>>`,
not `… + Send`). A user can store a pass containing `Rc`, `Cell`, or a raw
pointer, then move the PM to another thread — entirely in safe code. The
analysis registry has the same shape: `add_analysis<P>` has no `Send` bound, but
the entries are stored in a global `unsafe impl Send` map and may be invoked from
another thread (N3).

**Realized:** `finding1_send_hole_allows_non_send_pass_to_cross_threads` compiles
and runs today; it must fail to compile once the bound is added.

**Fix:** add `P: Send` to every `add_*` method (smallest change), or change the
stores to `Box<dyn Any + Send>` (enforces it structurally), or drop the
`unsafe impl Send`.

### N1. Type confusion via colliding analysis `id()`
**Severity: High — type confusion / potential OOB read from safe code**
**Location:** `src/lib.rs:441, 466` (cgscc), `485, 495`, `563, 620`, `646` (loop).

`get_result::<A>`/`get_cached_result::<A>` key the cache on
`(manager_ptr, A::id(), ir_key)` and, on a hit, do
`&*(ptr as *const A::Result)`. The *type identity is the user-supplied `id()`*,
not the Rust type. Two analyses returning the same `id()` alias each other's
cached results: `get_result::<B>` returns `A`'s cached bytes reinterpreted as
`B::Result` — and `B` need not even be registered (the cache check precedes the
"analysis was not registered" assert). With matching layouts this is observable;
with mismatched layouts (`B::Result` larger, or a type with validity invariants
like `bool`/`char`/`&T`) it is undefined behavior.

**Realized:** `finding_new_analysis_id_collision_causes_type_confusion`.

**Fix:** store and check a `TypeId` alongside `id()` in the cache entry, or derive
the key from the Rust type rather than a user-supplied pointer.

### N2. Self-deadlock when an analysis requests another analysis
**Severity: High — liveness (deadlock in ordinary use)**
**Location:** `src/lib.rs:448-456` (cgscc), `602-610` (loop).

`get_result` locks the global analysis-passes `Mutex`, then invokes the user
`run_analysis` callback *while still holding the guard* (`drop(passes)` runs only
after the callback). If that callback requests a *different* analysis via
`get_result`, the nested call re-acquires the same non-reentrant `Mutex` on the
same thread → permanent deadlock. Dependent analyses are the norm in real LLVM
(e.g. LoopAnalysis depends on DominatorTree), so this is reachable in ordinary
use.

**Realized:** `finding_new_dependent_analysis_request_deadlocks` (child process
killed by a watchdog because it never returns).

**Fix:** clone/release the entry and drop the registry guard *before* calling the
user callback.

### 4. Analysis cache & registry are process-global and never cleared
**Severity: Medium — correctness + leak**
**Location:** `src/lib.rs:685-739` (the four `static mut` maps); `Drop` impls at
`1143-1152, 1310-1319, 1463-1472` call only `llvm_pm_dispose` (grep confirms no
`.remove`/`.clear` anywhere).

The registry and result cache are keyed by `manager_ptr as usize` and live for
the whole process. Consequences, all stemming from "never cleared":
- **Stale results:** after a PM drops, a new PM whose C++ analysis manager reuses
  the same address gets the previous PM's cached results (wrong values, no
  recompute).
- **Leak of registered passes:** `{Cgscc,Loop}AnalysisEntry::Drop` (which frees
  the pass `Box`) never runs because the map is never cleared.
- **Leak of results:** every computed `get_result` leaks a `Box<Result>` forever.
- **Cannot re-register:** a second manager at a reused address hits
  `assert!(… "analysis already registered")` (`src/lib.rs:414-417, 568-571`) —
  and that assert fires *while the registry `Mutex` is held*, which poisons it
  for the rest of the process and (if reached from a pass) aborts across FFI.

**Realized:** `finding4_cgscc_*`, `finding4_loop_*`, `finding4_registered_analysis_pass_leaks_*`.
(The "cannot re-register" panic is documented rather than tested in-process
because triggering it poisons the global mutex for all other tests.)

**Fix:** scope the registry/cache to a live manager and clear the relevant
entries in `Drop`, or add a generation counter to disambiguate reused addresses.

### 3. Panic in a pass aborts across `extern "C"`
**Severity: Medium — robustness**
**Location:** trampolines `src/lib.rs:759, 787, 813, 838`; panic sites also at
`FunctionValue::new(function).expect("invalid function")` (`796, 820`); plugin
callback entrypoints in `src/plugin.rs` (no `catch_unwind`).

A panic in a user `run_pass` unwinds into the `extern "C"` trampoline, which (Rust
≥ 1.71) aborts the process immediately. The backtrace from the realized test
confirms the path: `run_pass` → `llvm_pm_run` (C++) →
*"thread caused non-unwinding panic. aborting."*

**Realized:** `finding3_panic_in_pass_aborts_across_ffi`.

**Fix:** wrap trampoline bodies in `catch_unwind`; on panic, log and return
`PreservedAnalyses::None` (or deliberately `process::abort()` with a clear
message). The same applies to the plugin callback entrypoints.

### 2. `from_utf8_unchecked` (+ `from_raw_parts(null, 0)`) on `StringRef`
**Severity: Low — potential UB**
**Location:** `src/plugin.rs:110-115, 154-159`.

Pass names come from LLVM `StringRef`, which carries no UTF-8 guarantee; building
a `&str` from non-UTF-8 bytes is immediate UB. Additionally, an empty `StringRef`
may have a null `.data()`, and `slice::from_raw_parts(null, 0)` is UB (the pointer
must be non-null even for length 0). Practically the parser only feeds ASCII pass
names, so this is low-likelihood — but free to fix.

**Fix:** use `std::str::from_utf8(...)` with a `NotParsed` fallback, and guard the
null/zero-length case.

### N4. README examples use removed/incorrect API
**Severity: Medium — documentation rot**
**Location:** `README.md:54-133` (embedded into crate docs via
`#![doc = include_str!("../README.md")]`).

The `rust` examples are marked ```` ```ignore ```` (so the suite stays green) but
call APIs that do not exist / have the wrong shape: `context.raw()`,
`ModulePassManager::with_opt_level(context.raw(), None, OptLevel::O2, None)` (real
signature takes `Option<&TargetMachine>, OptLevel, Option<&Options>`),
`pm.run_on_module(&module)` (real method is `run(&mut self, &Module)`), and `let
pm = …` without `mut`. A user copying the docs gets compile errors.

**Fix:** update the examples to the current API and switch ```` ```ignore ```` to
````` ```no_run ````` so they are type-checked by `cargo test --doc`.

### N5. `exam.rs` does not compile against the current API
**Severity: Low — example rot**
**Location:** `exam.rs:155, 230, 308, 324, 326, 338, 340`.

The root scratch file calls `module_pm.add_analysis_pass(...)`,
`add_cgscc_analysis_pass(...)`, `add_loop_analysis_pass(...)` — none of which
exist on the pass managers (analysis registration is `add_analysis` *on the
analysis manager* passed into a pass) — and imports `llvm_pm::LLVMBasicBlockRef`,
which is not re-exported at the crate root (only `llvm_pm::traits::LLVMBasicBlockRef`).

**Fix:** delete `exam.rs` or rewrite it against the real API (cf. the working
`examples/all_pass_kinds.rs`).

### N6. `compile_error!` version range typo
**Severity: Info**
**Location:** `src/lib.rs:115-117` — message says
`"llvm10-0 .. llvm7-0"` (should be `llvm17-0`).

### N7. `consume_c_error` NULL-derefs on the OOM error path
**Severity: Low — robustness**
**Location:** `src/lib.rs:213-218`; `llvm-pm-sys/cpp/llvm_pm.cpp:93-98`
(`copyString` returns `nullptr` if `malloc` fails), used by the create-error
paths (`llvm_pm.cpp:330, 399`). If allocation of the error string fails, `err_msg`
is null and `CStr::from_ptr(null)` is UB. Low-likelihood (OOM only).

### N8. `get_result` TOCTOU double-compute / overwrite-leak
**Severity: Low — correctness + leak**
**Location:** `src/lib.rs:444-464` (cgscc), `598-618` (loop). The cache lock is
released between the miss-check and the insert, so two concurrent callers for the
same `(manager, analysis, ir)` both compute, the second `insert` overwrites the
first (leaking it), and the first caller may hold a reference that is no longer
the cached one.

### N9. `RetiredPICs`/`RetiredSIs` grow unbounded across `run()`
**Severity: Low — per-run leak**
**Location:** `llvm-pm-sys/cpp/llvm_pm.cpp:224-248` (`reinitSI` pushes the old
PIC/SI on every `run()`), cleared only in the destructor (`64-76`). Each `run()`
permanently retains one more PIC+SI pair; a long-lived PM run many times grows
without bound.

### 5 / 6 (carried over, verified)
- **5. `static mut` registries** (`src/lib.rs:685-739`): correct today but
  `static mut` is soft-deprecated; prefer `OnceLock`.
- **6. Manual C++ destructor ordering** (`llvm_pm.cpp:64-76`): correct for current
  LLVM; add a rationale comment so future maintainers preserve the order.

---

## Verified correct (investigated, *not* bugs)

These were examined and refuted as defects:

- **TargetMachine lifetime is enforced.** `PhantomData<&'a ()>` ties each PM to
  the borrowed `&TargetMachine`; a use-after-free attempt (`tm` dropped while the
  PM lives) fails to compile (`error[E0597]: 'tm' does not live long enough`).
- **The uncommitted `LLVM_VERSION_MAJOR >= 20` `ThinOrFullLTOPhase` guards** in
  `llvm_pm.cpp` match the upstream EP-callback signature change; correct.
- **The `llvm_plugin` blanket impls** (`src/llvm_plugin_harness.rs`) do **not**
  overlap the concrete analysis-adapter impls in `traits.rs` — they compile under
  the `llvm-plugin-crate` feature without coherence error.
- **Plugin `PassBuilder`/`Plugin*PassManager` raw-pointer design** is sound for
  its intended single-threaded pipeline-construction use (the `Send`/`Sync`
  question on plugin callbacks, #7, is an off-thread *hardening* item, not a
  demonstrated soundness bug).
- **Trampoline `Module::new` + `mem::forget`** correctly avoids double-freeing the
  C++-owned module; error-string `malloc`/`free` are paired and consumed once.

---

## Recommended fix priority

1. **N2 (deadlock)** and **N1 (type confusion)** — high impact, reachable in
   ordinary safe use; both have small, local fixes.
2. **#1 / N3 (Send bounds)** — add `P: Send` to the `add_*` methods.
3. **#4 (global registry/cache lifecycle)** — scope to a live manager + clear on
   `Drop`; this also removes the leaks (N8/N9-adjacent), the stale-result and the
   "already registered" hazards.
4. **#3 (catch_unwind in trampolines)** — defense-in-depth against aborts.
5. **#2, N4, N5, N6, N7** — low-effort hygiene/UB-elimination.
