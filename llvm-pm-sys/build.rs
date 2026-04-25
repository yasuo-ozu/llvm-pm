use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let macos_sdk = if cfg!(target_os = "macos") {
        detect_macos_sdk_path()
    } else {
        None
    };

    // --- Locate LLVM (for include paths and cxxflags only) ---
    // LLVM library linking is handled by llvm-sys.
    let llvm = if cfg!(target_env = "msvc") {
        find_llvm_msvc()
    } else {
        find_llvm_unix()
    };

    // Emit LLVM major version as cfg
    let major: u32 = llvm
        .version
        .split('.')
        .next()
        .unwrap()
        .parse()
        .expect("Failed to parse LLVM major version");
    println!("cargo:rustc-cfg=llvm_version_major=\"{}\"", major);

    // --- Compile C++ stubs ---
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("cpp/llvm_pm.cpp")
        .include("cpp")
        .include(&llvm.include_dir);

    // Parse cxxflags for defines and additional include dirs
    for flag in llvm.cxxflags.split_whitespace() {
        if let Some(def) = flag.strip_prefix("-D") {
            if let Some((k, v)) = def.split_once('=') {
                build.define(k, Some(v));
            } else {
                build.define(def, None);
            }
        } else if let Some(inc) = flag.strip_prefix("-I") {
            build.include(inc);
        }
    }

    if cfg!(target_env = "msvc") {
        // LLVM is built without RTTI and exceptions on MSVC
        // LLVM 18+ headers require C++17 (e.g. std::optional).
        build.flag("/std:c++17");
        build.flag("/EHs-c-");
        build.flag("/GR-");
    } else {
        build.flag("-fno-exceptions");
        build.flag("-fno-rtti");
        // Use C++17 (required by LLVM 18+)
        build.flag("-std=c++17");
    }
    if let Some(ref sdk) = macos_sdk {
        build.flag("-isysroot");
        build.flag(sdk);
    }

    build.compile("llvm_pm_stubs");

    // Link C++ standard library (needed for our C++ stubs; llvm-sys doesn't handle this)
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=stdc++");
    } else if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
    }

    // --- Bindgen ---
    // Blocklist LLVM types — they are provided by llvm-sys instead.
    let mut bindings = bindgen::Builder::default()
        .header("cpp/llvm_pm.h")
        .clang_arg(format!("-I{}", llvm.include_dir))
        .allowlist_function("llvm_pm_.*")
        .allowlist_type("LlvmPm.*")
        .blocklist_type("LLVM.*")
        .generate_comments(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    if let Some(ref sdk) = macos_sdk {
        bindings = bindings.clang_arg("-isysroot").clang_arg(sdk);
    }
    let bindings = bindings.generate().expect("Failed to generate bindings");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings");

    // Rerun triggers
    println!("cargo:rerun-if-changed=cpp/llvm_pm.h");
    println!("cargo:rerun-if-changed=cpp/llvm_pm.cpp");
    println!("cargo:rerun-if-env-changed=LLVM_CONFIG");
    println!("cargo:rerun-if-env-changed=LLVM_DIR");
    println!("cargo:rerun-if-env-changed=SDKROOT");
}

struct LlvmInfo {
    version: String,
    include_dir: String,
    cxxflags: String,
}

fn find_llvm_unix() -> LlvmInfo {
    let llvm_config = env::var("LLVM_CONFIG").unwrap_or_else(|_| detect_llvm_config_unix());

    let version = run_llvm_config(&llvm_config, &["--version"]);
    let include_dir = run_llvm_config(&llvm_config, &["--includedir"]);
    let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);

    LlvmInfo {
        version,
        include_dir,
        cxxflags,
    }
}

fn find_llvm_msvc() -> LlvmInfo {
    // On MSVC, try LLVM_DIR env var first, then try llvm-config
    let llvm_dir = env::var("LLVM_DIR").ok();

    if let Some(ref dir) = llvm_dir {
        // Try llvm-config from LLVM_DIR
        let llvm_config = format!("{}\\bin\\llvm-config.exe", dir);
        if Command::new(&llvm_config).arg("--version").output().is_ok() {
            let version = run_llvm_config(&llvm_config, &["--version"]);
            let include_dir = run_llvm_config(&llvm_config, &["--includedir"]);
            let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);

            return LlvmInfo {
                version,
                include_dir,
                cxxflags,
            };
        }

        // Fallback: manual path construction
        let include_dir = format!("{}\\include", dir);
        let version = detect_llvm_version_from_dir(dir);

        return LlvmInfo {
            version,
            include_dir,
            cxxflags: String::new(),
        };
    }

    // Last resort: try llvm-config on PATH
    let llvm_config = "llvm-config".to_string();
    let version = run_llvm_config(&llvm_config, &["--version"]);
    let include_dir = run_llvm_config(&llvm_config, &["--includedir"]);
    let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);

    LlvmInfo {
        version,
        include_dir,
        cxxflags,
    }
}

/// Try versioned llvm-config names (llvm-config-22 .. llvm-config-10),
/// falling back to plain `llvm-config`.
fn detect_llvm_config_unix() -> String {
    for ver in &[
        "22", "21", "20", "19", "18", "17", "16", "15", "14", "13", "12", "11", "10",
    ] {
        let candidate = format!("llvm-config-{}", ver);
        if Command::new(&candidate).arg("--version").output().is_ok() {
            return candidate;
        }
    }
    "llvm-config".to_string()
}

fn detect_llvm_version_from_dir(dir: &str) -> String {
    // Try to read version from llvm-config.h or llvm-config/llvm-config.h
    let config_h = PathBuf::from(dir)
        .join("include")
        .join("llvm")
        .join("Config")
        .join("llvm-config.h");
    if let Ok(content) = std::fs::read_to_string(&config_h) {
        for line in content.lines() {
            if line.contains("LLVM_VERSION_MAJOR") {
                if let Some(v) = line.split_whitespace().last() {
                    return format!("{}.0.0", v);
                }
            }
        }
    }
    "18.0.0".to_string()
}

fn run_llvm_config(llvm_config: &str, args: &[&str]) -> String {
    let output = Command::new(llvm_config)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("Failed to run `{} {}`: {}", llvm_config, args.join(" "), e));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("`{} {}` failed: {}", llvm_config, args.join(" "), stderr);
    }
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn detect_macos_sdk_path() -> Option<String> {
    if let Ok(sdkroot) = env::var("SDKROOT") {
        if !sdkroot.is_empty() {
            return Some(sdkroot);
        }
    }
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sdk = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sdk.is_empty() {
        None
    } else {
        Some(sdk)
    }
}
