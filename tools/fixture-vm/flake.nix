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
    # Pin nixpkgs to a specific revision for reproducibility.
    # nixos-24.05 is a stable release channel; update the rev + sha256
    # when a newer release is needed.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
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
      cargoLock.lockFile = ../../Cargo.lock;

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
            timeout = 0;
          };

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
      ];
    };
  };
}
