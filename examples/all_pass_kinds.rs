use llvm_pm::inkwell;
use llvm_pm::inkwell::values::AsValueRef;
use llvm_pm::inkwell::IntPredicate;
use llvm_pm::inkwell::OptimizationLevel;
use llvm_pm::traits::{
    LlvmCgsccAnalysis, LlvmCgsccPass, LlvmFunctionAnalysis, LlvmFunctionPass, LlvmLoopAnalysis,
    LlvmLoopPass, LlvmModuleAnalysis, LlvmModulePass, PreservedAnalyses,
};
use llvm_pm::{
    CgsccAnalysisManager, FunctionAnalysisManager, FunctionPassManager, LoopAnalysisManager,
    ModuleAnalysisManager, ModulePassManager, OptLevel,
};

type AnalysisKey = *const u8;
use llvm_pm_sys::llvm_sys::core::{
    LLVMConstInt, LLVMGetBasicBlockParent, LLVMGetGlobalParent, LLVMGetNamedGlobal,
    LLVMGlobalGetValueType, LLVMSetInitializer,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

const DELTA_GLOBAL: &str = "pass_delta";
const DELTA_MODULE: u64 = 3;
const DELTA_FUNCTION: u64 = 5;
const DELTA_CGSCC: u64 = 8;
const DELTA_LOOP: u64 = 13;
const DELTA_GLOBAL_CSTR: &[u8] = b"pass_delta\0";

fn set_pass_delta(module: &inkwell::module::Module<'_>, value: u64) {
    let i32_ty = module.get_context().i32_type();
    let g = module
        .get_global(DELTA_GLOBAL)
        .expect("global pass_delta must exist");
    g.set_initializer(&i32_ty.const_int(value, false));
}

fn get_pass_delta(module: &inkwell::module::Module<'_>) -> u64 {
    let g = module
        .get_global(DELTA_GLOBAL)
        .expect("global pass_delta must exist");
    let init = g
        .get_initializer()
        .expect("global pass_delta must have initializer")
        .into_int_value();
    init.get_zero_extended_constant()
        .expect("initializer should be integer constant")
}

unsafe fn set_pass_delta_from_function_ref(
    function_ref: llvm_pm::traits::LLVMValueRef,
    value: u64,
) {
    // SAFETY: function_ref is received from LLVM callbacks and points to a live function.
    // We only read its parent module/global and replace the constant initializer with
    // a same-typed integer constant.
    let module = LLVMGetGlobalParent(function_ref);
    assert!(!module.is_null(), "function must belong to a module");
    let global = LLVMGetNamedGlobal(module, DELTA_GLOBAL_CSTR.as_ptr().cast());
    assert!(
        !global.is_null(),
        "global pass_delta must exist in function module"
    );
    let ty = LLVMGlobalGetValueType(global);
    let v = LLVMConstInt(ty, value, 0);
    LLVMSetInitializer(global, v);
}

#[cfg(any(
    feature = "llvm10-0",
    feature = "llvm11-0",
    feature = "llvm12-0",
    feature = "llvm13-0",
    feature = "llvm14-0"
))]
fn build_load_i32<'ctx>(
    b: &inkwell::builder::Builder<'ctx>,
    _i32_ty: inkwell::types::IntType<'ctx>,
    ptr: inkwell::values::PointerValue<'ctx>,
    name: &str,
) -> inkwell::values::IntValue<'ctx> {
    b.build_load(ptr, name).unwrap().into_int_value()
}

#[cfg(any(
    feature = "llvm15-0",
    feature = "llvm16-0",
    feature = "llvm17-0",
    feature = "llvm18-0",
    feature = "llvm19-1",
    feature = "llvm20-1",
    feature = "llvm21-1",
    feature = "llvm22-1"
))]
fn build_load_i32<'ctx>(
    b: &inkwell::builder::Builder<'ctx>,
    i32_ty: inkwell::types::IntType<'ctx>,
    ptr: inkwell::values::PointerValue<'ctx>,
    name: &str,
) -> inkwell::values::IntValue<'ctx> {
    b.build_load(i32_ty, ptr, name).unwrap().into_int_value()
}

