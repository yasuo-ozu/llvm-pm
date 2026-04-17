#include "llvm_pm.h"

#include <llvm/Config/llvm-config.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/PassInstrumentation.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Passes/StandardInstrumentations.h>
#include <llvm/Analysis/CGSCCPassManager.h>
#include <llvm/Analysis/LoopAnalysisManager.h>
#include <llvm/Analysis/LazyCallGraph.h>
#include <llvm/Transforms/Scalar/LoopPassManager.h>
#include <llvm/Target/TargetMachine.h>
#include <llvm/Support/Error.h>
#include <llvm/Support/raw_ostream.h>

#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

using namespace llvm;

// ===== Options =====

struct LlvmPmOpaqueOptions {
    bool DebugLogging = false;
    bool VerifyEach = false;
    // Extension point pipelines
    std::vector<std::string> PeepholeEPs;
    std::vector<std::string> OptimizerEarlyEPs;
    std::vector<std::string> OptimizerLastEPs;
    std::vector<std::string> VectorizerStartEPs;
    std::vector<std::string> ScalarOptimizerLateEPs;
    std::vector<std::string> PipelineStartEPs;
    std::vector<std::string> PipelineEarlySimplificationEPs;
};

// ===== Pass manager bundle =====

struct LlvmPmOpaquePassManager {
    std::unique_ptr<PassInstrumentationCallbacks> PIC;
    std::unique_ptr<StandardInstrumentations> SI;
    std::unique_ptr<LoopAnalysisManager> LAM;
    std::unique_ptr<FunctionAnalysisManager> FAM;
    std::unique_ptr<CGSCCAnalysisManager> CGAM;
    std::unique_ptr<ModuleAnalysisManager> MAM;
    std::unique_ptr<PassBuilder> PB;
    std::unique_ptr<ModulePassManager> MPM;
    std::unique_ptr<FunctionPassManager> FPM;

    // Options saved from construction for deferred SI initialization
    bool DebugLogging = false;
    bool VerifyEach = false;

    // Old PIC/SI pairs kept alive so cached PassInstrumentation analysis
    // results (which hold PIC pointers) do not dangle.
    std::vector<std::unique_ptr<PassInstrumentationCallbacks>> RetiredPICs;
    std::vector<std::unique_ptr<StandardInstrumentations>> RetiredSIs;

    ~LlvmPmOpaquePassManager() {
        MPM.reset();
        FPM.reset();
        PB.reset();
        MAM.reset();
        CGAM.reset();
        FAM.reset();
        LAM.reset();
        SI.reset();
        PIC.reset();
        RetiredSIs.clear();
        RetiredPICs.clear();
    }
};

// ===== Helpers =====

static Module *unwrapModule(LLVMModuleRef M) {
    return reinterpret_cast<Module *>(M);
}

static TargetMachine *unwrapTM(LLVMTargetMachineRef TM) {
    return reinterpret_cast<TargetMachine *>(TM);
}

static Function *unwrapFunction(LLVMValueRef V) {
    return reinterpret_cast<Function *>(V);
}

static char *copyString(const std::string &msg) {
    char *buf = static_cast<char *>(std::malloc(msg.size() + 1));
    if (buf)
        std::memcpy(buf, msg.c_str(), msg.size() + 1);
    return buf;
}

static OptimizationLevel mapOptLevel(LlvmPmOptLevel level) {
    switch (level) {
    case LlvmPmOptLevel_O0: return OptimizationLevel::O0;
    case LlvmPmOptLevel_O1: return OptimizationLevel::O1;
    case LlvmPmOptLevel_O2: return OptimizationLevel::O2;
    case LlvmPmOptLevel_O3: return OptimizationLevel::O3;
    case LlvmPmOptLevel_Os: return OptimizationLevel::Os;
    case LlvmPmOptLevel_Oz: return OptimizationLevel::Oz;
    default: return OptimizationLevel::O2;
    }
}

/// Dereference an optional LlvmPmOptionsRef, returning a default if NULL.
static const LlvmPmOpaqueOptions &derefOpts(LlvmPmOptionsRef opts) {
    static const LlvmPmOpaqueOptions defaultOpts;
    return opts ? *opts : defaultOpts;
}

