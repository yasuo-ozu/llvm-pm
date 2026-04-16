use llvm_pm::{LLVMContextRef, LLVMModuleRef, ModulePassManager, OptLevel};
use std::ffi::c_char;

// Declare the LLVM C API functions we need for testing.
// These are available because llvm-pm-sys links against LLVM.
type LLVMTypeRef = *mut std::ffi::c_void;
type LLVMValueRef = *mut std::ffi::c_void;
type LLVMBasicBlockRef = *mut std::ffi::c_void;
type LLVMBuilderRef = *mut std::ffi::c_void;

extern "C" {
    fn LLVMContextCreate() -> LLVMContextRef;
    fn LLVMContextDispose(C: LLVMContextRef);
    fn LLVMModuleCreateWithNameInContext(
        ModuleID: *const c_char,
        C: LLVMContextRef,
    ) -> LLVMModuleRef;
    fn LLVMDisposeModule(M: LLVMModuleRef);
    fn LLVMVoidTypeInContext(C: LLVMContextRef) -> LLVMTypeRef;
    fn LLVMInt32TypeInContext(C: LLVMContextRef) -> LLVMTypeRef;
    fn LLVMFunctionType(
        ReturnType: LLVMTypeRef,
        ParamTypes: *const LLVMTypeRef,
        ParamCount: u32,
        IsVarArg: i32,
    ) -> LLVMTypeRef;
    fn LLVMAddFunction(
        M: LLVMModuleRef,
        Name: *const c_char,
        FunctionTy: LLVMTypeRef,
    ) -> LLVMValueRef;
    fn LLVMAppendBasicBlockInContext(
        C: LLVMContextRef,
        Fn: LLVMValueRef,
        Name: *const c_char,
    ) -> LLVMBasicBlockRef;
    fn LLVMCreateBuilderInContext(C: LLVMContextRef) -> LLVMBuilderRef;
    fn LLVMPositionBuilderAtEnd(Builder: LLVMBuilderRef, Block: LLVMBasicBlockRef);
    fn LLVMBuildRetVoid(Builder: LLVMBuilderRef) -> LLVMValueRef;
    fn LLVMBuildRet(Builder: LLVMBuilderRef, V: LLVMValueRef) -> LLVMValueRef;
    fn LLVMBuildAdd(
        Builder: LLVMBuilderRef,
        LHS: LLVMValueRef,
        RHS: LLVMValueRef,
        Name: *const c_char,
    ) -> LLVMValueRef;
    fn LLVMGetParam(Fn: LLVMValueRef, Index: u32) -> LLVMValueRef;
    fn LLVMDisposeBuilder(Builder: LLVMBuilderRef);
}

/// Helper: create a context + module with a simple `void @test_fn()` function.
unsafe fn create_test_module() -> (LLVMContextRef, LLVMModuleRef) {
    let ctx = LLVMContextCreate();
    let module = LLVMModuleCreateWithNameInContext(b"test_module\0".as_ptr() as *const _, ctx);

    let void_ty = LLVMVoidTypeInContext(ctx);
    let fn_ty = LLVMFunctionType(void_ty, std::ptr::null(), 0, 0);
    let func = LLVMAddFunction(module, b"test_fn\0".as_ptr() as *const _, fn_ty);
    let bb = LLVMAppendBasicBlockInContext(ctx, func, b"entry\0".as_ptr() as *const _);
    let builder = LLVMCreateBuilderInContext(ctx);
    LLVMPositionBuilderAtEnd(builder, bb);
    LLVMBuildRetVoid(builder);
    LLVMDisposeBuilder(builder);

    (ctx, module)
}

/// Helper: create a context + module with `i32 @add(i32, i32)` function.
unsafe fn create_add_module() -> (LLVMContextRef, LLVMModuleRef) {
    let ctx = LLVMContextCreate();
    let module = LLVMModuleCreateWithNameInContext(b"test_add\0".as_ptr() as *const _, ctx);

    let i32_ty = LLVMInt32TypeInContext(ctx);
    let param_types = [i32_ty, i32_ty];
    let fn_ty = LLVMFunctionType(i32_ty, param_types.as_ptr(), 2, 0);
    let func = LLVMAddFunction(module, b"add\0".as_ptr() as *const _, fn_ty);
    let bb = LLVMAppendBasicBlockInContext(ctx, func, b"entry\0".as_ptr() as *const _);
    let builder = LLVMCreateBuilderInContext(ctx);
    LLVMPositionBuilderAtEnd(builder, bb);

    let a = LLVMGetParam(func, 0);
    let b_param = LLVMGetParam(func, 1);
    let sum = LLVMBuildAdd(builder, a, b_param, b"sum\0".as_ptr() as *const _);
    LLVMBuildRet(builder, sum);
    LLVMDisposeBuilder(builder);

    (ctx, module)
}

#[test]
fn test_opt_level_o2() {
    unsafe {
        let (ctx, module) = create_test_module();

        let pm = ModulePassManager::with_opt_level(ctx, None, OptLevel::O2)
            .expect("Failed to create pass manager");
        pm.run(module).expect("Failed to run passes");

        LLVMDisposeModule(module);
        LLVMContextDispose(ctx);
    }
}

#[test]
fn test_opt_level_o2_with_add() {
    unsafe {
        let (ctx, module) = create_add_module();

        let pm = ModulePassManager::with_opt_level(ctx, None, OptLevel::O2)
            .expect("Failed to create pass manager");
        pm.run(module).expect("Failed to run passes");

        LLVMDisposeModule(module);
        LLVMContextDispose(ctx);
    }
}

#[test]
fn test_pipeline_string() {
    unsafe {
        let (ctx, module) = create_add_module();

        let pm = ModulePassManager::with_pipeline(ctx, None, "instcombine,dce")
            .expect("Failed to create PM with pipeline");
        pm.run(module).expect("Failed to run passes");

        LLVMDisposeModule(module);
        LLVMContextDispose(ctx);
    }
}

#[test]
fn test_default_pipeline_string() {
    unsafe {
        let (ctx, module) = create_add_module();

        let pm = ModulePassManager::with_pipeline(ctx, None, "default<O2>")
            .expect("Failed to create PM with default<O2>");
        pm.run(module).expect("Failed to run passes");

        LLVMDisposeModule(module);
        LLVMContextDispose(ctx);
    }
}

#[test]
fn test_invalid_pipeline() {
    unsafe {
        let ctx = LLVMContextCreate();

        let result = ModulePassManager::with_pipeline(ctx, None, "this-is-not-a-real-pass");
        assert!(result.is_err(), "Expected error for invalid pipeline");
        let err = result.unwrap_err();
        assert!(
            !err.message().is_empty(),
            "Error message should not be empty"
        );

        LLVMContextDispose(ctx);
    }
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
        unsafe {
            let (ctx, module) = create_test_module();

            let pm = ModulePassManager::with_opt_level(ctx, None, level)
                .unwrap_or_else(|e| panic!("Failed to create PM for {:?}: {}", level, e));
            pm.run(module)
                .unwrap_or_else(|e| panic!("Failed to run passes for {:?}: {}", level, e));

            LLVMDisposeModule(module);
            LLVMContextDispose(ctx);
        }
    }
}
