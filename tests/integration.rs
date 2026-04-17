use llvm_pm::inkwell;
use llvm_pm::inkwell::context::Context;
use llvm_pm::inkwell::targets::{
    CodeModel, InitializationConfig, RelocMode, Target, TargetMachine,
};
use llvm_pm::inkwell::IntPredicate;
use llvm_pm::{
    CgsccAnalysisManager, FunctionPassManager, LlvmCgsccAnalysis, LlvmFunctionAnalysis,
    LlvmLoopAnalysis, LlvmModuleAnalysis, LoopAnalysisManager, ModulePassManager, OptLevel,
    Options, PreservedAnalyses,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;

/// Helper: create a module with a simple `void @test_fn()` function.
fn create_test_module() -> inkwell::module::Module<'static> {
    // SAFETY: We leak the context so the module can have 'static lifetime.
    // Tests clean up via LLVM process exit.
    let context = Box::leak(Box::new(Context::create()));
    let module = context.create_module("test_module");

    let void_ty = context.void_type();
    let fn_ty = void_ty.fn_type(&[], false);
    let func = module.add_function("test_fn", fn_ty, None);
    let bb = context.append_basic_block(func, "entry");
    let builder = context.create_builder();
    builder.position_at_end(bb);
    builder.build_return(None).unwrap();

    module
}

/// Helper: create a module with `i32 @add(i32, i32)` function.
fn create_add_module() -> (
    inkwell::module::Module<'static>,
    inkwell::values::FunctionValue<'static>,
) {
    let context = Box::leak(Box::new(Context::create()));
    let module = context.create_module("test_add");

    let i32_ty = context.i32_type();
    let fn_ty = i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false);
    let func = module.add_function("add", fn_ty, None);
    let bb = context.append_basic_block(func, "entry");
    let builder = context.create_builder();
    builder.position_at_end(bb);

    let a = func.get_nth_param(0).unwrap().into_int_value();
    let b_param = func.get_nth_param(1).unwrap().into_int_value();
    let sum = builder.build_int_add(a, b_param, "sum").unwrap();
    builder.build_return(Some(&sum)).unwrap();

    (module, func)
}

/// Helper: create a module with a simple counted loop in `i32 @loop_fn()`.
fn create_loop_module() -> (
    inkwell::module::Module<'static>,
    inkwell::values::FunctionValue<'static>,
) {
    let context = Box::leak(Box::new(Context::create()));
    let module = context.create_module("test_loop");

    let i32_ty = context.i32_type();
    let fn_ty = i32_ty.fn_type(&[], false);
    let func = module.add_function("loop_fn", fn_ty, None);

    let entry = context.append_basic_block(func, "entry");
    let loop_bb = context.append_basic_block(func, "loop");
    let exit = context.append_basic_block(func, "exit");

    let builder = context.create_builder();
    builder.position_at_end(entry);
    builder.build_unconditional_branch(loop_bb).unwrap();

    builder.position_at_end(loop_bb);
    let phi = builder.build_phi(i32_ty, "i").unwrap();
    phi.add_incoming(&[(&i32_ty.const_zero(), entry)]);
    let i = phi.as_basic_value().into_int_value();
    let next = builder
        .build_int_add(i, i32_ty.const_int(1, false), "next")
        .unwrap();
    phi.add_incoming(&[(&next, loop_bb)]);
    let cond = builder
        .build_int_compare(IntPredicate::ULT, next, i32_ty.const_int(4, false), "cond")
        .unwrap();
    builder
        .build_conditional_branch(cond, loop_bb, exit)
        .unwrap();

    builder.position_at_end(exit);
    builder.build_return(Some(&i32_ty.const_zero())).unwrap();

    (module, func)
}

// =========================================================================
// Module pass manager tests
// =========================================================================

