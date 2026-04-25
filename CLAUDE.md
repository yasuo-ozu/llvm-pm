# llvm-pm

Safe Rust wrapper for LLVM's new PassManager (PassBuilder-based, LLVM 10+).

## Architecture

```
src/lib.rs              Safe Rust wrapper (root crate, depends on inkwell)
src/plugin.rs           Plugin API (PassBuilder, PluginModulePassManager, etc.)
src/context.rs          BetterContext (Send+Sync context wrapper)
tests/integration.rs    Integration tests

llvm-pm-sys/            Low-level FFI crate (depends on llvm-sys)
  cpp/llvm_pm.h         C header (extern "C" API contract)
  cpp/llvm_pm.cpp       C++ stubs wrapping LLVM PassBuilder, analysis managers, etc.
  build.rs              Finds LLVM (llvm-config / LLVM_DIR), compiles C++ via cc, runs bindgen
  src/lib.rs            Re-exports llvm-sys, includes bindgen bindings

llvm-pm-macros/         Proc-macro crate (#[plugin] attribute)
  src/lib.rs            Generates llvmGetPassPluginInfo entry point

```

### Design: Bundled opaque handle

A single `LlvmPmPassManagerRef` owns all PassManager infrastructure:
- `PassInstrumentationCallbacks` + `StandardInstrumentations`
- 4 analysis managers (Loop, Function, CGSCC, Module)
- `PassBuilder` (with analyses registered and cross-registered)
- `ModulePassManager` or `FunctionPassManager`

Destruction order is explicit in the C++ destructor (reverse of construction).

### LLVM version compatibility

The C++ PassManager wrapper remains compatible with LLVM 10/11/12/13/14/15/16/17/18/19/20/21/22. `llvm-plugin` integration in `llvm-pm` is supported for `llvm10-0` .. `llvm18-0`.

### Dependency chain

```
llvm-pm (root crate)
  ├── llvm-plugin (0.6, optional)
  │     └── inkwell (0.5)
  │           └── llvm-sys 180
  ├── inkwell (0.5 or 0.9)
  │     └── llvm-sys (feature-selected)
  ├── llvm-pm-sys
  │     └── llvm-sys 100/110/120/130/140/150/160/170/180/191/201/211/221 (feature-selected)
  └── llvm-pm-macros (optional, proc-macro, no LLVM dep)
```

- `llvm-sys` handles LLVM library linking. Our `build.rs` only compiles C++ stubs and links C++ stdlib.
- Bindgen blocklists LLVM types (`LLVM.*`) — they come from llvm-sys instead, ensuring type compatibility with inkwell.
- Feature flags: `llvm10-0` .. `llvm18-0` (inkwell 0.5), and `llvm19-1` .. `llvm22-1` (inkwell 0.9) select LLVM versions.
- MSRV: 1.65 (bindgen 0.69)

## Build & Test

All commands use the nix dev shell:

```bash
nix develop . -c cargo build --workspace
nix develop . -c cargo test --workspace
nix develop . -c cargo clippy --workspace -- -D warnings
nix develop . -c cargo fmt --all -- --check
```

To build for a specific LLVM version:
```bash
nix develop . -c cargo build --workspace --no-default-features --features llvm19-1
```

### Environment variables

| Variable | Purpose | Set by |
|---|---|---|
| `LLVM_CONFIG` | Path to `llvm-config` binary | flake.nix (nix), CI |
| `LLVM_DIR` | LLVM install prefix (MSVC fallback) | CI (Windows) |
| `LIBCLANG_PATH` | Path to libclang.so for bindgen | flake.nix, CI |
| `BINDGEN_EXTRA_CLANG_ARGS` | System include paths for bindgen | flake.nix (nix) |

## Implementation Strategy

### Layer 1: C++ Stubs (`llvm-pm-sys/cpp/`)

Expose LLVM's C++ PassBuilder API through `extern "C"` functions. Key functions:
- `llvm_pm_create_with_opt_level()` — build default pipeline at O0-Oz
- `llvm_pm_create_with_pipeline()` — parse textual pipeline string
- `llvm_pm_create_function_with_pipeline()` — parse function-level pipeline
- `llvm_pm_create_lto()` / `llvm_pm_create_lto_pre_link()` / `llvm_pm_create_thin_lto_pre_link()` — LTO pipelines
- `llvm_pm_run()` / `llvm_pm_run_on_function()` — run passes on module or function
- `llvm_pm_options_*()` — configure debug logging, verify each, extension points
- `llvm_pm_add_module_pass()` / `llvm_pm_add_function_pass()` — add custom Rust callback passes
- `llvm_pm_create_empty_module()` / `llvm_pm_create_empty_function()` — empty PMs for custom-only pipelines
- `llvm_pm_dispose()` / `llvm_pm_dispose_message()` — cleanup
- `llvm_pm_plugin_api_version()` — return `LLVM_PLUGIN_API_VERSION`
- `llvm_pm_pb_add_*()` — register callbacks on a raw PassBuilder* (pipeline parsing, analysis registration, extension points)
- `llvm_pm_raw_mpm_add_module_pass()` / `llvm_pm_raw_fpm_add_function_pass()` — add owned passes to raw PMs (used by plugin callbacks)

