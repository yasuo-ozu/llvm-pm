{
  description = "dev shell with llvm";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          llvmPkgs = pkgs.llvmPackages_18;
          gccVersion = (builtins.parseDrvName pkgs.stdenv.cc.cc.name).version;
          gccIncludes = pkgs.stdenv.cc.cc;
          targetTriple = pkgs.stdenv.hostPlatform.config;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              llvmPkgs.libllvm
              llvmPkgs.libclang
              llvmPkgs.clang
              rustc
              cargo
              rustfmt
              clippy
              gnumake
              # System libraries required by llvm-sys
              libxml2
              libffi
              ncurses
              zlib
            ];
            LLVM_CONFIG = "${llvmPkgs.libllvm.dev}/bin/llvm-config";
            LIBCLANG_PATH = "${llvmPkgs.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = builtins.toString [
              "-isystem ${gccIncludes}/include/c++/${gccVersion}"
              "-isystem ${gccIncludes}/include/c++/${gccVersion}/${targetTriple}"
              "-isystem ${pkgs.glibc.dev}/include"
              "-isystem ${gccIncludes}/lib/gcc/${targetTriple}/${gccVersion}/include"
            ];
            RUSTFLAGS = "-L native=${llvmPkgs.libllvm.lib}/lib -C link-arg=-Wl,-rpath,${llvmPkgs.libllvm.lib}/lib";
          };
        }
      );
    };
}