fn build_demo_module() -> inkwell::module::Module<'static> {
    // Leak context for a standalone executable example.
    let context = Box::leak(Box::new(inkwell::context::Context::create()));
    let module = context.create_module("demo");
    let i32_ty = context.i32_type();

    let delta = module.add_global(i32_ty, None, DELTA_GLOBAL);
    delta.set_initializer(&i32_ty.const_zero());

    // i32 @callee_cond(i32 %x)
    let callee_ty = i32_ty.fn_type(&[i32_ty.into()], false);
    let callee = module.add_function("callee_cond", callee_ty, None);
    let entry = context.append_basic_block(callee, "entry");
    let then_bb = context.append_basic_block(callee, "then");
    let else_bb = context.append_basic_block(callee, "else");
    let merge_bb = context.append_basic_block(callee, "merge");
    let b = context.create_builder();
    b.position_at_end(entry);
    let x = callee.get_first_param().unwrap().into_int_value();
    let x_plus_4 = b
        .build_int_add(x, i32_ty.const_int(4, false), "x_plus_4")
        .unwrap();
    let x_fold = b
        .build_int_sub(x_plus_4, i32_ty.const_int(4, false), "x_fold")
        .unwrap();
    let cmp = b
        .build_int_compare(
            IntPredicate::SGT,
            x_fold,
            i32_ty.const_int(10, false),
            "cmp",
        )
        .unwrap();
    b.build_conditional_branch(cmp, then_bb, else_bb).unwrap();

    b.position_at_end(then_bb);
    let t1 = b
        .build_int_mul(x_fold, i32_ty.const_int(2, false), "t1")
        .unwrap();
    let t2 = b
        .build_int_unsigned_div(t1, i32_ty.const_int(2, false), "t2")
        .unwrap();
    let dec = b
        .build_int_sub(t2, i32_ty.const_int(1, false), "dec")
        .unwrap();
    b.build_unconditional_branch(merge_bb).unwrap();

    b.position_at_end(else_bb);
    let e1 = b
        .build_xor(x_fold, i32_ty.const_int(0, false), "e1")
        .unwrap();
    let e2 = b
        .build_int_add(e1, i32_ty.const_int(1, false), "e2")
        .unwrap();
    let inc = b
        .build_int_add(e2, i32_ty.const_int(1, false), "inc")
        .unwrap();
    b.build_unconditional_branch(merge_bb).unwrap();

    b.position_at_end(merge_bb);
    let phi = b.build_phi(i32_ty, "r").unwrap();
    phi.add_incoming(&[(&dec, then_bb), (&inc, else_bb)]);
    let r = phi.as_basic_value().into_int_value();
    let keep = b
        .build_int_add(r, i32_ty.const_int(0, false), "keep")
        .unwrap();
    b.build_return(Some(&keep)).unwrap();

    // i32 @helper_loop(i32 %n)
    let helper_ty = i32_ty.fn_type(&[i32_ty.into()], false);
    let helper = module.add_function("helper_loop", helper_ty, None);
    let entry = context.append_basic_block(helper, "entry");
    let cond_bb = context.append_basic_block(helper, "cond");
    let body_bb = context.append_basic_block(helper, "body");
    let exit_bb = context.append_basic_block(helper, "exit");
    let b = context.create_builder();
    b.position_at_end(entry);
    let i_ptr = b.build_alloca(i32_ty, "i").unwrap();
    let acc_ptr = b.build_alloca(i32_ty, "acc").unwrap();
    let tmp_ptr = b.build_alloca(i32_ty, "tmp").unwrap();
    b.build_store(i_ptr, i32_ty.const_zero()).unwrap();
    b.build_store(acc_ptr, i32_ty.const_zero()).unwrap();
    b.build_store(tmp_ptr, i32_ty.const_int(7, false)).unwrap();
    b.build_unconditional_branch(cond_bb).unwrap();

    b.position_at_end(cond_bb);
    let i = build_load_i32(&b, i32_ty, i_ptr, "i");
    let n = helper.get_first_param().unwrap().into_int_value();
    let tmp = build_load_i32(&b, i32_ty, tmp_ptr, "tmp");
    let _noise_cmp = b
        .build_int_compare(IntPredicate::SGE, tmp, i32_ty.const_zero(), "noise_cmp")
        .unwrap();
    let keep_going = b
        .build_int_compare(IntPredicate::SLT, i, n, "keep_going")
        .unwrap();
    b.build_conditional_branch(keep_going, body_bb, exit_bb)
        .unwrap();

    b.position_at_end(body_bb);
    let acc = build_load_i32(&b, i32_ty, acc_ptr, "acc");
    let i_square = b.build_int_mul(i, i, "i_square").unwrap();
    let folded_i = b
        .build_int_unsigned_div(
            b.build_int_add(i_square, i, "plus_i").unwrap(),
            b.build_int_add(i, i32_ty.const_int(1, false), "i_plus_1")
                .unwrap(),
            "folded_i",
        )
        .unwrap();
    let acc_next = b.build_int_add(acc, folded_i, "acc_next").unwrap();
    let i_next = b
        .build_int_add(i, i32_ty.const_int(1, false), "i_next")
        .unwrap();
    b.build_store(acc_ptr, acc_next).unwrap();
    b.build_store(i_ptr, i_next).unwrap();
    b.build_unconditional_branch(cond_bb).unwrap();

    b.position_at_end(exit_bb);
    let acc_final = build_load_i32(&b, i32_ty, acc_ptr, "acc_final");
    let out = b
        .build_int_add(
            acc_final,
            i32_ty.const_int(0, false),
            "out",
        )
        .unwrap();
    b.build_return(Some(&out)).unwrap();

    // i32 @driver(i32 %a, i32 %b)
    // returns (callee_cond(a) + helper_loop(b) + pass_delta) or +1 for small values
    let driver_ty = i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false);
    let driver = module.add_function("driver", driver_ty, None);
    let entry = context.append_basic_block(driver, "entry");
    let big_bb = context.append_basic_block(driver, "big");
    let small_bb = context.append_basic_block(driver, "small");
    let b = context.create_builder();
    b.position_at_end(entry);
    let a = driver.get_nth_param(0).unwrap().into_int_value();
    let b_arg = driver.get_nth_param(1).unwrap().into_int_value();
    let c1 = {
        let value = b
            .build_call(callee, &[a.into()], "c1")
            .unwrap()
            .try_as_basic_value();
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
        {
            value.left().unwrap().into_int_value()
        }
        #[cfg(any(
            feature = "llvm19-1",
            feature = "llvm20-1",
            feature = "llvm21-1",
            feature = "llvm22-1"
        ))]
        {
            value.unwrap_basic().into_int_value()
        }
    };
    let c2 = {
        let value = b
            .build_call(helper, &[b_arg.into()], "c2")
            .unwrap()
            .try_as_basic_value();
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
        {
            value.left().unwrap().into_int_value()
        }
        #[cfg(any(
            feature = "llvm19-1",
            feature = "llvm20-1",
            feature = "llvm21-1",
            feature = "llvm22-1"
        ))]
        {
            value.unwrap_basic().into_int_value()
        }
    };
    let raw_sum = b.build_int_add(c1, c2, "raw_sum").unwrap();
    let delta_loaded = build_load_i32(&b, i32_ty, delta.as_pointer_value(), "delta");
    let adjusted = b.build_int_add(raw_sum, delta_loaded, "adjusted").unwrap();
    let is_big = b
        .build_int_compare(
            IntPredicate::SGT,
            adjusted,
            i32_ty.const_int(100, false),
            "is_big",
        )
        .unwrap();
    b.build_conditional_branch(is_big, big_bb, small_bb)
        .unwrap();

    b.position_at_end(big_bb);
    let big_t0 = b
        .build_int_mul(adjusted, i32_ty.const_int(2, false), "big_t0")
        .unwrap();
    let big_t1 = b
        .build_int_unsigned_div(big_t0, i32_ty.const_int(2, false), "big_t1")
        .unwrap();
    let big_out = b
        .build_int_add(big_t1, i32_ty.const_int(0, false), "big_out")
        .unwrap();
    b.build_return(Some(&big_out)).unwrap();

    b.position_at_end(small_bb);
    let plus_one = b
        .build_int_add(adjusted, i32_ty.const_int(1, false), "plus_one")
        .unwrap();
    let out = b
        .build_int_add(plus_one, i32_ty.const_int(0, false), "small_out")
        .unwrap();
    b.build_return(Some(&out)).unwrap();

    module
}

