# tools/fixture-vm/flake.nix
#
# NixOS guest image for the spar trace-topology fixture generator.
#
# This flake builds a bootable qcow2 NixOS disk image containing the full
# fixture-generation toolchain and the `gen-fixtures` binary from the spar
# workspace.  The image is used by the `fixture-vm` Rust harness
# (crates/spar-trace-topology/src/bin/fixture-vm.rs): the harness boots the
# image under QEMU/KVM, the guest runs `gen-fixtures` once as a systemd
# oneshot service, writes four fixture files to a virtio-9p share, and then
# powers off.
#
# == Approach for gen-fixtures ==
#
# We chose approach (a): build gen-fixtures via rustPlatform.buildRustPackage
# inside the flake, then include the resulting package in
# environment.systemPackages.  Rationale:
#
#   - NixOS has a non-standard dynamic-linker path (/nix/store/…/ld-linux-x86-64.so.2);
#     a glibc binary built on Ubuntu will not run there without patching.
#   - Building inside the flake avoids the need for a musl cross-compilation
#     step on the host, keeping CI simpler.
#   - The binary and its full closure are pinned by the flake.lock, giving
#     complete reproducibility.
#   - Approach (b) (musl static binary via 9p share) is simpler for rapid
#     iteration but requires a separate host build step and two separate
#     pinning points.  For this PR, approach (a) is preferred.
#
# == Guest boot behaviour ==
#
# On every boot a systemd oneshot service (`gen-fixtures.service`) runs
# gen-fixtures /fixtures, where /fixtures is the virtio-9p mount that the
# QEMU harness exports from the host.  After the service exits the system
# powers off via ExecStartPost=systemctl poweroff.  The service is
# wantedBy = ["multi-user.target"] so it runs at the end of normal boot
# without a special target.
#
# == Usage ==
#
#   nix build .#nixosConfigurations.fixture-vm.config.system.build.qcow2
#
# The resulting qcow2 is at result/nixos.qcow2 (symlink from ./result).
# The fixture-vm Rust harness wraps this in a per-run CoW overlay.
#
# == Building inside podman (CI) ==
#
# The CI workflow builds this flake inside a rootless podman container:
#
#   podman run --rm \
#     --device /dev/kvm \
#     --security-opt label=disable \
#     -v $PWD:/spar:Z \
#     docker.io/nixos/nix@sha256:<pinned-digest> \
#     sh -c 'echo "sandbox = false" >> /etc/nix/nix.conf && \
#            cd /spar/tools/fixture-vm && \
#            nix build .#nixosConfigurations.fixture-vm.config.system.build.qcow2 \
#              --extra-experimental-features "nix-command flakes"'
#
# --device /dev/kvm is passed because the NixOS qcow2 builder internally
# boots a transient builder VM.  sandbox = false drops Nix's in-container
# build sandbox (not host root — the container is still rootless); without
# it Nix's sandboxed builds fail inside the already-containerised environment.
{
  description = "NixOS KVM guest for spar trace-topology fixture generation";

  inputs = {
    # Pin nixpkgs to a stable release channel for reproducibility; the exact
    # rev lives in flake.lock.
    #
    # This pin is not only the guest OS — it is also the Rust toolchain that
    # builds gen-fixtures below, and THAT is the binding constraint. Every one
    # of spar's 23 crates inherits `edition = "2024"` from
    # `[workspace.package]`, and edition 2024 was stabilised in Rust 1.85.
    # Measured `rustc.version` per channel:
    #
    #     nixos-24.05 → 1.77.2   ✗ cannot compile this workspace
    #     nixos-24.11 → 1.82.0   ✗
    #     nixos-25.05 → 1.86.0   ✓
    #
    # The flake was pinned to 24.05 and so could never build spar at all; the
    # nightly died with "feature `edition2024` is required ... not stabilized
    # in this version of Cargo (1.77.1)" the moment the earlier blockers
    # stopped masking it (#362, #365).
    #
    # MAINTENANCE: bumping this channel is a Rust-toolchain bump. Before
    # lowering it, check `nix eval --raw
    # github:NixOS/nixpkgs/<channel>#legacyPackages.x86_64-linux.rustc.version`
    # against the workspace edition — a too-old channel fails at build time,
    # not at evaluation, so it survives every local check.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
  };

  outputs = { self, nixpkgs }: let
    system = "x86_64-linux";
    pkgs = nixpkgs.legacyPackages.${system};

    # Build gen-fixtures from the spar workspace.  The flake is located at
    # tools/fixture-vm/ so the workspace root is two levels up (../../).
    # We pass a cargoLock literal so the Nix sandbox can reproduce the
    # exact dependency closure without network access during the build.
    gen-fixtures = pkgs.rustPlatform.buildRustPackage {
      pname = "gen-fixtures";
      version = "0.10.0";

      # Source is the entire spar workspace (Cargo workspace resolver needs
      # the root Cargo.toml).  Filter out the target/ directory to keep
      # source hashes stable across partial host builds.
      src = pkgs.lib.cleanSourceWith {
        src = ../..;
        filter = path: type:
          !(pkgs.lib.hasSuffix "/target" path)
          && !(pkgs.lib.hasInfix "/.git/" path)
          && !(pkgs.lib.hasSuffix "/.git" path);
      };

      # cargoLock.lockFile points to the workspace Cargo.lock.  Nix reads
      # this to reproduce the exact crate set without running `cargo fetch`
      # inside the sandbox.
      cargoLock = {
        lockFile = ../../Cargo.lock;

        # Registry crates carry their checksum in Cargo.lock; git dependencies
        # do not, so Nix cannot vendor them without an explicit hash and fails
        # evaluation with "No hash was found while vendoring the git
        # dependency etch-0.2.0" (#365). The workspace has exactly one such
        # dependency — `etch`, from pulseengine/rivet at the rev pinned in the
        # root Cargo.toml. It is vendored even though gen-fixtures does not
        # use it: importCargoLock vendors the whole lock file, not just the
        # subtree that `cargoBuildFlags` selects.
        #
        # The value is the NAR hash of nixpkgs' `fetchgit` with its default
        # arguments (fetchSubmodules = true, leaveDotGit = false), keyed by
        # "<name>-<version>".
        #
        # MAINTENANCE: this hash pins the *rev*, so bumping the rivet rev in
        # the root Cargo.toml invalidates it and this build fails with a
        # fixed-output hash mismatch. Recompute with:
        #
        #   nix-prefetch-git --url https://github.com/pulseengine/rivet.git \
        #     --rev <new-rev> --fetch-submodules
        #
        # A nixpkgs channel bump can invalidate it too, if `fetchgit`'s
        # defaults change — and that failure appears only at build time, so it
        # survives every local evaluation. Re-derive it after any bump by
        # building with a deliberately wrong hash and reading the `got:` line;
        # a build with the *correct* hash proves nothing when the path is
        # already in the store, because a fixed-output derivation
        # short-circuits. Verified stable across 24.05 → 25.05 this way.
        outputHashes = {
          "etch-0.2.0" = "sha256-x37urQw97R/ARqvlVpXpp3tJqbvztbOiUyAGNZItlA0=";
        };
      };

      # Build only the gen-fixtures binary from spar-trace-topology.
      cargoBuildFlags = [
        "-p"
        "spar-trace-topology"
        "--bin"
        "gen-fixtures"
      ];

      # Disable tests (they require Linux netns — only valid inside the VM).
      doCheck = false;

      # Runtime toolchain executables that gen-fixtures invokes via PATH.
      # These are linked into the build environment so the resulting binary
      # can find them at runtime.
      nativeBuildInputs = [ pkgs.pkg-config ];
    };

  in {
    nixosConfigurations.fixture-vm = nixpkgs.lib.nixosSystem {
      inherit system;

      modules = [
        # ── Base NixOS configuration ──────────────────────────────────────

        ({ config, pkgs, lib, ... }: {
          # Pin the kernel to a recent stable version that includes
          # sch_taprio and CLOCK_TAI (present since Linux 4.18;
          # linuxPackages_latest tracks the latest stable).
          boot.kernelPackages = pkgs.linuxPackages_latest;

          # Enable sch_taprio and CBS as kernel modules.
          boot.kernelModules = [ "sch_taprio" "sch_cbs" ];

          # Boot directly from the qcow2's virtio disk (no PXE/UEFI menu).
          boot.loader.grub = {
            enable = true;
            device = "/dev/vda";
          };

          # Not `boot.loader.grub.timeout`: nixpkgs renamed it to the
          # bootloader-agnostic `boot.loader.timeout`, and the old spelling
          # emits a rename warning on every evaluation. It still works, but
          # the warning is noise in a log we want to read for real failures.
          boot.loader.timeout = 0;

          # Filesystem: a single ext4 root on vda.
          fileSystems."/" = {
            device = "/dev/vda1";
            fsType = "ext4";
          };

          # virtio-9p mount for the fixture output directory.  The QEMU
          # harness exports the host fixture dir with mount_tag="fixtures".
          fileSystems."/fixtures" = {
            device = "fixtures";
            fsType = "9p";
            options = [ "trans=virtio" "version=9p2000.L" "nofail" ];
          };

          # No swap.
          swapDevices = [];

          # Minimal networking — not needed for the fixture run.
          networking.useDHCP = false;

          # The fixture toolchain.
          environment.systemPackages = [
            gen-fixtures
            pkgs.iproute2        # ip netns, ip link
            pkgs.lldpd           # lldpd, lldpctl
            pkgs.linuxptp        # ptp4l, pmc
            pkgs.tcpdump         # tcpdump
            pkgs.tshark          # tshark (for CI verification step)
          ];

          # Allow gen-fixtures to create network namespaces and configure
          # taprio without an explicit sudo step — the guest is already root.
          security.wrappers = {};

          # ── gen-fixtures systemd service ─────────────────────────────────
          #
          # Runs once at boot, writes the four fixture files to /fixtures
          # (the 9p share), then powers the system off.

          systemd.services.gen-fixtures = {
            description = "spar trace-topology fixture generator";
            wantedBy = [ "multi-user.target" ];
            after = [ "network.target" "local-fs.target" ];

            serviceConfig = {
              Type = "oneshot";
              RemainAfterExit = false;
              ExecStart = "${gen-fixtures}/bin/gen-fixtures /fixtures";
              # Power off after gen-fixtures exits (success or failure).
              ExecStartPost = "${pkgs.systemd}/bin/systemctl poweroff --force";
              StandardOutput = "journal+console";
              StandardError = "journal+console";
            };
          };

          # Serial console for QEMU -serial mon:stdio — lets the CI log
          # show kernel + service output without a graphical window.
          services.getty.autologinUser = null;
          systemd.services."serial-getty@ttyS0".enable = true;

          system.stateVersion = "24.05";
        })

        # ── The qcow2 build product ───────────────────────────────────────
        #
        # `system.build.qcow2` is NOT a stock NixOS option — nothing in
        # nixpkgs defines it. The header comment at the top of this file and
        # the nightly workflow both build
        # `…fixture-vm.config.system.build.qcow2`, but until this module
        # existed that attribute appeared only in prose, so evaluation died
        # with `error: attribute 'qcow2' missing` before a single byte was
        # built (#365). nixos-generators defines its `qcow` format the same
        # way: by importing nixpkgs' own make-disk-image.nix as a build
        # product rather than depending on an option that doesn't exist.
        ({ config, lib, pkgs, modulesPath, ... }: {
          system.build.qcow2 =
            import "${toString modulesPath}/../lib/make-disk-image.nix" {
              inherit lib config pkgs;

              # `format` is the only argument that names the output file:
              # make-disk-image.nix computes `filename = "nixos." + ext`, so
              # qcow2 yields `$out/nixos.qcow2` — the exact path the
              # workflow's "Locate qcow2" step reads. The derivation `name`
              # argument does NOT rename the image; don't reach for it.
              format = "qcow2";

              # Must agree with the base module above, which puts grub on
              # /dev/vda and root on /dev/vda1. `legacy` is the one layout
              # whose rootPartition is "1" — efi is "2", hybrid is "3" — so
              # any other value here yields an image that partitions and
              # builds cleanly and then cannot find its root at boot.
              partitionTableType = "legacy";

              # Defaults to true, which copies the whole nixpkgs tree into
              # the image so nix-env/nix-build work inside the guest. This
              # guest boots once, runs one oneshot service, and powers off;
              # it never evaluates Nix. Skipping the channel costs nothing
              # we use and saves several hundred MB plus the copy.
              copyChannel = false;

              # diskSize is left at its "auto" default, which sizes the
              # image from the actual closure plus 512M. The guest writes
              # its fixtures to /fixtures — a host-backed 9p share, not the
              # disk — so there is no growth here to budget for.
            };
        })
      ];
    };
  };
}
