#ifndef LLVM_PM_H
#define LLVM_PM_H

#include <llvm-c/Types.h>
#include <llvm-c/TargetMachine.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Opaque handle to the full pass-manager infrastructure bundle.
 *
 * Internally holds PassInstrumentationCallbacks, StandardInstrumentations,
 * LoopAnalysisManager, FunctionAnalysisManager, CGSCCAnalysisManager,
 * ModuleAnalysisManager, PassBuilder, and ModulePassManager.
 */
typedef struct LlvmPmOpaquePassManager *LlvmPmPassManagerRef;

/** Maps to llvm::OptimizationLevel. */
typedef enum {
    LlvmPmOptLevel_O0 = 0,
    LlvmPmOptLevel_O1 = 1,
    LlvmPmOptLevel_O2 = 2,
    LlvmPmOptLevel_O3 = 3,
    LlvmPmOptLevel_Os = 4,
    LlvmPmOptLevel_Oz = 5,
} LlvmPmOptLevel;

/**
 * Create a ModulePassManager using a standard optimization pipeline.
 *
 * @param context      The LLVM context.
 * @param target_machine  Optional target machine (NULL for generic optimizations).
 * @param level        Optimization level.
 * @param err_msg      On failure, set to a malloc'd error string.
 *                     Caller must free via llvm_pm_dispose_message().
 * @return Non-NULL on success, NULL on failure.
 */
LlvmPmPassManagerRef llvm_pm_create_with_opt_level(
    LLVMContextRef context,
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    char **err_msg);

/**
 * Create a ModulePassManager from a textual pipeline string.
 *
 * The format matches `opt -passes=...` syntax, e.g.:
 *   "instcombine,dce,sroa"
 *   "default<O2>"
 *   "module(function(instcombine,sroa))"
 *
 * @param context         The LLVM context.
 * @param target_machine  Optional target machine (NULL for generic).
 * @param pipeline        Pipeline description string.
 * @param err_msg         On failure, set to a malloc'd error string.
 * @return Non-NULL on success, NULL on failure.
 */
LlvmPmPassManagerRef llvm_pm_create_with_pipeline(
    LLVMContextRef context,
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    char **err_msg);

/**
 * Run the pass manager on a module.
 *
 * @param pm      The pass manager.
 * @param module  The module to optimize.
 * @return NULL on success, or a malloc'd error string on failure.
 */
char *llvm_pm_run(LlvmPmPassManagerRef pm, LLVMModuleRef module);

/**
 * Dispose of the pass manager and all owned infrastructure.
 * pm may be NULL (no-op).
 */
void llvm_pm_dispose(LlvmPmPassManagerRef pm);

/**
 * Free an error message returned by other functions.
 * msg may be NULL (no-op).
 */
void llvm_pm_dispose_message(char *msg);

#ifdef __cplusplus
}
#endif

#endif /* LLVM_PM_H */
