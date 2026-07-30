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
#   * it names that principal outright, from /proc/self/status.
#
# WHY NOT `id`. The first version ran `echo "builder principal: $(id)"`. In run
# 30575158798 that printed `sh: line 2: id: command not found` and then the bare
# label `builder principal:` — Nix clears PATH for builders, and "dependency-free"
# means there is no coreutils either. `echo` still exited 0, so `set -eu` did not
# catch it. The line whose entire job was to be EVIDENCE rendered "nothing
# happened" identically to "it worked" — the same defect one level inside the fix
# for it. /proc/self/status needs only shell built-ins, and an empty read is now
# a hard failure rather than a blank line: a diagnostic is only evidence if its
# absence fails the build.
#
# What the Groups: line is actually for. `build-users-group =` (empty, set by the
# workflow) makes Nix run builders as the invoking user rather than dropping to a
# nixbld account — which is precisely why the supplementary group that
# `--group-add keep-groups` carries in survives into the build, and therefore why
# /dev/kvm is openable at all. Printing Uid/Gid/Groups makes that mechanism
# visible in the log instead of inferred.
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

      # Built-ins only — no coreutils on a dependency-free builder. Redirecting
      # a compound command (rather than piping into it) keeps the assignments in
      # this shell, so they are readable after the loop.
      uid= gid= grps=
      while read -r k v; do
        case "$k" in
          Uid:) uid="$v" ;;
          Gid:) gid="$v" ;;
          Groups:) grps="$v" ;;
        esac
      done < /proc/self/status

      if [ -z "$uid" ]; then
        echo "could not read the builder principal from /proc/self/status." >&2
        echo "The probe cannot name the principal it is testing, so its result" >&2
        echo "would not be attributable. Failing rather than reporting blank." >&2
        exit 1
      fi
      echo "builder principal: Uid=$uid Gid=$gid Groups=[$grps]"

      # A failed `exec` redirection already exits a non-interactive shell, but
      # say so explicitly — this line IS the assertion, and it should not depend
      # on a POSIX subtlety to fail the build.
      exec 3<>/dev/kvm
      echo ok > "$out"
    ''
  ];
  requiredSystemFeatures = [ "kvm" ];
}
