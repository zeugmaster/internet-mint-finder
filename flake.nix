{
  description = "Leptos SSR + Axum development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    let
      overlays = [
        (import rust-overlay)
        (final: prev: {
          nutty = final.callPackage ./nix/package.nix { };
        })
      ];
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [ "wasm32-unknown-unknown" ];
        };
      in
      {
        packages.default = pkgs.nutty;
        packages.nutty = pkgs.nutty;

        devShells.default = pkgs.mkShell {
          buildInputs = [
            # Rust toolchain with WASM target
            rustToolchain

            # Leptos build tooling
            pkgs.cargo-leptos
            pkgs.cargo-generate
            pkgs.trunk
            pkgs.wasm-bindgen-cli

            # Sass/SCSS compiler
            pkgs.dart-sass

            # WASM optimization
            pkgs.binaryen

            # Native dependencies commonly needed by Rust web projects
            pkgs.pkg-config
            pkgs.openssl

            # Useful extras
            pkgs.cargo-watch
          ];

          # Ensure native libs are found during compilation
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.openssl
          ];

          shellHook = ''
            echo "🦀 Leptos (SSR + Axum) dev environment loaded"
            echo ""
            echo "Available tools:"
            echo "  rustc        $(rustc --version)"
            echo "  cargo-leptos $(cargo-leptos --version 2>/dev/null || echo 'installed')"
            echo "  wasm-bindgen $(wasm-bindgen --version 2>/dev/null || echo 'installed')"
            echo "  wasm-opt     $(wasm-opt --version 2>/dev/null || echo 'installed')"
            echo ""
            echo "Quick start:"
            echo "  cargo leptos watch"
          '';
        };
      })
    // {
      nixosModules.default = import ./nix/modules/nutty.nix;
      nixosModules.nutty = import ./nix/modules/nutty.nix;
    };
}
