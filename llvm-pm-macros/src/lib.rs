use proc_macro::TokenStream;

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse::Parse, parse::ParseStream, Ident, ItemFn, LitStr, Token};

struct PluginArgs {
    name: String,
    version: String,
}

impl Parse for PluginArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name_ident: Ident = input.parse()?;
        if name_ident != "name" {
            return Err(syn::Error::new_spanned(
                name_ident,
                "expected `name = \"...\"`",
            ));
        }
        let _: Token![=] = input.parse()?;
        let name: LitStr = input.parse()?;
        let _: Token![,] = input.parse()?;

        let version_ident: Ident = input.parse()?;
        if version_ident != "version" {
            return Err(syn::Error::new_spanned(
                version_ident,
                "expected `version = \"...\"`",
            ));
        }
        let _: Token![=] = input.parse()?;
        let version: LitStr = input.parse()?;

        Ok(PluginArgs {
            name: name.value(),
            version: version.value(),
        })
    }
}

/// Attribute macro for defining an LLVM plugin.
///
/// This macro generates the `llvmGetPassPluginInfo` entry point required for
/// LLVM to load the plugin. The annotated function receives a
/// [`PassBuilder`](llvm_pm::plugin::PassBuilder) and should register callbacks
/// for pipeline parsing, analysis registration, or extension points.
///
/// # Parameters
///
/// - `name`: The plugin name (string literal).
/// - `version`: The plugin version (string literal).
///
/// # Warning
///
/// This macro should be used on `cdylib` crates **only**. Since it generates
/// an exported symbol, it should be used **once** per dylib.
///
/// # Example
///
/// ```ignore
/// #[llvm_pm::plugin(name = "my_plugin", version = "0.1")]
/// fn plugin_registrar(builder: &mut llvm_pm::plugin::PassBuilder) {
///     builder.add_module_pipeline_parsing_callback(|name, mpm| {
///         if name == "my-pass" {
///             mpm.add_pass(MyPass);
///             llvm_pm::plugin::PipelineParsing::Parsed
///         } else {
///             llvm_pm::plugin::PipelineParsing::NotParsed
///         }
///     });
/// }
/// ```
#[proc_macro_attribute]
pub fn plugin(attrs: TokenStream, input: TokenStream) -> TokenStream {
    match plugin_impl(attrs, input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn plugin_impl(attrs: TokenStream, input: TokenStream) -> syn::Result<TokenStream2> {
    let args: PluginArgs = syn::parse(attrs)?;
    let func: ItemFn = syn::parse(input)?;

    let registrar_name = &func.sig.ident;
    let registrar_name_sys = format_ident!("{}_sys", registrar_name);

    // Null-terminated strings for C interop
    let name_cstr = args.name + "\0";
    let version_cstr = args.version + "\0";

    Ok(quote! {
        #func

        extern "C" fn #registrar_name_sys(builder: *mut ::std::ffi::c_void) {
            // SAFETY: LLVM passes a valid PassBuilder pointer to the registrar.
            let mut builder = unsafe { ::llvm_pm::plugin::PassBuilder::from_raw(builder) };
            #registrar_name(&mut builder);
        }

        #[no_mangle]
        pub extern "C" fn llvmGetPassPluginInfo() -> ::llvm_pm::plugin::PassPluginLibraryInfo {
            ::llvm_pm::plugin::PassPluginLibraryInfo {
                api_version: ::llvm_pm::plugin::plugin_api_version(),
                plugin_name: #name_cstr .as_ptr(),
                plugin_version: #version_cstr .as_ptr(),
                plugin_registrar: #registrar_name_sys,
            }
        }
    })
}
