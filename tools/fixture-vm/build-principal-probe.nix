# A two-second stand-in for nixos-disk-image.drv, used by the precondition step
# in .github/workflows/trace-fixtures.yml.
#
# It asserts, from inside a real Nix builder, the two things that derivation
# needs from its host which no configuration read can speak for: an openable
# /dev/kvm, and the Linux capabilities virtiofsd requires. Both are properties
# of the BUILD PRINCIPAL, and the parent process is not it.
#
# It exists because the first precondition asked the wrong question. That one
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
#   * it reads its own capability set from the same principal, for the reason
#     below.
#   * it names that principal outright, from /proc/self/status.
#
# WHY CAPABILITIES ARE CHECKED HERE AND NOT ONLY ON THE CONTAINER. virtiofsd is
# what serves the Nix store into the transient VM that nixos-disk-image.drv
# boots. It hands its child a fixed capability set with capng_apply(BOTH);
# raising a capability that is not already permitted returns EPERM, which
# virtiofsd reports as `can't apply the child capabilities: failed to sync
# capabilities with the kernel`. That is what killed run 30579194737 — and the
# line the eye lands on is three lines lower, where qemu reports the socket
# virtiofsd never bound as `Failed to connect to 'virtio-store.sock':
# Connection refused`. A refused connection to a socket with no listener names
# the symptom, not the cause.
#
# virtiofsd is spawned BY a Nix builder, so the builder's capability set is the
# one that decides. Checking the container's set instead would repeat the exact
# parent-vs-builder mistake described above, one axis over: /dev/kvm was about
# the principal's GROUPS, this is about the same principal's CAPABILITIES. The
# workflow grants them with --cap-add (PODMAN_CAPS) and prints the container's
# own mask next to this one — if the two differ, the grant is being dropped
# between podman and the builder and the fix does not belong in --cap-add.
#
# Only the capabilities the workflow actually adds are ASSERTED. The full
# CapEff/CapBnd masks are PRINTED, so a capability missing for some other reason
# is readable from the log rather than guessed at from a list of podman defaults
# recalled from memory. Asserting a capability nobody has shown to be required
# would just be a second way to be confidently wrong.
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
  name = "build-principal-probe";
  system = "x86_64-linux";
  builder = "/bin/sh";
  args = [
    "-c"
    ''
      set -eu

      # Built-ins only — no coreutils on a dependency-free builder. Redirecting
      # a compound command (rather than piping into it) keeps the assignments in
      # this shell, so they are readable after the loop.
      uid= gid= grps= capeff= capbnd=
      while read -r k v; do
        case "$k" in
          Uid:) uid="$v" ;;
          Gid:) gid="$v" ;;
          Groups:) grps="$v" ;;
          CapEff:) capeff="$v" ;;
          CapBnd:) capbnd="$v" ;;
        esac
      done < /proc/self/status

      # capeff is checked here as well as uid because it is consumed by
      # arithmetic below, where an empty value would expand to a bare `0x` and
      # fail with a parse error that names neither this file nor the cause.
      if [ -z "$uid" ] || [ -z "$capeff" ]; then
        echo "could not read the builder principal from /proc/self/status." >&2
        echo "The probe cannot name the principal it is testing, so its result" >&2
        echo "would not be attributable. Failing rather than reporting blank." >&2
        exit 1
      fi
      echo "builder principal:    Uid=$uid Gid=$gid Groups=[$grps]"
      echo "builder capabilities: CapEff=$capeff CapBnd=$capbnd"

      # Bit numbers are from linux/capability.h. POSIX arithmetic parses the
      # 0x form, and 64 bits is ample for a set that currently ends at 40.
      missing=
      for spec in 2:DAC_READ_SEARCH 27:MKNOD; do
        bit="''${spec%%:*}"
        nm="''${spec#*:}"
        if [ $(( (0x$capeff >> bit) & 1 )) -ne 1 ]; then
          missing="$missing $nm"
        fi
      done
      if [ -n "$missing" ]; then
        echo "build principal is missing capabilities:$missing" >&2
        echo "virtiofsd raises exactly these for its child; capng_apply returns" >&2
        echo "EPERM for any that is not already permitted, it exits, and the" >&2
        echo "store socket is never bound. Grant them via PODMAN_CAPS in" >&2
        echo ".github/workflows/trace-fixtures.yml. If the container's mask" >&2
        echo "printed above them DOES have these bits, the grant is being lost" >&2
        echo "between podman and the builder and --cap-add is not the fix." >&2
        exit 1
      fi

      # A failed `exec` redirection already exits a non-interactive shell, but
      # say so explicitly — this line IS the assertion, and it should not depend
      # on a POSIX subtlety to fail the build.
      exec 3<>/dev/kvm
      echo ok > "$out"
    ''
  ];
  requiredSystemFeatures = [ "kvm" ];
}