struct CgsccNameLenAnalysis;
impl LlvmCgsccAnalysis for CgsccNameLenAnalysis {
    type Result = usize;
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        function.get_name().to_string_lossy().len()
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct LoopHeaderNonNullAnalysis;
impl LlvmLoopAnalysis for LoopHeaderNonNullAnalysis {
    type Result = bool;
    fn run_analysis(
        &self,
        loop_header: llvm_pm::traits::LLVMBasicBlockRef,
        _manager: &LoopAnalysisManager,
    ) -> Self::Result {
        !loop_header.is_null()
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct ModuleStatsAnalysis;
impl LlvmModuleAnalysis for ModuleStatsAnalysis {
    type Result = (usize, usize);
    fn run_analysis(
        &self,
        module: &inkwell::module::Module<'_>,
        _manager: &ModuleAnalysisManager,
    ) -> Self::Result {
        let mut fn_count = 0usize;
        let mut bb_count = 0usize;
        for f in module.get_functions() {
            fn_count += 1;
            bb_count += f.count_basic_blocks() as usize;
        }
        (fn_count, bb_count)
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct FunctionStatsAnalysis;
impl LlvmFunctionAnalysis for FunctionStatsAnalysis {
    type Result = (u32, u32);
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &FunctionAnalysisManager,
    ) -> Self::Result {
        let bbs = function.count_basic_blocks();
        let mut insts = 0u32;
        for bb in function.get_basic_blocks() {
            insts += bb.get_instructions().count() as u32;
        }
        (bbs, insts)
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct ModuleMutatePass {
    ran: Arc<AtomicU32>,
}
impl LlvmModulePass for ModuleMutatePass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        self.ran.fetch_add(1, Ordering::SeqCst);
        set_pass_delta(module, DELTA_MODULE);
        PreservedAnalyses::None
    }
}

struct FunctionMutatePass {
    ran: Arc<AtomicU32>,
}
impl LlvmFunctionPass for FunctionMutatePass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        self.ran.fetch_add(1, Ordering::SeqCst);
        if function.get_name().to_string_lossy() == "driver" {
            // SAFETY: function originates from LLVM pass callback and is valid for callback duration.
            unsafe { set_pass_delta_from_function_ref(function.as_value_ref(), DELTA_FUNCTION) };
        }
        PreservedAnalyses::None
    }
}

struct CgsccUsesAnalysisPass {
    ran: Arc<AtomicU32>,
    registered: Arc<AtomicBool>,
}
impl LlvmCgsccPass for CgsccUsesAnalysisPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        self.ran.fetch_add(1, Ordering::SeqCst);
        if self
            .registered
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(CgsccNameLenAnalysis);
        }
        let _name_len = *manager.get_result::<CgsccNameLenAnalysis>(function);
        if function.get_name().to_string_lossy() == "driver" {
            // SAFETY: function originates from LLVM pass callback and is valid for callback duration.
            unsafe { set_pass_delta_from_function_ref(function.as_value_ref(), DELTA_CGSCC) };
        }
        PreservedAnalyses::None
    }
}