#[test]
fn test_opt_level_o2() {
    let module = create_test_module();
    let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O2, None)
        .expect("Failed to create pass manager");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_opt_level_o2_with_add() {
    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O2, None)
        .expect("Failed to create pass manager");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_pipeline_string() {
    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_pipeline(None, "instcombine,dce", None)
        .expect("Failed to create PM with pipeline");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_default_pipeline_string() {
    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_pipeline(None, "default<O2>", None)
        .expect("Failed to create PM with default<O2>");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_invalid_pipeline() {
    let result = ModulePassManager::with_pipeline(None, "this-is-not-a-real-pass", None);
    assert!(result.is_err(), "Expected error for invalid pipeline");
    let err = result.unwrap_err();
    assert!(
        !err.message().is_empty(),
        "Error message should not be empty"
    );
}

#[test]
fn test_all_opt_levels() {
    let levels = [
        OptLevel::O0,
        OptLevel::O1,
        OptLevel::O2,
        OptLevel::O3,
        OptLevel::Os,
        OptLevel::Oz,
    ];

    for level in levels {
        let module = create_test_module();
        let mut pm = ModulePassManager::with_opt_level(None, level, None)
            .unwrap_or_else(|e| panic!("Failed to create PM for {:?}: {}", level, e));
        pm.run(&module)
            .unwrap_or_else(|e| panic!("Failed to run passes for {:?}: {}", level, e));
    }
}

// =========================================================================
// Options tests
// =========================================================================

#[test]
fn test_options_verify_each() {
    let (module, _func) = create_add_module();
    let mut opts = Options::new();
    opts.verify_each(true);
    let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O1, Some(&opts))
        .expect("Failed to create PM with verify_each");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_options_extension_point() {
    let (module, _func) = create_add_module();
    let mut opts = Options::new();
    opts.add_peephole_ep("dce");
    let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O2, Some(&opts))
        .expect("Failed to create PM with extension point");
    pm.run(&module).expect("Failed to run passes");
}

#[test]
fn test_options_multiple_extension_points() {
    let (module, _func) = create_add_module();
    let mut opts = Options::new();
    opts.add_peephole_ep("dce")
        .add_scalar_optimizer_late_ep("dce");
    let mut pm = ModulePassManager::with_opt_level(None, OptLevel::O2, Some(&opts))
        .expect("Failed to create PM with multiple EPs");
    pm.run(&module).expect("Failed to run passes");
}

// =========================================================================
// FunctionPassManager tests
// =========================================================================

#[test]
fn test_function_pass_manager() {
    let (_module, func) = create_add_module();
    let mut fpm = FunctionPassManager::with_pipeline(None, "instcombine,dce", None)
        .expect("Failed to create FPM");
    fpm.run(func).expect("Failed to run function passes");
}

#[test]
fn test_function_pass_manager_invalid_pipeline() {
    let result = FunctionPassManager::with_pipeline(None, "not-a-real-function-pass", None);
    assert!(
        result.is_err(),
        "Expected error for invalid function pipeline"
    );
}

// =========================================================================
// LTO pipeline tests
// =========================================================================

#[test]
fn test_lto_pipeline() {
    let (module, _func) = create_add_module();
    let mut pm =
        ModulePassManager::with_lto(None, OptLevel::O2, None).expect("Failed to create LTO PM");
    pm.run(&module).expect("Failed to run LTO passes");
}

#[test]
fn test_lto_pre_link_pipeline() {
    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_lto_pre_link(None, OptLevel::O2, None)
        .expect("Failed to create LTO pre-link PM");
    pm.run(&module).expect("Failed to run LTO pre-link passes");
}

#[test]
fn test_thin_lto_pre_link_pipeline() {
    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_thin_lto_pre_link(None, OptLevel::O2, None)
        .expect("Failed to create ThinLTO pre-link PM");
    pm.run(&module)
        .expect("Failed to run ThinLTO pre-link passes");
}

// =========================================================================
// TargetMachine integration tests
// =========================================================================

#[test]
fn test_opt_with_target_machine() {
    Target::initialize_all(&InitializationConfig::default());

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).expect("Failed to get target from triple");
    let tm = target
        .create_target_machine(
            &triple,
            "",
            "",
            inkwell::OptimizationLevel::Default,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect("Failed to create target machine");

    let (module, _func) = create_add_module();
    let mut pm = ModulePassManager::with_opt_level(Some(&tm), OptLevel::O2, None)
        .expect("Failed to create PM with target machine");
    pm.run(&module).expect("Failed to run passes");
}

// =========================================================================
// Send safety test
// =========================================================================

#[test]
fn test_module_pm_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<ModulePassManager<'static>>();
}

#[test]
fn test_function_pm_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<FunctionPassManager<'static>>();
}

// =========================================================================
// Custom pass tests (struct-based)
// =========================================================================

/// A custom module pass that counts functions in the module.
struct FunctionCounter {
    count: Arc<AtomicU32>,
}

