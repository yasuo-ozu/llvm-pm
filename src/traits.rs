//! This module defines traits to define Pass or Analysis.

use super::{
    inkwell, AnalysisKey, CgsccAnalysisManager, FunctionAnalysisManager, LoopAnalysisManager,
    ModuleAnalysisManager,
};
pub use llvm_pm_sys::{LLVMBasicBlockRef, LLVMModuleRef, LLVMValueRef};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreservedAnalyses {
    All,
    None,
}

pub trait LlvmModulePass {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses;
}

pub trait LlvmFunctionPass {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses;
}

pub trait LlvmModuleAnalysis {
    type Result;
    fn run_analysis(
        &self,
        module: &inkwell::module::Module<'_>,
        manager: &ModuleAnalysisManager,
    ) -> Self::Result;
    fn id() -> AnalysisKey;

    fn into_pass(self) -> ModuleAnalysisPassAdapter<Self>
    where
        Self: Sized,
    {
        ModuleAnalysisPassAdapter(self)
    }
}

pub trait LlvmFunctionAnalysis {
    type Result;
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        manager: &FunctionAnalysisManager,
    ) -> Self::Result;
    fn id() -> AnalysisKey;

    fn into_pass(self) -> FunctionAnalysisPassAdapter<Self>
    where
        Self: Sized,
    {
        FunctionAnalysisPassAdapter(self)
    }
}

/// Trait for custom CGSCC-level transformation passes.
pub trait LlvmCgsccPass {
    /// Entrypoint for the pass.
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses;
}

/// Trait for custom loop-level transformation passes.
pub trait LlvmLoopPass {
    /// Entrypoint for the pass.
    fn run_pass(
        &self,
        loop_header: LLVMBasicBlockRef,
        manager: &LoopAnalysisManager,
    ) -> PreservedAnalyses;
}

/// Trait for custom CGSCC-level analysis passes.
pub trait LlvmCgsccAnalysis {
    /// Result of the analysis.
    type Result;

    /// Entrypoint for the analysis.
    fn run_analysis(
        &self,
        function: &inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> Self::Result;

    /// Identifier for the analysis type.
    fn id() -> AnalysisKey;

    fn into_pass(self) -> CgsccAnalysisPassAdapter<Self>
    where
        Self: Sized,
    {
        CgsccAnalysisPassAdapter(self)
    }
}

/// Trait for custom loop-level analysis passes.
pub trait LlvmLoopAnalysis {
    /// Result of the analysis.
    type Result;

    /// Entrypoint for the analysis.
    fn run_analysis(
        &self,
        loop_header: LLVMBasicBlockRef,
        manager: &LoopAnalysisManager,
    ) -> Self::Result;

    /// Identifier for the analysis type.
    fn id() -> AnalysisKey;

    fn into_pass(self) -> LoopAnalysisPassAdapter<Self>
    where
        Self: Sized,
    {
        LoopAnalysisPassAdapter(self)
    }
}

/// See [`LlvmModuleAnalysis::into_pass()`]
pub struct ModuleAnalysisPassAdapter<T: LlvmModuleAnalysis>(T);

impl<T: LlvmModuleAnalysis> LlvmModulePass for ModuleAnalysisPassAdapter<T> {
    fn run_pass(
        &self,
        module: &mut inkwell::module::Module<'_>,
        manager: &ModuleAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = self.0.run_analysis(module, manager);
        PreservedAnalyses::All
    }
}

/// See [`LlvmFunctionAnalysis::into_pass()`]
pub struct FunctionAnalysisPassAdapter<T: LlvmFunctionAnalysis>(T);

impl<T: LlvmFunctionAnalysis> LlvmFunctionPass for FunctionAnalysisPassAdapter<T> {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &FunctionAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = self.0.run_analysis(function, manager);
        PreservedAnalyses::All
    }
}

/// See [`LlvmCgsccAnalysis::into_pass()`]
pub struct CgsccAnalysisPassAdapter<T: LlvmCgsccAnalysis>(T);

impl<T: LlvmCgsccAnalysis> LlvmCgsccPass for CgsccAnalysisPassAdapter<T> {
    fn run_pass(
        &self,
        function: &mut inkwell::values::FunctionValue<'_>,
        manager: &CgsccAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = self.0.run_analysis(function, manager);
        PreservedAnalyses::All
    }
}

/// See [`LlvmLoopAnalysis::into_pass()`]
pub struct LoopAnalysisPassAdapter<T: LlvmLoopAnalysis>(T);

impl<T: LlvmLoopAnalysis> LlvmLoopPass for LoopAnalysisPassAdapter<T> {
    fn run_pass(
        &self,
        loop_header: LLVMBasicBlockRef,
        manager: &LoopAnalysisManager,
    ) -> PreservedAnalyses {
        let _ = self.0.run_analysis(loop_header, manager);
        PreservedAnalyses::All
    }
}
