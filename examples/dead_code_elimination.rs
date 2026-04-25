//! Simplified Dead Code Elimination (DCE) example.
//!
//! Demonstrates a realistic pass pipeline inspired by LLVM's `DCEPass`:
//!
//! 1. `InstructionCountAnalysis` (FunctionAnalysis via `into_pass()`) — counts
//!    instructions per function and stores stats in shared state
//! 2. `SimpleDCEPass` (FunctionPass) — removes trivially dead instructions
//!    (no uses, not terminators, not side-effecting)
//! 3. `DCEStatsPass` (ModulePass) — reports per-function instruction counts
//!
//! The analysis pass stores its results in a shared `Arc<Mutex<HashMap>>`,
//! which downstream passes read. This pattern shows how passes in the same
//! pipeline can share analysis information.
//!
//! Run: `cargo run --example dead_code_elimination`

use llvm_pm::inkwell;
use llvm_pm::inkwell::context::Context;
use llvm_pm::inkwell::values::InstructionOpcode;
use llvm_pm::inkwell::IntPredicate;
use llvm_pm::traits::{
    LlvmCgsccAnalysis, LlvmCgsccPass, LlvmFunctionPass, LlvmModulePass, PreservedAnalyses,
};
use llvm_pm::{
    CgsccAnalysisManager, FunctionAnalysisManager, FunctionPassManager, ModuleAnalysisManager,
    ModulePassManager,
};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

// =========================================================================
// Analysis: FunctionNameLengthAnalysis (CGSCC-level, supports get_result)
// =========================================================================

/// A CGSCC-level analysis that returns the number of basic blocks in a function.
///
/// Demonstrates the `get_result()` pattern available on `CgsccAnalysisManager`.
/// CGSCC and Loop analyses support on-demand evaluation: a pass calls
/// `manager.get_result::<Analysis>(ir_unit)` and the analysis is computed
/// (and cached) automatically.
struct BasicBlockCountAnalysis;

impl LlvmCgsccAnalysis for BasicBlockCountAnalysis {
    type Result = u32;

    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        _manager: &CgsccAnalysisManager,
    ) -> Self::Result {
        function.count_basic_blocks()
    }

    fn id() -> *const u8 {
        static ID: u8 = 0;
        &ID
    }
}

// =========================================================================
// Pass: BasicBlockReportPass (CGSCC-level, uses analysis results)
// =========================================================================

/// A CGSCC pass that queries `BasicBlockCountAnalysis` for each function
/// and prints the result. This demonstrates the analysis-driven pass pattern
/// where a transformation (or inspection) pass consumes cached analysis data.
struct BasicBlockReportPass {
    registered: Arc<AtomicBool>,
}

impl LlvmCgsccPass for BasicBlockReportPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        // Register analysis on first invocation (once per PM lifetime).
        if self
            .registered
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            manager.add_analysis(BasicBlockCountAnalysis);
        }

        // Query the analysis — computed on first call, cached thereafter.
        let bb_count = *manager.get_result::<BasicBlockCountAnalysis>(function);
        let name = function.get_name().to_string_lossy();
        println!("  [CGSCC] {}: {} basic blocks", name, bb_count);

        PreservedAnalyses::All
    }
}

// =========================================================================
// Pass: SimpleDCEPass (function-level transformation)
// =========================================================================

/// A simplified Dead Code Elimination pass.
///
/// Removes instructions that:
/// - Have no uses (the result is unused)
/// - Are not terminators (br, ret, switch, etc.)
/// - Are not side-effecting (store, call, fence, etc.)
///
/// This mirrors a subset of LLVM's `DCEPass` (`llvm/lib/Transforms/Scalar/DCE.cpp`).
struct SimpleDCEPass {
    removed: Arc<AtomicU32>,
}

/// Returns true if the instruction may have side effects and should not be
/// removed even when its result is unused.
fn may_have_side_effects(opcode: InstructionOpcode) -> bool {
    matches!(
        opcode,
        InstructionOpcode::Store
            | InstructionOpcode::Call
            | InstructionOpcode::Fence
            | InstructionOpcode::AtomicCmpXchg
            | InstructionOpcode::AtomicRMW
            | InstructionOpcode::Resume
            | InstructionOpcode::LandingPad
            | InstructionOpcode::IndirectBr
            | InstructionOpcode::Invoke
            | InstructionOpcode::Unreachable
    )
}

impl LlvmFunctionPass for SimpleDCEPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        _manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        let mut changed = false;

        // Iterate over all basic blocks and collect dead instructions.
        // We collect first to avoid invalidating the iterator during deletion.
        for bb in function.get_basic_blocks() {
            let dead: Vec<_> = bb
                .get_instructions()
                .filter(|inst| {
                    if inst.is_terminator() {
                        return false;
                    }
                    if may_have_side_effects(inst.get_opcode()) {
                        return false;
                    }
                    if inst.get_opcode() == InstructionOpcode::Alloca {
                        return false;
                    }
                    inst.get_first_use().is_none()
                })
                .collect();

            for inst in dead {
                inst.erase_from_basic_block();
                self.removed.fetch_add(1, Ordering::SeqCst);
                changed = true;
            }
        }

        if changed {
            PreservedAnalyses::None
        } else {
            PreservedAnalyses::All
        }
    }
}

// =========================================================================
// Pass: DCEStatsPass (module-level reporting)
// =========================================================================

/// Module pass that prints per-function instruction counts.
struct DCEStatsPass {
    label: &'static str,
}

