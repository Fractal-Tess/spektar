{
  description = "A basic Rust development environment with direnv";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSupportedSystem =
        f:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          f {
            inherit system;
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            };
          }
        );
    in
    {
      devShells = forEachSupportedSystem (
        { system, pkgs }:
        {
          default =
            let
              # Select the rust toolchain you want to use
              rustToolchain = pkgs.rust-bin.stable.latest.default.override {
                extensions = [
                  "rust-src"
                  "rust-analyzer"
                  "clippy"
                  "rustfmt"
                ];
              };
            in
            pkgs.mkShell {
              buildInputs = with pkgs; [
                # Rust toolchain
                rustToolchain

                # Development tools
                rust-analyzer
                cargo-edit
                cargo-watch
                cargo-audit

                # System dependencies
                pkg-config
                openssl
                openssl.dev

                # Audio dependencies
                alsa-lib
                pulseaudio
                wayland
                libxkbcommon
                vulkan-loader
                xorg.libxcb
                xorg.libX11
                xorg.libXcursor
                xorg.libXrandr
                xorg.libXi
                libGL

                # Other useful tools
                direnv
                nix-direnv
              ];

              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
                pkgs.alsa-lib
                pkgs.pulseaudio
                pkgs.wayland
                pkgs.libxkbcommon
                pkgs.vulkan-loader
                pkgs.xorg.libxcb
                pkgs.xorg.libX11
                pkgs.xorg.libXcursor
                pkgs.xorg.libXrandr
                pkgs.xorg.libXi
                pkgs.libGL
              ];

              shellHook = ''
                echo "Rust development environment loaded!"
                echo "Using Rust toolchain: $(rustc --version)"
                echo ""
                echo "To reload the environment after flake changes:"
                echo "  exit the shell and run 'direnv allow' followed by 'cd .' or 'direnv reload'"
                echo ""
              '';

              # Set environment variables if needed
              RUST_BACKTRACE = 1;
            };
        }
      );
    };
}
