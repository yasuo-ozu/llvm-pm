#include "llvm_pm.h"

#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Module.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Passes/StandardInstrumentations.h>
#include <llvm/Analysis/CGSCCPassManager.h>
#include <llvm/Analysis/LoopAnalysisManager.h>
#include <llvm/Target/TargetMachine.h>
#include <llvm/Support/Error.h>
#include <llvm/Support/raw_ostream.h>

#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>

using namespace llvm;

// --- Internal struct ---

struct LlvmPmOpaquePassManager {
    std::unique_ptr<PassInstrumentationCallbacks> PIC;
    std::unique_ptr<StandardInstrumentations> SI;
    std::unique_ptr<LoopAnalysisManager> LAM;
    std::unique_ptr<FunctionAnalysisManager> FAM;
    std::unique_ptr<CGSCCAnalysisManager> CGAM;
    std::unique_ptr<ModuleAnalysisManager> MAM;
    std::unique_ptr<PassBuilder> PB;
    std::unique_ptr<ModulePassManager> MPM;

    // Explicit destruction in the correct order.
    ~LlvmPmOpaquePassManager() {
        MPM.reset();
        PB.reset();
        MAM.reset();
        CGAM.reset();
        FAM.reset();
        LAM.reset();
        SI.reset();
        PIC.reset();
    }
};

// --- Helpers ---

static LLVMContext *unwrapContext(LLVMContextRef C) {
    return reinterpret_cast<LLVMContext *>(C);
}

static Module *unwrapModule(LLVMModuleRef M) {
    return reinterpret_cast<Module *>(M);
}

static TargetMachine *unwrapTM(LLVMTargetMachineRef TM) {
    return reinterpret_cast<TargetMachine *>(TM);
}

static char *copyErrorString(const std::string &msg) {
    char *buf = static_cast<char *>(std::malloc(msg.size() + 1));
    if (buf) {
        std::memcpy(buf, msg.c_str(), msg.size() + 1);
    }
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

/// Create the common infrastructure: PIC, SI, analysis managers, PassBuilder.
/// Registers all analyses and cross-registers proxies.
static LlvmPmOpaquePassManager *createInfrastructure(
    LLVMContextRef context,
    LLVMTargetMachineRef target_machine)
{
    auto *pm = new LlvmPmOpaquePassManager();

    LLVMContext *Ctx = unwrapContext(context);
    TargetMachine *TM = target_machine ? unwrapTM(target_machine) : nullptr;

    pm->PIC = std::make_unique<PassInstrumentationCallbacks>();
    pm->SI = std::make_unique<StandardInstrumentations>(
        *Ctx, /*DebugLogging=*/false, /*VerifyEach=*/false);
    pm->SI->registerCallbacks(*pm->PIC);

    pm->LAM = std::make_unique<LoopAnalysisManager>();
    pm->FAM = std::make_unique<FunctionAnalysisManager>();
    pm->CGAM = std::make_unique<CGSCCAnalysisManager>();
    pm->MAM = std::make_unique<ModuleAnalysisManager>();

    pm->PB = std::make_unique<PassBuilder>(
        TM, PipelineTuningOptions(), std::nullopt, pm->PIC.get());

    pm->PB->registerModuleAnalyses(*pm->MAM);
    pm->PB->registerCGSCCAnalyses(*pm->CGAM);
    pm->PB->registerFunctionAnalyses(*pm->FAM);
    pm->PB->registerLoopAnalyses(*pm->LAM);
    pm->PB->crossRegisterProxies(*pm->LAM, *pm->FAM, *pm->CGAM, *pm->MAM);

    return pm;
}

// --- API implementation ---

extern "C" LlvmPmPassManagerRef llvm_pm_create_with_opt_level(
    LLVMContextRef context,
    LLVMTargetMachineRef target_machine,
    LlvmPmOptLevel level,
    char **err_msg)
{
    auto *pm = createInfrastructure(context, target_machine);
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
    LLVMContextRef context,
    LLVMTargetMachineRef target_machine,
    const char *pipeline,
    char **err_msg)
{
    auto *pm = createInfrastructure(context, target_machine);
    pm->MPM = std::make_unique<ModulePassManager>();

    if (auto Err = pm->PB->parsePassPipeline(*pm->MPM, StringRef(pipeline))) {
        std::string msg = toString(std::move(Err));
        *err_msg = copyErrorString(msg);
        delete pm;
        return nullptr;
    }

    *err_msg = nullptr;
    return pm;
}

extern "C" char *llvm_pm_run(LlvmPmPassManagerRef pm, LLVMModuleRef module) {
    Module *M = unwrapModule(module);
    pm->MPM->run(*M, *pm->MAM);
    return nullptr;
}

extern "C" void llvm_pm_dispose(LlvmPmPassManagerRef pm) {
    delete pm;
}

extern "C" void llvm_pm_dispose_message(char *msg) {
    std::free(msg);
}
