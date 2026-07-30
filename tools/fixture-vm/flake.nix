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
    # builds gen-fixtures below, and THAT is the binding constraint.
    #
    # The constraint is NOT the workspace edition. It is
    #
    #     max(rust-version) over the entire RESOLVED DEPENDENCY CLOSURE
    #
    # which is an emergent property of Cargo.lock, not a property of this
    # repo: the workspace declares no `rust-version` and carries no
    # rust-toolchain.toml, so nothing here states the floor and nothing here
    # pins it. Re-derive it, never retype it:
    #
    #     cargo metadata --format-version 1 --locked \
    #       | jq -r '.packages[] | select(.rust_version) | .rust_version' \
    #       | sort -V | tail -1
    #
    # Measured 2026-07-30: 152 of 235 packages declare `rust-version`;
    # the max is 1.89 (smol_str 0.3.6), then 1.87.0 (wasip2, wit-bindgen).
    # Measured `rustc.version` per channel:
    #
    #     nixos-24.05 → 1.77.2   ✗ edition 2024 not stabilised (needs 1.85)
    #     nixos-24.11 → 1.82.0   ✗ likewise
    #     nixos-25.05 → 1.86.0   ✗ clears the edition, FAILS the closure (1.89)
    #     nixos-25.11 → 1.91.1   ✓ clears both
    #
    # HOW THIS WENT WRONG, so it is not repeated: the pin was moved 24.05 →
    # 25.05 against the edition floor alone, and 25.05 was recorded here as
    # "✓ cannot compile → can compile". That was false. Edition 2024 needing
    # ≥1.85 is a NECESSARY condition that was mistaken for THE condition, and
    # because the probe genuinely ran and genuinely returned 1.86.0, the
    # check felt like verification while silently scoping the claim to the
    # one variable that had been thought of. Cargo caught it — inside the
    # derivation, at build time, one CI dispatch later:
    #
    #     error: rustc 1.86.0 is not supported by the following package:
    #       smol_str@0.3.6 requires rustc 1.89
    #
    # MAINTENANCE: bumping this channel is a Rust-toolchain bump, and the
    # floor it must clear MOVES ON EVERY `cargo update` — a dependency can
    # raise the closure max without a single line of this repo changing.
    # Nothing gates that today; it surfaces only as a red nightly here, which
    # is why this comment carries the derivation command instead of a number
    # to trust. Before changing this line, run the command above and compare
    # against `nix eval --raw
    # github:NixOS/nixpkgs/<channel>#legacyPackages.x86_64-linux.rustc.version`.
    # Both a too-old channel and a raised closure floor fail at BUILD time,
    # not at evaluation, so they survive every local check this flake permits.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
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

      # Keep this build from poisoning /homeless-shelter for every LATER build.
      #
      # Nix runs builders with HOME=/homeless-shelter, and when the sandbox is
      # off it refuses to START any build if that path exists — an existing home
      # directory would let builders read state from outside the derivation. The
      # check lives in local-derivation-goal.cc behind `!useChroot`, so it is a
      # direct consequence of the `sandbox = false` this container relies on
      # (see NIX_CONF_EXTRA in .github/workflows/trace-fixtures.yml).
      #
      # Run 30575158798 died on exactly that, one derivation AFTER this one:
      #
      #   building '/nix/store/…-gen-fixtures-0.10.0.drv'...      20:11:21
      #   error: home directory '/homeless-shelter' exists;       20:16:42
      #   please remove it to assure purity of builds without sandboxing
      #
      # WHAT IS MEASURED vs INFERRED. Measured: the error fired 5m21s after this
      # derivation started, with no other `building '…'` line in between, and it
      # is thrown when a build STARTS — so the directory was created during this
      # 5m21s window. Inferred: that `cmake` did it, via its user package
      # registry at $HOME/.cmake/packages — cmake is on this build's PATH for
      # highs-sys (see below) and is the usual culprit. The fix does not depend
      # on that inference being right: redirecting HOME covers whichever tool in
      # this derivation writes to it.
      #
      # A `rm -rf /homeless-shelter` in the workflow would NOT fix this. The
      # directory is created mid-graph, so removing it before `nix build` leaves
      # the failing window untouched. Enabling the sandbox would also fix it —
      # and is arguably the right answer — but it is a larger change: it would
      # make build-principal-probe.nix's out-of-store `builder = "/bin/sh"`
      # illegal, and
      # whether nested user namespaces work under rootless podman here is
      # untested. Deliberately not bundled into a run that is already testing
      # two other fixes.
      preConfigure = ''
        export HOME="$TMPDIR"
      '';

      # Build-time tools. `cmake` is here for a non-obvious reason: it is not
      # used by gen-fixtures, it is used by a build script five levels down.
      #
      #   spar-trace-topology -> spar-network -> good_lp -> highs -> highs-sys
      #
      # highs-sys compiles the HiGHS C++ LP solver from vendored source via
      # the `cmake` crate, and without cmake on PATH its build script panics
      # with `failed to execute command: No such file or directory` /
      # `is cmake not installed?`.
      #
      # Note this is NOT fixed by narrowing cargoBuildFlags. `--bin
      # gen-fixtures` selects what gets LINKED; cargo still compiles the
      # dependency graph of the whole `-p spar-trace-topology` package, and
      # gen-fixtures never calls the solver. Same shape as the etch vendoring
      # below: the lock file, not the binary, decides what must build.
      #
      # `dontUseCmakeConfigure` is load-bearing, not defensive. Putting cmake
      # in nativeBuildInputs makes nixpkgs' cmake setup-hook install
      # cmakeConfigurePhase as the package's configurePhase (setup-hook.sh:145
      # guards on exactly this variable), which would try to cmake the Rust
      # workspace root — it has no CMakeLists.txt. We want cmake on PATH for
      # the build script and nothing else.
      #
      # `bindgenHook` is here for the SAME build script, found by reading the
      # dependency graph rather than by waiting for the next red dispatch:
      # highs-sys also build-depends on bindgen 0.71, which reaches libclang
      # through clang-sys -> libloading, i.e. it dlopen()s libclang.so at
      # build-script runtime. Nothing in the sandbox provides that, and the
      # failure ("Unable to find libclang") would have surfaced only after the
      # cmake fix cleared the way — the same one-blocker-at-a-time pattern that
      # made this workflow take 60 red runs to diagnose. The hook exports
      # LIBCLANG_PATH and BINDGEN_EXTRA_CLANG_ARGS; nothing else here sets them.
      #
      # This is a PREDICTION, not a verification. Reading the graph says the
      # dependency exists; only an x86_64-linux build says the fix is
      # sufficient. clang-sys and highs-sys are the only two -sys crates in the
      # graph, so this should be the last of this species — "should be" doing
      # real work in that sentence.
      nativeBuildInputs = [
        pkgs.cmake
        pkgs.pkg-config
        pkgs.rustPlatform.bindgenHook
      ];
      dontUseCmakeConfigure = true;
    };

  in {
    nixosConfigurations.fixture-vm = nixpkgs.lib.nixosSystem {
      inherit system;

      modules = [
        # ── Base NixOS configuration ──────────────────────────────────────

        ({ config, pkgs, lib, modulesPath, ... }: {
          # Without this the initrd has no virtio drivers, so /dev/vda1 —
          # the root device declared 40 lines below — never appears, and
          # stage 1 dies with "Timed out waiting for device /dev/vda1".
          #
          # `nixpkgs.lib.nixosSystem` imports neither profiles/base.nix nor
          # this profile; the virtio module set that every NixOS-under-QEMU
          # user takes for granted arrives HERE and nowhere else. Measured on
          # the config as it stood, `config.boot.initrd.availableKernelModules`
          # was ahci / ata_piix / nvme / sd_mod / sr_mod / usbhid — IDE, SATA,
          # NVMe and USB keyboards, and not one virtio_*.
          #
          # WHY IT SURVIVED SO LONG. Nothing cross-checks `fileSystems."/"`
          # against the drivers the initrd can load, so the image evaluated,
          # built and PARTLY booted: SeaBIOS and GRUB reach the disk through
          # BIOS int 13h, which works for any controller, so run 30580839772
          # got all the way to "<<< NixOS Stage 1 >>>" before failing. A disk
          # that boots is not a disk the kernel can mount.
          #
          # It also carries 9p/9pnet_virtio, which the /fixtures share needs —
          # and that mount is `nofail`, so without them gen-fixtures would
          # have written its output into an ordinary directory inside the
          # guest and the host would have collected nothing, with every step
          # green. Two failure modes, one import.
          imports = [ "${toString modulesPath}/profiles/qemu-guest.nix" ];

          # Pin the kernel to a recent stable version that includes
          # sch_taprio and CLOCK_TAI (present since Linux 4.18;
          # linuxPackages_latest tracks the latest stable).
          boot.kernelPackages = pkgs.linuxPackages_latest;

          # Enable sch_taprio and CBS as kernel modules.
          boot.kernelModules = [ "sch_taprio" "sch_cbs" ];

          # Put the kernel console on the serial port QEMU is actually
          # reading. Without this the guest is silent from the moment the
          # kernel starts, and every failure inside it looks identical.
          #
          # The harness runs `-nographic -serial mon:stdio` and inherits
          # stdout, which is correct and was never the problem. The problem
          # is that only *firmware* output survived it: under `-nographic`
          # SeaBIOS redirects the BIOS text console onto the serial port,
          # and GRUB draws via BIOS calls, so GRUB rode that redirect for
          # free. Linux does not use BIOS calls — at handoff it switches to
          # whatever `console=` names, which defaults to `tty0`, the VGA
          # framebuffer `-nographic` has disconnected. So the kernel,
          # systemd, and gen-fixtures all logged into a void.
          #
          # That is exactly what the log of run 30569595760 shows: SeaBIOS,
          # iPXE and "Welcome to GRUB!", then 900 seconds of nothing and the
          # harness timeout. The silence was not the guest hanging early; it
          # was the last line we were able to see.
          #
          # Order is load-bearing. Every `console=` receives kernel messages,
          # but the LAST one becomes /dev/console, which is what init and
          # `serial-getty@ttyS0` attach to. ttyS0 must therefore come last.
          boot.kernelParams = [ "console=tty0" "console=ttyS0,115200" ];

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
