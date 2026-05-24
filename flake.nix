{
  description = "scrollmux — fixed-width horizontal PTY strip";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        lib = pkgs.lib;
      in
      {
        devShells.default = pkgs.mkShell {
          packages = (with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            coreutils
            gawk
            gnugrep
            gnused
            hyperfine
            neovim
            time
            util-linux
          ]) ++ lib.optionals pkgs.stdenv.isLinux (with pkgs; [
            strace
            linuxPackages.perf
          ]);
          RUST_BACKTRACE = "1";
        };
      });
}
