{
  description = "Loggle development environment and package";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = nixpkgs.lib.genAttrs systems;

      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          lib = pkgs.lib;
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          source = lib.cleanSourceWith {
            src = ./.;
            filter =
              path: _type:
              let
                name = baseNameOf path;
              in
              !(builtins.elem name [
                ".git"
                ".jj"
                "result"
                "target"
              ]);
          };
        in
        rec {
          default = loggle;

          loggle = pkgs.rustPlatform.buildRustPackage {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;

            src = source;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

            meta = {
              description = cargoToml.package.description;
              homepage = cargoToml.package.homepage;
              license = lib.licenses.mit;
              mainProgram = "loggle";
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/loggle";
          meta.description = "Run Loggle";
        };
      });

      checks = forAllSystems (system: {
        default = self.packages.${system}.default;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          lib = pkgs.lib;
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                actionlint
                cargo
                cargo-dist
                clippy
                gh
                jq
                nixfmt
                pkg-config
                rust-analyzer
                rustc
                rustfmt
                stdenv.cc
              ]
              ++ lib.optionals stdenv.isDarwin [ libiconv ];

            RUST_BACKTRACE = "1";
            CARGO_TARGET_DIR = "target/nix";
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);
    };
}