struct LoopUsesAnalysisPass {
    ran: Arc<AtomicU32>,
    registered: Arc<AtomicBool>,
}
impl LlvmLoopPass for LoopUsesAnalysisPass {
    fn run_pass(
        &self,
        loop_header: llvm_pm::traits::LLVMBasicBlockRef,
        manager: &LoopAnalysisManager,
    ) -> PreservedAnalyses {
        self.ran.fetch_add(1, Ordering::SeqCst);
        if self
            .registered
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(LoopHeaderNonNullAnalysis);
        }
        let ok = *manager.get_result::<LoopHeaderNonNullAnalysis>(loop_header);
        if ok {
            // SAFETY: loop_header originates from LLVM pass callback and belongs to a live function.
            let function = unsafe { LLVMGetBasicBlockParent(loop_header) };
            assert!(!function.is_null(), "loop header must have parent function");
            // SAFETY: parent function pointer was obtained from a valid loop header.
            unsafe { set_pass_delta_from_function_ref(function, DELTA_LOOP) };
        }
        PreservedAnalyses::None
    }
}

struct ModuleAnalysisOnlyPass;
impl LlvmModuleAnalysis for ModuleAnalysisOnlyPass {
    type Result = ();
    fn run_analysis(
        &self,
        _module: &inkwell::module::Module<'_>,
        _manager: &ModuleAnalysisManager,
    ) -> Self::Result {
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct FunctionAnalysisOnlyPass;
impl LlvmFunctionAnalysis for FunctionAnalysisOnlyPass {
    type Result = ();
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &FunctionAnalysisManager,
    ) -> Self::Result {
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct CgsccAnalysisOnlyPass;
impl LlvmCgsccAnalysis for CgsccAnalysisOnlyPass {
    type Result = ();
    fn run_analysis(
        &self,
        _function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

struct LoopAnalysisOnlyPass;
impl LlvmLoopAnalysis for LoopAnalysisOnlyPass {
    type Result = ();
    fn run_analysis(
        &self,
        _loop_header: llvm_pm::traits::LLVMBasicBlockRef,
        _manager: &LoopAnalysisManager,
    ) -> Self::Result {
    }
    fn id() -> AnalysisKey {
        static ID: u8 = 0;
        &ID
    }
}

fn assert_rich_ir(module: &inkwell::module::Module<'_>) {
    let mut min_insts = u32::MAX;
    for f in module.get_functions() {
        for bb in f.get_basic_blocks() {
            let insts = bb.get_instructions().count() as u32;
            min_insts = min_insts.min(insts);
        }
    }
    assert!(
        min_insts >= 3,
        "expected each basic block to contain multiple instructions, min={min_insts}"
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_demo_module();
    module.verify()?;
    assert_rich_ir(&module);

    let module_ran = Arc::new(AtomicU32::new(0));
    let function_ran = Arc::new(AtomicU32::new(0));
    let cgscc_ran = Arc::new(AtomicU32::new(0));
    let loop_ran = Arc::new(AtomicU32::new(0));

    let mut module_pm = ModulePassManager::with_opt_level(None, OptLevel::O2, None)?;
    module_pm.add_pass((ModuleAnalysisOnlyPass).into_pass());
    module_pm.add_pass((ModuleStatsAnalysis).into_pass());
    module_pm.add_pass(ModuleMutatePass {
        ran: module_ran.clone(),
    });
    module_pm.add_cgscc_pass((CgsccAnalysisOnlyPass).into_pass());
    module_pm.add_cgscc_pass(CgsccUsesAnalysisPass {
        ran: cgscc_ran.clone(),
        registered: Arc::new(AtomicBool::new(false)),
    });
    module_pm.run(&module)?;

    let mut function_pm = FunctionPassManager::with_pipeline(
        None,
        "loop-simplify,loop-rotate,instcombine,simplifycfg",
        None,
    )?;
    function_pm.add_pass((FunctionAnalysisOnlyPass).into_pass());
    function_pm.add_pass((FunctionStatsAnalysis).into_pass());
    function_pm.add_pass(FunctionMutatePass {
        ran: function_ran.clone(),
    });
    function_pm.add_loop_pass((LoopAnalysisOnlyPass).into_pass());
    function_pm.add_loop_pass(LoopUsesAnalysisPass {
        ran: loop_ran.clone(),
        registered: Arc::new(AtomicBool::new(false)),
    });

    for f in module.get_functions() {
        if f.count_basic_blocks() == 0 {
            continue;
        }
        function_pm.run(f)?;
    }

    module.verify()?;

    assert_eq!(
        get_pass_delta(&module),
        DELTA_LOOP,
        "final delta must reflect loop-pass mutation"
    );
    assert!(module_ran.load(Ordering::SeqCst) >= 1);
    assert!(function_ran.load(Ordering::SeqCst) >= 3);
    assert!(cgscc_ran.load(Ordering::SeqCst) >= 3);
    assert!(loop_ran.load(Ordering::SeqCst) >= 1);

    // Execute the final IR and assert deterministic output.
    // driver(12, 5):
    // callee_cond(12)=11, helper_loop(5)=10, delta=13 => adjusted=34 => returns 35.
    let ee = module.create_jit_execution_engine(OptimizationLevel::None)?;
    type DriverFn = unsafe extern "C" fn(i32, i32) -> i32;
    // SAFETY: "driver" exists with signature i32(i32, i32) in this module.
    let driver = unsafe { ee.get_function::<DriverFn>("driver") }?;
    // SAFETY: calling with valid i32 arguments matches the JIT symbol signature.
    let got = unsafe { driver.call(12, 5) };
    assert_eq!(got, 35, "unexpected final JIT result");

    println!(
        "ok: delta={}, module_passes={}, function_passes={}, cgscc_passes={}, loop_passes={}, driver(12,5)={}",
        get_pass_delta(&module),
        module_ran.load(Ordering::SeqCst),
        function_ran.load(Ordering::SeqCst),
        cgscc_ran.load(Ordering::SeqCst),
        loop_ran.load(Ordering::SeqCst),
        got
    );
    Ok(())
}
