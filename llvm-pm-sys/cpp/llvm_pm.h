#ifndef LLVM_PM_H
#define LLVM_PM_H

#include <llvm-c/Types.h>
#include <llvm-c/TargetMachine.h>

#ifdef __cplusplus
extern "C" {
#endif

/* =========================================================================
 * Opaque types
 * ========================================================================= */

/** Bundled pass-manager infrastructure (analysis managers, PassBuilder, PM). */
typedef struct LlvmPmOpaquePassManager *LlvmPmPassManagerRef;

/** Options for creating pass managers. */
typedef struct LlvmPmOpaqueOptions *LlvmPmOptionsRef;

/* =========================================================================
 * Enums
 * ========================================================================= */

/** Maps to llvm::OptimizationLevel. */
typedef enum {
    LlvmPmOptLevel_O0 = 0,
    LlvmPmOptLevel_O1 = 1,
    LlvmPmOptLevel_O2 = 2,
    LlvmPmOptLevel_O3 = 3,
    LlvmPmOptLevel_Os = 4,
    LlvmPmOptLevel_Oz = 5,
} LlvmPmOptLevel;

/* =========================================================================
 * Options
 * ========================================================================= */

/** Create default options (debug_logging=false, verify_each=false). */
LlvmPmOptionsRef llvm_pm_options_create(void);

/** Dispose of options. opts may be NULL (no-op). */
void llvm_pm_options_dispose(LlvmPmOptionsRef opts);

/** Enable debug logging output during pass execution. */
void llvm_pm_options_set_debug_logging(LlvmPmOptionsRef opts, LLVMBool val);

/** Enable IR verification after each pass. */
void llvm_pm_options_set_verify_each(LlvmPmOptionsRef opts, LLVMBool val);

/* --- Extension point pipeline additions ---
 * Each adds a textual pipeline string at the named extension point.
 * Function-level EPs parse as function passes.
 * Module-level EPs parse as module passes. */

/** After instruction combiner (function-level). */
void llvm_pm_options_add_peephole_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** Before main function optimization pipeline (module-level). */
void llvm_pm_options_add_optimizer_early_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** At end of function optimization pipeline (module-level). */
void llvm_pm_options_add_optimizer_last_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** Before vectorizer (function-level). */
void llvm_pm_options_add_vectorizer_start_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** After main scalar optimizations, before cleanup (function-level). */
void llvm_pm_options_add_scalar_optimizer_late_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** At start of the pipeline (module-level). */
void llvm_pm_options_add_pipeline_start_ep(LlvmPmOptionsRef opts, const char *pipeline);

/** Right after basic IR simplification (module-level). */
void llvm_pm_options_add_pipeline_early_simplification_ep(
    LlvmPmOptionsRef opts, const char *pipeline);

/* =========================================================================
 * Module pass manager creation
 * ========================================================================= */

/**
 * Create a ModulePassManager using a standard optimization pipeline.
 * @param options  Optional (NULL for defaults).
 */
LlvmPmPassManagerRef llvm_pm_create_with_opt_level(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg);

/**
 * Create a ModulePassManager from a textual pipeline string.
 * @param options  Optional (NULL for defaults).
 */
LlvmPmPassManagerRef llvm_pm_create_with_pipeline(
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    LlvmPmOptionsRef options,
    char **err_msg);

/**
 * Create a ModulePassManager with the full-LTO default pipeline.
 * Uses no export summary (pass NULL internally).
 */
LlvmPmPassManagerRef llvm_pm_create_lto(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg);

/**
 * Create a ModulePassManager with the full-LTO pre-link pipeline.
 */
LlvmPmPassManagerRef llvm_pm_create_lto_pre_link(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg);

/**
 * Create a ModulePassManager with the ThinLTO pre-link pipeline.
 */
LlvmPmPassManagerRef llvm_pm_create_thin_lto_pre_link(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg);

/* =========================================================================
 * Function pass manager creation
 * ========================================================================= */

/**
 * Create a FunctionPassManager from a textual pipeline string.
 * The pipeline should contain function-level passes (e.g. "instcombine,dce").
 */
LlvmPmPassManagerRef llvm_pm_create_function_with_pipeline(
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    LlvmPmOptionsRef options,
    char **err_msg);

/* =========================================================================
 * Custom pass callbacks
 * ========================================================================= */

/** Callback for a custom module pass. Return 0 = PreservedAnalyses::all(), 1 = none(). */
typedef int (*LlvmPmModulePassCallback)(LLVMModuleRef module, void *manager, void *user_data);

/** Callback for a custom function pass. Return 0 = PreservedAnalyses::all(), 1 = none(). */
typedef int (*LlvmPmFunctionPassCallback)(LLVMValueRef function, void *manager, void *user_data);

/** Callback for a custom CGSCC pass. Called for each function in an SCC. */
typedef int (*LlvmPmCGSCCPassCallback)(LLVMValueRef function, void *manager, void *user_data);

/** Callback for a custom loop pass. Called with the loop header block. */
typedef int (*LlvmPmLoopPassCallback)(LLVMBasicBlockRef header, void *manager, void *user_data);

/** Add a custom module pass (appended after any existing passes). */
void llvm_pm_add_module_pass(
    LlvmPmPassManagerRef pm, LlvmPmModulePassCallback callback, void *user_data);

/** Add a custom function pass (appended after any existing passes). */
void llvm_pm_add_function_pass(
    LlvmPmPassManagerRef pm, LlvmPmFunctionPassCallback callback, void *user_data);

/** Add a custom CGSCC pass (adapted into the module pipeline). */
void llvm_pm_add_cgscc_pass(
    LlvmPmPassManagerRef pm, LlvmPmCGSCCPassCallback callback, void *user_data);

/** Add a custom loop pass (adapted into the function pipeline). */
void llvm_pm_add_loop_pass(
    LlvmPmPassManagerRef pm, LlvmPmLoopPassCallback callback, void *user_data);

/* =========================================================================
 * Empty pass manager creation (for custom-pass-only pipelines)
 * ========================================================================= */

/** Create a ModulePassManager with no built-in passes. Add custom passes via llvm_pm_add_module_pass. */
LlvmPmPassManagerRef llvm_pm_create_empty_module(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptionsRef options,
    char **err_msg);

/** Create a FunctionPassManager with no built-in passes. Add custom passes via llvm_pm_add_function_pass. */
LlvmPmPassManagerRef llvm_pm_create_empty_function(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptionsRef options,
    char **err_msg);

/* =========================================================================
 * Execution
 * ========================================================================= */

/** Run the module pass manager on a module. Returns NULL on success. */
char *llvm_pm_run(LlvmPmPassManagerRef pm, LLVMModuleRef module);

/** Run the function pass manager on a single function. Returns NULL on success. */
char *llvm_pm_run_on_function(LlvmPmPassManagerRef pm, LLVMValueRef function);

/* =========================================================================
 * Disposal
 * ========================================================================= */

/** Dispose of the pass manager. pm may be NULL (no-op). */
void llvm_pm_dispose(LlvmPmPassManagerRef pm);

/** Free an error/info message. msg may be NULL (no-op). */
void llvm_pm_dispose_message(char *msg);

#ifdef __cplusplus
}
#endif

#endif /* LLVM_PM_H */
