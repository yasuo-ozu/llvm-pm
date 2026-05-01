//! Integration tests for the `#[plugin]` attribute macro.
//!
//! This is a separate test binary because the macro generates a
//! `#[no_mangle] pub extern "C" fn llvmGetPassPluginInfo()` symbol
//! that can only appear once per binary.

#![cfg(feature = "plugin-macros")]

use llvm_pm::plugin::{PassBuilder, PipelineParsing};
use llvm_pm::traits::{LlvmFunctionPass, LlvmModulePass, PreservedAnalyses};
use std::sync::atomic::{AtomicU32, Ordering};

static REGISTRAR_CALLED: AtomicU32 = AtomicU32::new(0);

/// A plugin registrar that registers module and function pipeline parsing callbacks.
#[llvm_pm::plugin(name = "test-plugin", version = "1.2.3")]
fn my_plugin_registrar(builder: &mut PassBuilder) {
    REGISTRAR_CALLED.fetch_add(1, Ordering::SeqCst);

    builder.add_module_pipeline_parsing_callback(|name, mpm| {
        if name == "my-module-pass" {
            struct TestModulePass;
            impl LlvmModulePass for TestModulePass {
                fn run_pass(
                    &self,
                    _module: &mut llvm_pm::inkwell::module::Module<'_>,
                    _manager: &llvm_pm::ModuleAnalysisManager,
                ) -> PreservedAnalyses {
                    PreservedAnalyses::All
                }
            }
            mpm.add_pass(TestModulePass);
            PipelineParsing::Parsed
        } else {
            PipelineParsing::NotParsed
        }
    });

    builder.add_function_pipeline_parsing_callback(|name, fpm| {
        if name == "my-function-pass" {
            struct TestFunctionPass;
            impl LlvmFunctionPass for TestFunctionPass {
                fn run_pass(
                    &self,
                    _function: &mut llvm_pm::inkwell::values::FunctionValue<'_>,
                    _manager: &llvm_pm::FunctionAnalysisManager,
                ) -> PreservedAnalyses {
                    PreservedAnalyses::All
                }
            }
            fpm.add_pass(TestFunctionPass);
            PipelineParsing::Parsed
        } else {
            PipelineParsing::NotParsed
        }
    });
}

// =========================================================================
// Entry point tests
// =========================================================================

#[test]
fn test_macro_generates_correct_api_version() {
    let info = llvmGetPassPluginInfo();
    assert_eq!(info.api_version, llvm_pm::plugin::plugin_api_version());
    assert_eq!(info.api_version, 1);
}

#[test]
fn test_macro_generates_correct_plugin_name() {
    let info = llvmGetPassPluginInfo();
    let name = unsafe { std::ffi::CStr::from_ptr(info.plugin_name as *const std::ffi::c_char) };
    assert_eq!(name.to_str().unwrap(), "test-plugin");
}

#[test]
fn test_macro_generates_correct_plugin_version() {
    let info = llvmGetPassPluginInfo();
    let version =
        unsafe { std::ffi::CStr::from_ptr(info.plugin_version as *const std::ffi::c_char) };
    assert_eq!(version.to_str().unwrap(), "1.2.3");
}

#[test]
fn test_macro_registrar_points_to_generated_wrapper() {
    let info = llvmGetPassPluginInfo();
    let expected_fn: extern "C" fn(*mut std::ffi::c_void) = my_plugin_registrar_sys;
    assert_eq!(info.plugin_registrar as usize, expected_fn as usize);
}

#[test]
fn test_macro_entry_point_is_stable_across_calls() {
    let info1 = llvmGetPassPluginInfo();
    let info2 = llvmGetPassPluginInfo();
    assert_eq!(info1.api_version, info2.api_version);
    assert_eq!(info1.plugin_name, info2.plugin_name);
    assert_eq!(info1.plugin_version, info2.plugin_version);
    assert_eq!(
        info1.plugin_registrar as usize,
        info2.plugin_registrar as usize
    );
}

#[test]
fn test_macro_name_is_null_terminated() {
    let info = llvmGetPassPluginInfo();
    // Verify the pointer is to a valid null-terminated string by reading
    // until the first null byte. The expected length is "test-plugin".len() = 11.
    let name = unsafe { std::ffi::CStr::from_ptr(info.plugin_name as *const std::ffi::c_char) };
    assert_eq!(name.to_bytes().len(), 11);
}

#[test]
fn test_macro_version_is_null_terminated() {
    let info = llvmGetPassPluginInfo();
    let version =
        unsafe { std::ffi::CStr::from_ptr(info.plugin_version as *const std::ffi::c_char) };
    assert_eq!(version.to_bytes().len(), 5); // "1.2.3".len()
}