/// Create common infrastructure: PIC, analysis managers, PassBuilder.
/// Registers all analyses, cross-registers proxies, and registers extension
/// point callbacks from options.
/// StandardInstrumentations is deferred to run() time, where the LLVMContext
/// is obtained from the module/function being optimized.
static LlvmPmOpaquePassManager *createInfrastructure(
    LLVMTargetMachineRef target_machine,
    const LlvmPmOpaqueOptions &opts)
{
    auto *pm = new LlvmPmOpaquePassManager();

    TargetMachine *TM = target_machine ? unwrapTM(target_machine) : nullptr;

    pm->DebugLogging = opts.DebugLogging;
    pm->VerifyEach = opts.VerifyEach;

    pm->PIC = std::make_unique<PassInstrumentationCallbacks>();

    pm->LAM = std::make_unique<LoopAnalysisManager>();
    pm->FAM = std::make_unique<FunctionAnalysisManager>();
    pm->CGAM = std::make_unique<CGSCCAnalysisManager>();
    pm->MAM = std::make_unique<ModuleAnalysisManager>();

    pm->PB = std::make_unique<PassBuilder>(
        TM, PipelineTuningOptions(), std::nullopt, pm->PIC.get());

    PassBuilder *PBPtr = pm->PB.get();

    for (const auto &p : opts.PeepholeEPs) {
        pm->PB->registerPeepholeEPCallback(
            [PBPtr, p](FunctionPassManager &FPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(FPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.OptimizerEarlyEPs) {
        pm->PB->registerOptimizerEarlyEPCallback(
            [PBPtr, p](ModulePassManager &MPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(MPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.OptimizerLastEPs) {
        pm->PB->registerOptimizerLastEPCallback(
            [PBPtr, p](ModulePassManager &MPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(MPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.VectorizerStartEPs) {
        pm->PB->registerVectorizerStartEPCallback(
            [PBPtr, p](FunctionPassManager &FPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(FPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.ScalarOptimizerLateEPs) {
        pm->PB->registerScalarOptimizerLateEPCallback(
            [PBPtr, p](FunctionPassManager &FPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(FPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.PipelineStartEPs) {
        pm->PB->registerPipelineStartEPCallback(
            [PBPtr, p](ModulePassManager &MPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(MPM, p))
                    consumeError(std::move(Err));
            });
    }
    for (const auto &p : opts.PipelineEarlySimplificationEPs) {
        pm->PB->registerPipelineEarlySimplificationEPCallback(
            [PBPtr, p](ModulePassManager &MPM, auto&&...) {
                if (auto Err = PBPtr->parsePassPipeline(MPM, p))
                    consumeError(std::move(Err));
            });
    }

    pm->PB->registerModuleAnalyses(*pm->MAM);
    pm->PB->registerCGSCCAnalyses(*pm->CGAM);
    pm->PB->registerFunctionAnalyses(*pm->FAM);
    pm->PB->registerLoopAnalyses(*pm->LAM);
    pm->PB->crossRegisterProxies(*pm->LAM, *pm->FAM, *pm->CGAM, *pm->MAM);

    return pm;
}

/// Re-initialize PIC and StandardInstrumentations for a run() call.
/// Old PIC/SI are retired (kept alive) so that any cached
/// PassInstrumentation analysis results do not dangle.
static void reinitSI(LlvmPmOpaquePassManager *pm, LLVMContext &Ctx) {
    // Retire old PIC+SI (keep alive for cached analysis results)
    if (pm->SI) pm->RetiredSIs.push_back(std::move(pm->SI));
    if (pm->PIC) pm->RetiredPICs.push_back(std::move(pm->PIC));

    // Create fresh PIC+SI with the current context
    pm->PIC = std::make_unique<PassInstrumentationCallbacks>();
    pm->SI = std::make_unique<StandardInstrumentations>(
        Ctx, pm->DebugLogging, pm->VerifyEach);
    pm->SI->registerCallbacks(*pm->PIC);

    // Re-register PassInstrumentationAnalysis in all analysis managers
    pm->LAM->registerPass([&] { return PassInstrumentationAnalysis(pm->PIC.get()); });
    pm->FAM->registerPass([&] { return PassInstrumentationAnalysis(pm->PIC.get()); });
    pm->CGAM->registerPass([&] { return PassInstrumentationAnalysis(pm->PIC.get()); });
    pm->MAM->registerPass([&] { return PassInstrumentationAnalysis(pm->PIC.get()); });
}

// ===== Options API =====

extern "C" LlvmPmOptionsRef llvm_pm_options_create(void) {
    return new LlvmPmOpaqueOptions();
}

extern "C" void llvm_pm_options_dispose(LlvmPmOptionsRef opts) {
    delete opts;
}

extern "C" void llvm_pm_options_set_debug_logging(LlvmPmOptionsRef opts, LLVMBool val) {
    opts->DebugLogging = val != 0;
}

extern "C" void llvm_pm_options_set_verify_each(LlvmPmOptionsRef opts, LLVMBool val) {
    opts->VerifyEach = val != 0;
}

extern "C" void llvm_pm_options_add_peephole_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->PeepholeEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_optimizer_early_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->OptimizerEarlyEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_optimizer_last_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->OptimizerLastEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_vectorizer_start_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->VectorizerStartEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_scalar_optimizer_late_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->ScalarOptimizerLateEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_pipeline_start_ep(LlvmPmOptionsRef opts, const char *pipeline) {
    opts->PipelineStartEPs.emplace_back(pipeline);
}

extern "C" void llvm_pm_options_add_pipeline_early_simplification_ep(
    LlvmPmOptionsRef opts, const char *pipeline) {
    opts->PipelineEarlySimplificationEPs.emplace_back(pipeline);
}

// ===== Module pass manager creation =====

extern "C" LlvmPmPassManagerRef llvm_pm_create_with_opt_level(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    OptimizationLevel opt = mapOptLevel(level);

    pm->MPM = std::make_unique<ModulePassManager>();
    if (opt == OptimizationLevel::O0) {
        *pm->MPM = pm->PB->buildO0DefaultPipeline(opt);
    } else {
        *pm->MPM = pm->PB->buildPerModuleDefaultPipeline(opt);
    }

    *err_msg = nullptr;
    return pm;
}

extern "C" LlvmPmPassManagerRef llvm_pm_create_with_pipeline(
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    pm->MPM = std::make_unique<ModulePassManager>();

    if (auto Err = pm->PB->parsePassPipeline(*pm->MPM, StringRef(pipeline))) {
        *err_msg = copyString(toString(std::move(Err)));
        delete pm;
        return nullptr;
    }

    *err_msg = nullptr;
    return pm;
}

extern "C" LlvmPmPassManagerRef llvm_pm_create_lto(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    OptimizationLevel opt = mapOptLevel(level);

    pm->MPM = std::make_unique<ModulePassManager>();
    *pm->MPM = pm->PB->buildLTODefaultPipeline(opt, /*ExportSummary=*/nullptr);

    *err_msg = nullptr;
    return pm;
}

extern "C" LlvmPmPassManagerRef llvm_pm_create_lto_pre_link(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    OptimizationLevel opt = mapOptLevel(level);

    pm->MPM = std::make_unique<ModulePassManager>();
    *pm->MPM = pm->PB->buildLTOPreLinkDefaultPipeline(opt);

    *err_msg = nullptr;
    return pm;
}

extern "C" LlvmPmPassManagerRef llvm_pm_create_thin_lto_pre_link(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    OptimizationLevel opt = mapOptLevel(level);

    pm->MPM = std::make_unique<ModulePassManager>();
    *pm->MPM = pm->PB->buildThinLTOPreLinkDefaultPipeline(opt);

    *err_msg = nullptr;
    return pm;
}

// ===== Function pass manager creation =====

extern "C" LlvmPmPassManagerRef llvm_pm_create_function_with_pipeline(
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    pm->FPM = std::make_unique<FunctionPassManager>();

    if (auto Err = pm->PB->parsePassPipeline(*pm->FPM, StringRef(pipeline))) {
        *err_msg = copyString(toString(std::move(Err)));
        delete pm;
        return nullptr;
    }

    *err_msg = nullptr;
    return pm;
}

// ===== Custom pass wrappers =====

struct RustModulePass : public PassInfoMixin<RustModulePass> {
    LlvmPmModulePassCallback Callback;
    void *UserData;

    RustModulePass(LlvmPmModulePassCallback cb, void *data)
        : Callback(cb), UserData(data) {}

    PreservedAnalyses run(Module &M, ModuleAnalysisManager &MAM) {
        int result = Callback(
            reinterpret_cast<LLVMModuleRef>(&M),
            reinterpret_cast<void*>(&MAM),
            UserData);
        return result == 0 ? PreservedAnalyses::all() : PreservedAnalyses::none();
    }
};

struct RustFunctionPass : public PassInfoMixin<RustFunctionPass> {
    LlvmPmFunctionPassCallback Callback;
    void *UserData;

    RustFunctionPass(LlvmPmFunctionPassCallback cb, void *data)
        : Callback(cb), UserData(data) {}

    PreservedAnalyses run(Function &F, FunctionAnalysisManager &FAM) {
        int result = Callback(
            reinterpret_cast<LLVMValueRef>(&F),
            reinterpret_cast<void*>(&FAM),
            UserData);
        return result == 0 ? PreservedAnalyses::all() : PreservedAnalyses::none();
    }
};

struct RustCGSCCPass : public PassInfoMixin<RustCGSCCPass> {
    LlvmPmCGSCCPassCallback Callback;
    void *UserData;

    RustCGSCCPass(LlvmPmCGSCCPassCallback cb, void *data)
        : Callback(cb), UserData(data) {}

    PreservedAnalyses run(
        LazyCallGraph::SCC &C,
        CGSCCAnalysisManager &AM,
        LazyCallGraph &,
        CGSCCUpdateResult &)
    {
        bool preservesAll = true;
        for (LazyCallGraph::Node &N : C) {
            Function &F = N.getFunction();
            int result = Callback(
                reinterpret_cast<LLVMValueRef>(&F),
                reinterpret_cast<void*>(&AM),
                UserData);
            if (result != 0)
                preservesAll = false;
        }
        return preservesAll ? PreservedAnalyses::all() : PreservedAnalyses::none();
    }
};

struct RustLoopPass : public PassInfoMixin<RustLoopPass> {
    LlvmPmLoopPassCallback Callback;
    void *UserData;

    RustLoopPass(LlvmPmLoopPassCallback cb, void *data)
        : Callback(cb), UserData(data) {}

    PreservedAnalyses run(
        Loop &L,
        LoopAnalysisManager &AM,
        LoopStandardAnalysisResults &,
        LPMUpdater &)
    {
        int result = Callback(
            reinterpret_cast<LLVMBasicBlockRef>(L.getHeader()),
            reinterpret_cast<void*>(&AM),
            UserData);
        return result == 0 ? PreservedAnalyses::all() : PreservedAnalyses::none();
    }
};

extern "C" void llvm_pm_add_module_pass(
    LlvmPmPassManagerRef pm, LlvmPmModulePassCallback callback, void *user_data)
{
    if (!pm->MPM)
        pm->MPM = std::make_unique<ModulePassManager>();
    pm->MPM->addPass(RustModulePass(callback, user_data));
}

extern "C" void llvm_pm_add_function_pass(
    LlvmPmPassManagerRef pm, LlvmPmFunctionPassCallback callback, void *user_data)
{
    if (!pm->FPM)
        pm->FPM = std::make_unique<FunctionPassManager>();
    pm->FPM->addPass(RustFunctionPass(callback, user_data));
}

extern "C" void llvm_pm_add_cgscc_pass(
    LlvmPmPassManagerRef pm, LlvmPmCGSCCPassCallback callback, void *user_data)
{
    if (!pm->MPM)
        pm->MPM = std::make_unique<ModulePassManager>();
    pm->MPM->addPass(createModuleToPostOrderCGSCCPassAdaptor(
        RustCGSCCPass(callback, user_data)));
}

extern "C" void llvm_pm_add_loop_pass(
    LlvmPmPassManagerRef pm, LlvmPmLoopPassCallback callback, void *user_data)
{
    if (!pm->FPM)
        pm->FPM = std::make_unique<FunctionPassManager>();
    pm->FPM->addPass(createFunctionToLoopPassAdaptor(
        RustLoopPass(callback, user_data)));
}

// ===== Empty pass manager creation =====

extern "C" LlvmPmPassManagerRef llvm_pm_create_empty_module(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    pm->MPM = std::make_unique<ModulePassManager>();
    *err_msg = nullptr;
    return pm;
}

extern "C" LlvmPmPassManagerRef llvm_pm_create_empty_function(
    LLVMTargetMachineRef target_machine,
    LlvmPmOptionsRef options,
    char **err_msg)
{
    auto *pm = createInfrastructure(target_machine, derefOpts(options));
    pm->FPM = std::make_unique<FunctionPassManager>();
    *err_msg = nullptr;
    return pm;
}

// ===== Execution =====

extern "C" char *llvm_pm_run(LlvmPmPassManagerRef pm, LLVMModuleRef module) {
    if (!pm->MPM)
        return copyString("No module pass manager configured");
    Module *M = unwrapModule(module);
    reinitSI(pm, M->getContext());
    pm->MPM->run(*M, *pm->MAM);
    return nullptr;
}

extern "C" char *llvm_pm_run_on_function(LlvmPmPassManagerRef pm, LLVMValueRef function) {
    if (!pm->FPM)
        return copyString("No function pass manager configured");
    Function *F = unwrapFunction(function);
    reinitSI(pm, F->getContext());
    pm->FPM->run(*F, *pm->FAM);
    return nullptr;
}

// ===== Disposal =====

extern "C" void llvm_pm_dispose(LlvmPmPassManagerRef pm) {
    delete pm;
}

extern "C" void llvm_pm_dispose_message(char *msg) {
    std::free(msg);
}