impl llvm_pm::LlvmModulePass for FunctionCounter {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &llvm_pm::ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        self.count
            .fetch_add(module.get_functions().count() as u32, Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

#[test]
fn test_custom_module_pass_struct() {
    let (module, _func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let counter = FunctionCounter {
        count: count.clone(),
    };
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass(counter);
    pm.run(&module).expect("Failed to run custom pass");

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "Should have counted 1 function"
    );
}

// =========================================================================
// Custom pass tests (function-based with macro)
// =========================================================================

static MODULE_PASS_RAN: AtomicU32 = AtomicU32::new(0);

struct SimpleModulePass;

impl llvm_pm::LlvmModulePass for SimpleModulePass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &llvm_pm::ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = module;
        MODULE_PASS_RAN.fetch_add(1, Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

#[test]
fn test_custom_module_pass_function() {
    MODULE_PASS_RAN.store(0, Ordering::SeqCst);
    let module = create_test_module();
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass(SimpleModulePass);
    pm.run(&module).expect("Failed to run custom pass");

    assert_eq!(MODULE_PASS_RAN.load(Ordering::SeqCst), 1);
}

// =========================================================================
// Custom function pass test
// =========================================================================

struct FnPassCounter {
    count: Arc<AtomicU32>,
}

impl llvm_pm::LlvmFunctionPass for FnPassCounter {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &llvm_pm::FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

#[test]
fn test_custom_function_pass() {
    let (_module, func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let counter = FnPassCounter {
        count: count.clone(),
    };
    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_pass(counter);
    fpm.run(func).expect("Failed to run custom function pass");

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "Function pass should have run once"
    );
}

// =========================================================================
// Custom CGSCC and loop pass tests
// =========================================================================

struct CgsccPassCounter {
    count: Arc<AtomicU32>,
}

impl llvm_pm::LlvmCgsccPass for CgsccPassCounter {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

#[test]
fn test_custom_cgscc_pass() {
    let (module, _func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_cgscc_pass(CgsccPassCounter {
        count: count.clone(),
    });
    pm.run(&module).expect("Failed to run custom CGSCC pass");
    assert!(
        count.load(Ordering::SeqCst) > 0,
        "CGSCC pass should have run"
    );
}

struct CgsccFnNameLenAnalysis;

impl LlvmCgsccAnalysis for CgsccFnNameLenAnalysis {
    type Result = usize;
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        function.get_name().to_string_lossy().len()
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct CgsccAnalysisUserPass {
    count: Arc<AtomicU32>,
    registered: Arc<AtomicU32>,
}

impl llvm_pm::LlvmCgsccPass for CgsccAnalysisUserPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        if self
            .registered
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(CgsccFnNameLenAnalysis);
        }
        let len = *manager.get_result::<CgsccFnNameLenAnalysis>(function);
        if len > 0 {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        PreservedAnalyses::All
    }
}

#[test]
fn test_cgscc_analysis_manager_add_pass() {
    let (module, _func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let registered = Arc::new(AtomicU32::new(0));
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_cgscc_pass(CgsccAnalysisUserPass {
        count: count.clone(),
        registered: registered.clone(),
    });
    pm.run(&module)
        .expect("Failed to run cgscc analysis user pass");
    assert!(count.load(Ordering::SeqCst) > 0);
}

struct LoopPassCounter {
    count: Arc<AtomicU32>,
}

impl llvm_pm::LlvmLoopPass for LoopPassCounter {
    fn run_pass(
        &self,
        loop_header: llvm_pm::LLVMBasicBlockRef,
        _manager: &LoopAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = loop_header;
        self.count.fetch_add(1, Ordering::SeqCst);
        PreservedAnalyses::All
    }
}

#[test]
fn test_custom_loop_pass() {
    let (_module, func) = create_loop_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_loop_pass(LoopPassCounter {
        count: count.clone(),
    });
    fpm.run(func).expect("Failed to run custom loop pass");
    assert!(
        count.load(Ordering::SeqCst) > 0,
        "Loop pass should have run"
    );
}

struct LoopHeaderNonNullAnalysis;

impl LlvmLoopAnalysis for LoopHeaderNonNullAnalysis {
    type Result = bool;
    fn run_analysis(
        &self,
        loop_header: llvm_pm::LLVMBasicBlockRef,
        _manager: &LoopAnalysisManager,
    ) -> Self::Result {
        !loop_header.is_null()
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct LoopAnalysisUserPass {
    count: Arc<AtomicU32>,
    registered: Arc<AtomicU32>,
}

impl llvm_pm::LlvmLoopPass for LoopAnalysisUserPass {
    fn run_pass(
        &self,
        loop_header: llvm_pm::LLVMBasicBlockRef,
        manager: &LoopAnalysisManager,
    ) -> PreservedAnalyses {
        if self
            .registered
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(LoopHeaderNonNullAnalysis);
        }
        if *manager.get_result::<LoopHeaderNonNullAnalysis>(loop_header) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        PreservedAnalyses::All
    }
}

#[test]
fn test_loop_analysis_manager_add_pass() {
    let (_module, func) = create_loop_module();
    let count = Arc::new(AtomicU32::new(0));
    let registered = Arc::new(AtomicU32::new(0));
    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_loop_pass(LoopAnalysisUserPass {
        count: count.clone(),
        registered: registered.clone(),
    });
    fpm.run(func)
        .expect("Failed to run loop analysis user pass");
    assert!(count.load(Ordering::SeqCst) > 0);
}

// =========================================================================
// Analysis pass tests
// =========================================================================

struct ModuleAnalysisCounter {
    count: Arc<AtomicU32>,
}

impl LlvmModuleAnalysis for ModuleAnalysisCounter {
    type Result = ();
    fn run_analysis(
        &self,
        module: &inkwell::module::Module<'_>,
        _manager: &llvm_pm::ModuleAnalysisManager,
    ) -> Self::Result {
        let _ = module;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[test]
fn test_module_analysis_pass() {
    let module = create_test_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass((ModuleAnalysisCounter {
        count: count.clone(),
    }).into_pass());
    pm.run(&module).expect("Failed to run module analysis pass");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

struct FunctionAnalysisCounter {
    count: Arc<AtomicU32>,
}

impl LlvmFunctionAnalysis for FunctionAnalysisCounter {
    type Result = ();
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &llvm_pm::FunctionAnalysisManager,
    ) -> Self::Result {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[test]
fn test_function_analysis_pass() {
    let (_module, func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_pass((FunctionAnalysisCounter {
        count: count.clone(),
    }).into_pass());
    fpm.run(func).expect("Failed to run function analysis pass");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

struct CgsccAnalysisCounter {
    count: Arc<AtomicU32>,
}

impl LlvmCgsccAnalysis for CgsccAnalysisCounter {
    type Result = ();
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[test]
fn test_cgscc_analysis_pass() {
    let (module, _func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_cgscc_pass((CgsccAnalysisCounter {
        count: count.clone(),
    }).into_pass());
    pm.run(&module).expect("Failed to run CGSCC analysis pass");
    assert!(count.load(Ordering::SeqCst) > 0);
}

struct LoopAnalysisCounter {
    count: Arc<AtomicU32>,
}

impl LlvmLoopAnalysis for LoopAnalysisCounter {
    type Result = ();
    fn run_analysis(
        &self,
        loop_header: llvm_pm::LLVMBasicBlockRef,
        _manager: &LoopAnalysisManager,
    ) -> Self::Result {
        let _ = loop_header;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_pm::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[test]
fn test_loop_analysis_pass() {
    let (_module, func) = create_loop_module();
    let count = Arc::new(AtomicU32::new(0));
    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_loop_pass((LoopAnalysisCounter {
        count: count.clone(),
    }).into_pass());
    fpm.run(func).expect("Failed to run loop analysis pass");
    assert!(count.load(Ordering::SeqCst) > 0);
}

// =========================================================================
// Custom pass combined with standard pipeline
// =========================================================================

#[test]
fn test_custom_pass_after_standard_pipeline() {
    let (module, _func) = create_add_module();
    let count = Arc::new(AtomicU32::new(0));
    let counter = FunctionCounter {
        count: count.clone(),
    };
    let mut pm =
        ModulePassManager::with_opt_level(None, OptLevel::O2, None).expect("Failed to create PM");
    pm.add_pass(counter);
    pm.run(&module).expect("Failed to run passes");

    // After O2, the add function may or may not survive, but the pass should run.
    assert!(
        count.load(Ordering::SeqCst) > 0,
        "Custom pass should have run"
    );
}

// =========================================================================
// Multiple custom passes
// =========================================================================

#[test]
fn test_multiple_custom_passes() {
    let (module, _func) = create_add_module();
    let count1 = Arc::new(AtomicU32::new(0));
    let count2 = Arc::new(AtomicU32::new(0));
    let counter1 = FunctionCounter {
        count: count1.clone(),
    };
    let counter2 = FunctionCounter {
        count: count2.clone(),
    };
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass(counter1);
    pm.add_pass(counter2);
    pm.run(&module).expect("Failed to run passes");

    assert_eq!(count1.load(Ordering::SeqCst), 1);
    assert_eq!(count2.load(Ordering::SeqCst), 1);
}

// =========================================================================
// PreservedAnalyses::None test
// =========================================================================

static NONE_PASS_RAN: AtomicU32 = AtomicU32::new(0);

struct PassReturnsNone;

impl llvm_pm::LlvmModulePass for PassReturnsNone {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &llvm_pm::ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = module;
        NONE_PASS_RAN.fetch_add(1, Ordering::SeqCst);
        PreservedAnalyses::None
    }
}

#[test]
fn test_preserved_analyses_none() {
    NONE_PASS_RAN.store(0, Ordering::SeqCst);
    let module = create_test_module();
    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass(PassReturnsNone);
    pm.run(&module).expect("Failed to run pass");

    assert_eq!(NONE_PASS_RAN.load(Ordering::SeqCst), 1);
}

// =========================================================================
// llvm-plugin trait bridge tests
// =========================================================================

#[cfg(feature = "llvm-plugin-crate")]
struct PluginModulePass {
    count: Arc<AtomicU32>,
}

#[cfg(feature = "llvm-plugin-crate")]
impl llvm_plugin::LlvmModulePass for PluginModulePass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &llvm_plugin::ModuleAnalysisManager,
    ) -> llvm_plugin::PreservedAnalyses {
        let _ = module;
        self.count.fetch_add(1, Ordering::SeqCst);
        llvm_plugin::PreservedAnalyses::All
    }
}

#[cfg(feature = "llvm-plugin-crate")]
struct PluginFunctionPass {
    count: Arc<AtomicU32>,
}

#[cfg(feature = "llvm-plugin-crate")]
impl llvm_plugin::LlvmFunctionPass for PluginFunctionPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &llvm_plugin::FunctionAnalysisManager,
    ) -> llvm_plugin::PreservedAnalyses {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
        llvm_plugin::PreservedAnalyses::All
    }
}

#[cfg(feature = "llvm-plugin-crate")]
struct PluginModuleAnalysis {
    count: Arc<AtomicU32>,
}

#[cfg(feature = "llvm-plugin-crate")]
impl llvm_plugin::LlvmModuleAnalysis for PluginModuleAnalysis {
    type Result = ();

    fn run_analysis(
        &self,
        module: &inkwell::module::Module<'_>,
        _manager: &llvm_plugin::ModuleAnalysisManager,
    ) -> Self::Result {
        let _ = module;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_plugin::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[cfg(feature = "llvm-plugin-crate")]
struct PluginFunctionAnalysis {
    count: Arc<AtomicU32>,
}

#[cfg(feature = "llvm-plugin-crate")]
impl llvm_plugin::LlvmFunctionAnalysis for PluginFunctionAnalysis {
    type Result = ();

    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &llvm_plugin::FunctionAnalysisManager,
    ) -> Self::Result {
        let _ = function;
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn id() -> llvm_plugin::AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

#[cfg(feature = "llvm-plugin-crate")]
#[test]
fn test_llvm_plugin_module_pass_and_analysis_bridge() {
    let module = create_test_module();
    let pass_count = Arc::new(AtomicU32::new(0));
    let analysis_count = Arc::new(AtomicU32::new(0));

    let mut pm = ModulePassManager::new(None, None).expect("Failed to create empty PM");
    pm.add_pass((PluginModuleAnalysis {
        count: analysis_count.clone(),
    }).into_pass());
    pm.add_pass(PluginModulePass {
        count: pass_count.clone(),
    });
    pm.run(&module)
        .expect("Failed to run llvm-plugin module bridge");

    assert_eq!(pass_count.load(Ordering::SeqCst), 1);
    assert_eq!(analysis_count.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "llvm-plugin-crate")]
#[test]
fn test_llvm_plugin_function_pass_and_analysis_bridge() {
    let (_module, func) = create_add_module();
    let pass_count = Arc::new(AtomicU32::new(0));
    let analysis_count = Arc::new(AtomicU32::new(0));

    let mut fpm = FunctionPassManager::new(None, None).expect("Failed to create empty FPM");
    fpm.add_pass((PluginFunctionAnalysis {
        count: analysis_count.clone(),
    }).into_pass());
    fpm.add_pass(PluginFunctionPass {
        count: pass_count.clone(),
    });
    fpm.run(func)
        .expect("Failed to run llvm-plugin function bridge");

    assert_eq!(pass_count.load(Ordering::SeqCst), 1);
    assert_eq!(analysis_count.load(Ordering::SeqCst), 1);
}