impl LlvmModulePass for DCEStatsPass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        _manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        println!("--- {} ---", self.label);
        let mut total_insts = 0u32;
        let mut total_bbs = 0u32;
        for func in module.get_functions() {
            if func.count_basic_blocks() == 0 {
                continue;
            }
            let mut insts = 0u32;
            for bb in func.get_basic_blocks() {
                insts += bb.get_instructions().count() as u32;
            }
            let bbs = func.count_basic_blocks();
            let name = func.get_name().to_string_lossy();
            println!("  {}: {} BBs, {} instructions", name, bbs, insts);
            total_insts += insts;
            total_bbs += bbs;
        }
        println!("  Total: {} BBs, {} instructions", total_bbs, total_insts);
        PreservedAnalyses::All
    }
}

// =========================================================================
// IR construction: module with dead code
// =========================================================================

/// Build a module containing functions with deliberately dead instructions
/// that our SimpleDCEPass can remove.
fn build_module_with_dead_code() -> inkwell::module::Module<'static> {
    let context = Box::leak(Box::new(Context::create()));
    let module = context.create_module("dce_demo");
    let i32_ty = context.i32_type();

    // i32 @add_with_dead_code(i32 %a, i32 %b)
    // Contains several dead intermediate values.
    let fn_ty = i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false);
    let func = module.add_function("add_with_dead_code", fn_ty, None);
    let bb = context.append_basic_block(func, "entry");
    let b = context.create_builder();
    b.position_at_end(bb);

    let a = func.get_nth_param(0).unwrap().into_int_value();
    let b_param = func.get_nth_param(1).unwrap().into_int_value();

    // Live: the actual return value
    let sum = b.build_int_add(a, b_param, "sum").unwrap();

    // Dead: computed but never used
    let _dead_mul = b.build_int_mul(a, b_param, "dead_mul").unwrap();
    let _dead_sub = b.build_int_sub(a, b_param, "dead_sub").unwrap();
    let _dead_xor = b.build_xor(a, b_param, "dead_xor").unwrap();

    b.build_return(Some(&sum)).unwrap();

    // i32 @branch_with_dead_code(i32 %x)
    // Has dead code in both branches.
    let fn_ty2 = i32_ty.fn_type(&[i32_ty.into()], false);
    let func2 = module.add_function("branch_with_dead_code", fn_ty2, None);
    let entry = context.append_basic_block(func2, "entry");
    let then_bb = context.append_basic_block(func2, "then");
    let else_bb = context.append_basic_block(func2, "else");
    let merge = context.append_basic_block(func2, "merge");
    let b = context.create_builder();

    b.position_at_end(entry);
    let x = func2.get_first_param().unwrap().into_int_value();
    let cmp = b
        .build_int_compare(IntPredicate::SGT, x, i32_ty.const_int(10, false), "cmp")
        .unwrap();
    b.build_conditional_branch(cmp, then_bb, else_bb).unwrap();

    b.position_at_end(then_bb);
    let then_val = b
        .build_int_mul(x, i32_ty.const_int(2, false), "then_val")
        .unwrap();
    let _dead_then = b
        .build_int_add(x, i32_ty.const_int(99, false), "dead_then")
        .unwrap();
    b.build_unconditional_branch(merge).unwrap();

    b.position_at_end(else_bb);
    let else_val = b
        .build_int_add(x, i32_ty.const_int(1, false), "else_val")
        .unwrap();
    let _dead_else = b
        .build_int_sub(x, i32_ty.const_int(42, false), "dead_else")
        .unwrap();
    let _dead_else2 = b.build_int_mul(x, x, "dead_else2").unwrap();
    b.build_unconditional_branch(merge).unwrap();

    b.position_at_end(merge);
    let phi = b.build_phi(i32_ty, "result").unwrap();
    phi.add_incoming(&[(&then_val, then_bb), (&else_val, else_bb)]);
    let result = phi.as_basic_value().into_int_value();
    b.build_return(Some(&result)).unwrap();

    module
}

// =========================================================================
// Main
// =========================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = build_module_with_dead_code();
    module.verify()?;

    let removed = Arc::new(AtomicU32::new(0));

    // Phase 1: Report initial stats + CGSCC analysis (before DCE)
    {
        let mut pm = ModulePassManager::new(None, None)?;
        pm.add_pass(DCEStatsPass {
            label: "Before DCE",
        });
        pm.add_cgscc_pass(BasicBlockReportPass {
            registered: Arc::new(AtomicBool::new(false)),
        });
        pm.run(&module)?;
    }

    // Phase 2: Run DCE on each function
    {
        let mut fpm = FunctionPassManager::new(None, None)?;
        fpm.add_pass(SimpleDCEPass {
            removed: removed.clone(),
        });
        for func in module.get_functions() {
            if func.count_basic_blocks() == 0 {
                continue;
            }
            fpm.run(func)?;
        }
    }

    module.verify()?;

    // Phase 3: Report final stats (after DCE)
    {
        let mut pm = ModulePassManager::new(None, None)?;
        pm.add_pass(DCEStatsPass {
            label: "After DCE",
        });
        pm.run(&module)?;
    }

    let total_removed = removed.load(Ordering::SeqCst);
    println!("\nRemoved {} dead instructions", total_removed);
    assert!(
        total_removed >= 5,
        "Expected to remove at least 5 dead instructions, got {}",
        total_removed
    );

    println!("ok");
    Ok(())
}
