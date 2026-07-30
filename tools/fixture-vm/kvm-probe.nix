# A two-second stand-in for nixos-disk-image.drv, used by the KVM precondition
# step in .github/workflows/trace-fixtures.yml.
#
# It exists because the previous precondition asked the wrong question. That one
# read Nix's computed `system-features` and greened when `kvm` appeared in it.
# `system-features` governs SCHEDULING — whether a derivation may be PLACED on
# this machine — and Nix computes it in the parent process. The build then runs
# its builder as a different user. In run 30572572863 the parent could reach
# /dev/kvm and the builder could not, so the probe passed and the build died
# with `Could not access KVM kernel module: Permission denied`.
#
# This derivation closes that gap by being the same shape as the thing it
# stands in for:
#
#   * `requiredSystemFeatures = [ "kvm" ]` — so Nix schedules it through exactly
#     the path it schedules nixos-disk-image.drv through. If scheduling is what
#     is broken, this fails with the same features{kvm} error, in seconds.
#   * the builder opens /dev/kvm O_RDWR — the syscall QEMU makes. It therefore
#     runs as the BUILD principal, which is the one that actually matters and the
#     one no config read can speak for.
#   * it prints `id` first, so the log names that principal outright.
#
# Deliberately dependency-free: no nixpkgs, no fetch, no store closure beyond
# what the container already has. `builder = "/bin/sh"` is an out-of-store path,
# which is legal only because the workflow sets `sandbox = false` (the container
# is already the isolation boundary). Keep those two facts together — if the
# sandbox is ever re-enabled here, this builder path must change with it.
derivation {
  name = "kvm-open-probe";
  system = "x86_64-linux";
  builder = "/bin/sh";
  args = [
    "-c"
    ''
      set -eu
      echo "builder principal: $(id)"
      # A failed `exec` redirection already exits a non-interactive shell, but
      # say so explicitly — this line IS the assertion, and it should not depend
      # on a POSIX subtlety to fail the build.
      exec 3<>/dev/kvm
      echo ok > "$out"
    ''
  ];
  requiredSystemFeatures = [ "kvm" ];
}
