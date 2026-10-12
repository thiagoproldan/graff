{
  description = "graff -- a code map for Claude Code";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Read rather than repeated, as in ekko's flake: a version written in
        # two places is one that eventually disagrees with itself.
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;

          # What the build reads and nothing else, so that a change to the
          # evals or the docs does not rebuild the binary. The plugin's
          # manifest is read by a test of the hook's tool names.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests
              ./plugin/.claude-plugin/plugin.json
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;

          # The index's tests make git repositories.
          nativeCheckInputs = [ pkgs.git ];

          meta = with pkgs.lib; {
            inherit (cargoToml.package) description;
            homepage = cargoToml.package.repository;
            license = licenses.mit;
            mainProgram = "graff";
          };
        };

        # The Claude Code plugin: the MCP server and the SessionStart hook,
        # generated from plugin/ with the binary pinned by store path, as
        # ekko's packages.plugin is, so the plugin can never drive another
        # revision of graff than the one it was built with. Every command in
        # plugin.json starts with `graff`, which a test holds it to, and each
        # is pinned.
        packages.plugin =
          let
            graff = self.packages.${system}.default;
            manifest = builtins.fromJSON (builtins.readFile ./plugin/.claude-plugin/plugin.json);
            pin = hook: hook // { command = "${graff}/bin/" + hook.command; };
            pinned = manifest // {
              inherit (cargoToml.package) version;
              mcpServers.graff = manifest.mcpServers.graff // { command = "${graff}/bin/graff"; };
              hooks = builtins.mapAttrs (
                _event: groups: map (group: group // { hooks = map pin group.hooks; }) groups
              ) manifest.hooks;
            };
          in
          pkgs.runCommand "graff-plugin" { } ''
            mkdir -p $out/.claude-plugin
            cp ${pkgs.writeText "plugin.json" (builtins.toJSON pinned)} $out/.claude-plugin/plugin.json
          '';

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            python3 # the evals under evals/
          ];

          shellHook = ''
            export RUST_SRC_PATH="${pkgs.rustPlatform.rustLibSrc}"
          '';
        };
      });
}
