use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // --- Locate LLVM ---
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
    println!("cargo::rustc-cfg=llvm_version_major=\"{}\"", major);

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
        build.flag("/EHs-c-");
        build.flag("/GR-");
    } else {
        build.flag("-fno-exceptions");
        build.flag("-fno-rtti");
        // Use C++17 (required by LLVM 18+)
        build.flag("-std=c++17");
    }

    build.compile("llvm_pm_stubs");

    // --- Link LLVM ---
    println!("cargo::rustc-link-search=native={}", llvm.lib_dir);

    // Parse --libs output
    for lib in llvm.libs.split_whitespace() {
        if let Some(name) = lib.strip_prefix("-l") {
            println!("cargo::rustc-link-lib={}", name);
        } else if !lib.is_empty() {
            // On some systems, llvm-config returns bare library names
            let name = lib
                .strip_prefix("lib")
                .unwrap_or(lib)
                .strip_suffix(".a")
                .or_else(|| lib.strip_suffix(".so"))
                .or_else(|| lib.strip_suffix(".dylib"))
                .unwrap_or(lib);
            println!("cargo::rustc-link-lib={}", name);
        }
    }

    // Parse --system-libs
    for lib in llvm.system_libs.split_whitespace() {
        if let Some(name) = lib.strip_prefix("-l") {
            println!("cargo::rustc-link-lib={}", name);
        }
    }

    // Parse --ldflags for additional library search paths
    for flag in llvm.ldflags.split_whitespace() {
        if let Some(path) = flag.strip_prefix("-L") {
            println!("cargo::rustc-link-search=native={}", path);
        }
    }

    // Link C++ standard library
    if cfg!(target_os = "linux") {
        println!("cargo::rustc-link-lib=stdc++");
    } else if cfg!(target_os = "macos") {
        println!("cargo::rustc-link-lib=c++");
    }

    // --- Bindgen ---
    let bindings = bindgen::Builder::default()
        .header("cpp/llvm_pm.h")
        .clang_arg(format!("-I{}", llvm.include_dir))
        .allowlist_function("llvm_pm_.*")
        .allowlist_type("LlvmPm.*")
        .generate_comments(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Failed to generate bindings");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings");

    // Rerun triggers
    println!("cargo::rerun-if-changed=cpp/llvm_pm.h");
    println!("cargo::rerun-if-changed=cpp/llvm_pm.cpp");
    println!("cargo::rerun-if-env-changed=LLVM_CONFIG");
    println!("cargo::rerun-if-env-changed=LLVM_DIR");
}

struct LlvmInfo {
    version: String,
    include_dir: String,
    lib_dir: String,
    cxxflags: String,
    ldflags: String,
    libs: String,
    system_libs: String,
}

fn find_llvm_unix() -> LlvmInfo {
    let llvm_config = env::var("LLVM_CONFIG").unwrap_or_else(|_| "llvm-config".to_string());

    let version = run_llvm_config(&llvm_config, &["--version"]);
    let include_dir = run_llvm_config(&llvm_config, &["--includedir"]);
    let lib_dir = run_llvm_config(&llvm_config, &["--libdir"]);
    let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);
    let ldflags = run_llvm_config(&llvm_config, &["--ldflags"]);

    // Try shared linking first, fall back to static
    let shared_mode = run_llvm_config(&llvm_config, &["--shared-mode"]);
    let libs = if shared_mode.contains("shared") {
        run_llvm_config(&llvm_config, &["--link-shared", "--libs"])
    } else {
        run_llvm_config(
            &llvm_config,
            &["--libs", "passes", "core", "support", "target", "analysis"],
        )
    };

    let system_libs = run_llvm_config(&llvm_config, &["--system-libs"]);

    LlvmInfo {
        version,
        include_dir,
        lib_dir,
        cxxflags,
        ldflags,
        libs,
        system_libs,
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
            let lib_dir = run_llvm_config(&llvm_config, &["--libdir"]);
            let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);
            let ldflags = run_llvm_config(&llvm_config, &["--ldflags"]);
            let libs = run_llvm_config(&llvm_config, &["--libs"]);
            let system_libs = run_llvm_config(&llvm_config, &["--system-libs"]);

            return LlvmInfo {
                version,
                include_dir,
                lib_dir,
                cxxflags,
                ldflags,
                libs,
                system_libs,
            };
        }

        // Fallback: manual path construction
        let include_dir = format!("{}\\include", dir);
        let lib_dir = format!("{}\\lib", dir);

        // Try to detect version from the directory
        let version = detect_llvm_version_from_dir(dir);

        return LlvmInfo {
            version,
            include_dir,
            lib_dir,
            cxxflags: String::new(),
            ldflags: String::new(),
            libs: "-lLLVM".to_string(),
            system_libs: String::new(),
        };
    }

    // Last resort: try llvm-config on PATH
    let llvm_config = "llvm-config".to_string();
    let version = run_llvm_config(&llvm_config, &["--version"]);
    let include_dir = run_llvm_config(&llvm_config, &["--includedir"]);
    let lib_dir = run_llvm_config(&llvm_config, &["--libdir"]);
    let cxxflags = run_llvm_config(&llvm_config, &["--cxxflags"]);
    let ldflags = run_llvm_config(&llvm_config, &["--ldflags"]);
    let libs = run_llvm_config(&llvm_config, &["--libs"]);
    let system_libs = run_llvm_config(&llvm_config, &["--system-libs"]);

    LlvmInfo {
        version,
        include_dir,
        lib_dir,
        cxxflags,
        ldflags,
        libs,
        system_libs,
    }
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