LLVM-C opaque types (`LLVMModuleRef`, etc.) are unwrapped via `reinterpret_cast` to avoid depending on LLVM's internal wrap/unwrap headers.

### Layer 2: Bindgen FFI (`llvm-pm-sys/`)

`build.rs` uses:
- `cc` crate to compile `llvm_pm.cpp` with LLVM cxxflags (`-fno-rtti -fno-exceptions`)
- `bindgen` with `allowlist_function("llvm_pm_.*")` and `blocklist_type("LLVM.*")` to generate only our bindings
- `llvm-config` for include paths and cxxflags (LLVM linking handled by `llvm-sys`)
- Re-exports `llvm-sys` as `llvm_pm_sys::llvm_sys` and LLVM types from it

### Layer 3: Safe Rust API (root crate)

Main types:
- `ModulePassManager<'a>` — wraps opaque C handle for module-level passes
  - `with_opt_level()`, `with_pipeline()`, `with_lto()`, `with_lto_pre_link()`, `with_thin_lto_pre_link()`
  - `new()` — create empty PM for custom-pass-only pipelines
  - `add_pass(pass: T)` — add a custom `ModulePass` implementation (takes ownership)
  - `run(&mut self, &Module)` — execute passes (safe)
- `FunctionPassManager<'a>` — wraps opaque C handle for function-level passes
  - `with_pipeline()`, `new()`
  - `add_pass(pass: T)` — add a custom `FunctionPass` implementation (takes ownership)
  - `run(&mut self, FunctionValue)` — execute passes (safe)
- `Options` — builder for debug logging, verify each, and extension point pipelines
- `BetterContext` — thread-safe (`Send + Sync`) wrapper around inkwell `Context`, requires `&mut self` for LLVM operations
- `PreservedAnalyses` — return type for custom passes (`All` or `None`)
- `ModulePass` / `FunctionPass` — traits for custom pass definitions
- Traits for custom passes:
  - `LlvmModulePass`, `LlvmFunctionPass`
  - `LlvmCgsccPass`, `LlvmLoopPass`
- `OptLevel`, `Error` — supporting types

Re-exports: `inkwell`, `LLVMModuleRef`, `LLVMValueRef`.

Pass manager constructors and `run()` are safe (take inkwell types). Use `inkwell::targets::TargetMachine` directly.
`ModulePassManager` and `FunctionPassManager` implement `Send`.

### Layer 4: Plugin API (`src/plugin.rs` + `llvm-pm-macros/`)

For building LLVM plugins (`cdylib` crates loaded by `opt`/`clang`):
- `#[llvm_pm::plugin(name = "...", version = "...")]` — attribute macro generating `llvmGetPassPluginInfo` entry point
- `PassBuilder` — wraps raw `PassBuilder*` from LLVM, provides callback registration
  - `add_module_pipeline_parsing_callback()`, `add_function_pipeline_parsing_callback()`
  - `add_module_analysis_registration_callback()`, `add_function_analysis_registration_callback()`
  - Extension point callbacks (peephole, optimizer_last, pipeline_start, etc.)
- `PluginModulePassManager` — borrowed `ModulePassManager*`, `add_pass()` transfers ownership to C++
- `PluginFunctionPassManager` — borrowed `FunctionPassManager*`, `add_pass()` transfers ownership to C++
- `PipelineParsing` — `Parsed` / `NotParsed` enum for pipeline callbacks
- `PassPluginLibraryInfo` — struct returned by `llvmGetPassPluginInfo`

C++ side uses `shared_ptr<void>` for callback data and `RustOwned{Module,Function,CGSCC,Loop}Pass` for pass data ownership.

### Platform support

| Platform | LLVM discovery | ABI | C++ stdlib |
|---|---|---|---|
| Linux (GCC/Clang) | `llvm-config` | Itanium | libstdc++ |
| macOS | `llvm-config` | Itanium | libc++ |
| Windows (MSVC) | `LLVM_DIR` env → `llvm-config.exe` | MSVC | msvcrt |

`extern "C"` functions bypass C++ ABI differences entirely.

## CI Matrix

