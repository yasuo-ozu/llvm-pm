use super::*;
use crate::traits::{
    LlvmFunctionAnalysis, LlvmFunctionPass, LlvmModuleAnalysis, LlvmModulePass, PreservedAnalyses,
};

impl<T> LlvmModulePass for T
where
    T: llvm_plugin::LlvmModulePass,
{
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        // SAFETY: `manager.inner` and `from_analysis_id` originate from LLVM callback context
        // and are valid for the duration of this pass invocation.
        let manager = ManuallyDrop::new(unsafe {
            llvm_plugin::ModuleAnalysisManager::from_raw(manager.inner, manager.from_analysis_id)
        });
        match llvm_plugin::LlvmModulePass::run_pass(self, module, &manager) {
            llvm_plugin::PreservedAnalyses::All => PreservedAnalyses::All,
            llvm_plugin::PreservedAnalyses::None => PreservedAnalyses::None,
        }
    }
}

impl<T> LlvmFunctionPass for T
where
    T: llvm_plugin::LlvmFunctionPass,
{
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        // SAFETY: `manager.inner` and `from_analysis_id` originate from LLVM callback context
        // and are valid for the duration of this pass invocation.
        let manager = ManuallyDrop::new(unsafe {
            llvm_plugin::FunctionAnalysisManager::from_raw(manager.inner, manager.from_analysis_id)
        });
        match llvm_plugin::LlvmFunctionPass::run_pass(self, function, &manager) {
            llvm_plugin::PreservedAnalyses::All => PreservedAnalyses::All,
            llvm_plugin::PreservedAnalyses::None => PreservedAnalyses::None,
        }
    }
}

impl<T> LlvmModuleAnalysis for T
where
    T: llvm_plugin::LlvmModuleAnalysis,
{
    type Result = T::Result;
    fn run_analysis(
        &self,
        module: &inkwell::module::Module<'_>,
        manager: &ModuleAnalysisManager,
    ) -> Self::Result {
        // SAFETY: `manager.inner` and `from_analysis_id` originate from LLVM callback context
        // and are valid for the duration of this analysis invocation.
        let manager = ManuallyDrop::new(unsafe {
            llvm_plugin::ModuleAnalysisManager::from_raw(manager.inner, manager.from_analysis_id)
        });
        llvm_plugin::LlvmModuleAnalysis::run_analysis(self, module, &manager)
    }
    fn id() -> AnalysisKey {
        T::id()
    }
}

impl<T> LlvmFunctionAnalysis for T
where
    T: llvm_plugin::LlvmFunctionAnalysis,
{
    type Result = T::Result;
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        manager: &FunctionAnalysisManager,
    ) -> Self::Result {
        // SAFETY: `manager.inner` and `from_analysis_id` originate from LLVM callback context
        // and are valid for the duration of this analysis invocation.
        let manager = ManuallyDrop::new(unsafe {
            llvm_plugin::FunctionAnalysisManager::from_raw(manager.inner, manager.from_analysis_id)
        });
        llvm_plugin::LlvmFunctionAnalysis::run_analysis(self, function, &manager)
    }
    fn id() -> AnalysisKey {
        T::id()
    }
}
