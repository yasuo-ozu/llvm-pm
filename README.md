# llvm-pm [![Latest Version]][crates.io] [![Documentation]][docs.rs] [![GitHub Actions]][actions]

[Latest Version]: https://img.shields.io/crates/v/llvm-pm.svg
[crates.io]: https://crates.io/crates/llvm-pm
[Documentation]: https://img.shields.io/docsrs/llvm-pm
[docs.rs]: https://docs.rs/llvm-pm/latest/llvm_pm/
[GitHub Actions]: https://github.com/yasuo-ozu/llvm-pm/actions/workflows/ci.yml/badge.svg
[actions]: https://github.com/yasuo-ozu/llvm-pm/actions/workflows/ci.yml

Safe Rust wrapper for LLVM's new PassManager (LLVM 10+).

Built on top of [inkwell](https://github.com/TheDan64/inkwell) and [llvm-sys](https://crates.io/crates/llvm-sys), providing:

## Supported LLVM versions

| Feature flag | LLVM version |
|---|---|
| `llvm10-0` | 10.x |
| `llvm11-0` | 11.x |
| `llvm12-0` | 12.x |
| `llvm13-0` | 13.x |
| `llvm14-0` | 14.x |
| `llvm15-0` | 15.x |
| `llvm16-0` | 16.x |
| `llvm17-0` | 17.x |
| `llvm18-0` (default) | 18.x |
| `llvm19-1` | 19.x |
| `llvm20-1` | 20.x |
| `llvm21-1` | 21.x |
| `llvm22-1` | 22.x |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
llvm-pm = "0.1"
```

For a specific LLVM version:

```toml
[dependencies]
llvm-pm = { version = "0.1", default-features = false, features = ["llvm19-1"] }
```

### Running standard optimizations

```rust
use inkwell::context::Context;
use llvm_pm::{ModulePassManager, OptLevel};

let context = Context::create();
let module = context.create_module("my_module");
// ... build IR ...

unsafe {
    let pm = ModulePassManager::with_opt_level(
        context.raw(), None, OptLevel::O2, None,
    ).expect("Failed to create pass manager");
    pm.run_on_module(&module).expect("Pass execution failed");
}
```

### Textual pipeline

```rust
use llvm_pm::ModulePassManager;

unsafe {
    // Same syntax as `opt -passes=...`
    let pm = ModulePassManager::with_pipeline(
        context.raw(), None, "default<O2>", None,
    )?;
    pm.run_on_module(&module)?;
}
```

### Custom module pass

```rust
use llvm_pm::{LlvmModulePass, ModuleAnalysisManager, ModulePassManager, PreservedAnalyses};

struct MyPass {
    count: std::sync::atomic::AtomicU32,
}

impl LlvmModulePass for MyPass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = module;
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

// Use it:
let mut pm = ModulePassManager::new(None, None)?;
let pass = MyPass { count: std::sync::atomic::AtomicU32::new(0) };
pm.add_pass(pass);
pm.run(&module)?;
```

### Custom function pass

```rust
use llvm_pm::{FunctionAnalysisManager, FunctionPassManager, LlvmFunctionPass, PreservedAnalyses};

struct MyFnPass;

impl LlvmFunctionPass for MyFnPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = function;
        PreservedAnalyses::All
    }
}

let mut fpm = FunctionPassManager::new(None, None)?;
fpm.add_pass(MyFnPass);
fpm.run(func)?;
```


## License

Licensed under [MIT license](http://opensource.org/licenses/MIT)