GitHub Actions (`.github/workflows/ci.yml`) using `.github/actions/install-llvm` composite action:
- **Linux**: LLVM 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22 via apt.llvm.org (with matching `--features llvmX-Y`)
- **macOS**: LLVM 18, 19, 20 via Homebrew (ARM `macos-latest` + Intel `macos-13`)
- **Windows/MSVC**: LLVM 18, 19, 20 via official installer
- **Lint**: rustfmt + clippy (LLVM 18, Linux only)
- **Coverage**: cargo-tarpaulin (LLVM 18, Linux only), uploaded to Codecov

## TODOs

- [x] Add `FunctionPassManager` support (create, parse pipeline, run on individual functions)
- [x] Expose `StandardInstrumentations` options (DebugLogging, VerifyEach) as builder options
- [x] Add `TargetMachine` creation helpers
- [x] Add `Send` safety analysis and implementation
- [x] Support extension point callbacks (PeepholeEP, OptimizerEarlyEP, etc.)
- [x] Add LTO pipeline support (`buildLTODefaultPipeline`, `buildThinLTOPreLinkDefaultPipeline`, etc.)
- [x] Add macOS CI job
- [x] Smarter `LLVM_CONFIG` auto-detection (tries `llvm-config-20`, `-19`, `-18` before plain `llvm-config`)
- [x] Add inkwell compatibility (re-export, convenience methods, tests using inkwell)
- [x] Add llvm-sys as dependency (remove duplicate LLVM type bindings)
- [x] Move llvm-pm/ to project root
- [x] Do not use `XXX.workspace` in Cargo.toml
- [x] Survey MSRV bottlenecks, set feasible MSRV (1.70, limited by bindgen 0.71)
- [x] Use install-llvm action on CI (matrix build for test)
- [x] Add macOS Intel (macos-13) target for test
- [x] Setup test coverage (cargo-tarpaulin + Codecov)
- [x] In `create_test_module()` in integration.rs, do not return Context
- [x] Implement `struct BetterContext` (Send+Sync wrapper, requires &mut for LLVM APIs)
- [x] Write safety comments on all unsafe blocks
- [x] reduce MSRV more relaxing dependencies
- [x] Change the argument type of `{Module,Function}PassManager::new()` from LlvmTargetMachine to &inkwell::targets::TargetMachine. Hold the lifetime `'ctx` of inkwell::Context during the module pass lifetime. make `new()` safe function. also do the same thing for `with_*()` s
- [x] In `*PassManager::run()`, change arg type from `LLVM*Ref` to `inkwell::*<'ctx>>`. make `run()` safe. remove `*PassManager::run_on_*()`.
- [x] In `add_pass()`, change the argument from mut ref to owned. make `add_pass()` safe
- [x] Change the arg type of createInfrastructure(). do not take pointer for `opts`
- [x] remove `Options::as_raw()` and `opts_to_raw()`
- [x] remove `llvm_pm_initialize_all_targets` and `initialize_all_targets()` because they can be covered by inkwell
- [x] remove `struct TargetMachine`, use `inkwell::targets::TargetMachine` instead
- [x] remove unused symbols or codes from cpp stub
- [x] support CGSCC pass, loop pass and analysis pass
- [x] Define `Llvm{Module,Function}Pass` trait in llvm-pm side, and auto-impl it on `llvm_plugin::Llvm{Module,Function}Pass`
- [x] Finish CgsccAnalysisManager, LoopAnalysisManager. impl `add_pass` interface for them
- [x] add feature gating to enable / disable llvm-plugin deps
- [x] setup coverage
- [x] remove llvm-pm-macros and attribute macro feature
- [x] support llvm-plugin with other llvm versions
- [x] implement `ToLlvm{Module,..}{Pass,...}` traits to virtualize adapter conversion. use them as `impl Trait` type in arguments of `run_pass()` / `run_analysis()`. Write identity impl. Auto impl for corresponding traits in llvm-plugin, sliding from existing auto impl of `LlvmXxxYyy`
- [x] use `llvm19-1` `llvm20-1` feature names
- [x] relax inkwell deps versions in Cargo.toml. can we use inkwell `0.5-0.8` or `0.10-` with this crate?
- [x] write safety comments where we are using unsafe blocks
- [x] add feature flags to support all llvm versions  that is supported by llvm-sys crate
- [x] finish TODO comments
- [x] Survey our safe harness which using unsafe interfaces. is there any concern about the safety? add explaining comment
- [x] Add test with multithreading, using multiple Context per thread.
- [x] improve coverage
- [x] implement a realistic example, that defines each pass and analysis, and use analyse pass result in each pass. refer existing LLVM pass to represent in Rust.
- [x] provide `#[plugin]` attribute macro, which plays the same role as `llvm-plugin` crate does (but support LLVM newer than 18). also provide original `PassBuilder`.
