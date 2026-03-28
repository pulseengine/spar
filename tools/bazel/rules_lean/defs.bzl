# rules_lean — Bazel rules for Lean4 proof compilation and verification.
#
# These rules wrap the Lean4 toolchain (lake/lean) so that proof-checking
# is tracked as a Bazel action. Generated scheduling proofs from
# `spar codegen` (proof_gen.rs) are compiled and verified through these rules.
#
# Usage in BUILD.bazel:
#   load("//tools/bazel/rules_lean:defs.bzl", "lean_library", "lean_verify")
#
#   lean_library(
#       name = "scheduling_rta",
#       srcs = ["Proofs/Scheduling/RTA.lean"],
#   )
#
#   lean_verify(
#       name = "verify_cpu_scheduling",
#       srcs = ["proofs/maincpu_scheduling.lean"],
#       deps = [":scheduling_rta"],
#   )

# ---------------------------------------------------------------------------
# Toolchain type + resolution
# ---------------------------------------------------------------------------

LeanToolchainInfo = provider(
    doc = "Provider for the Lean4 toolchain.",
    fields = {
        "lean": "Path to the lean binary.",
        "lake": "Path to the lake build tool.",
        "lean_path": "LEAN_PATH environment variable value (colon-separated lib dirs).",
        "extra_args": "Additional arguments to pass to lean on every invocation.",
    },
)

def _lean_toolchain_impl(ctx):
    toolchain_info = platform_common.ToolchainInfo(
        lean = LeanToolchainInfo(
            lean = ctx.attr.lean,
            lake = ctx.attr.lake,
            lean_path = ctx.attr.lean_path,
            extra_args = ctx.attr.extra_args,
        ),
    )
    return [toolchain_info]

lean_toolchain = rule(
    implementation = _lean_toolchain_impl,
    attrs = {
        "lean": attr.string(
            default = "lean",
            doc = "Path to the lean binary. Defaults to PATH lookup.",
        ),
        "lake": attr.string(
            default = "lake",
            doc = "Path to the lake build tool. Defaults to PATH lookup.",
        ),
        "lean_path": attr.string(
            default = "",
            doc = "Colon-separated list of directories to add to LEAN_PATH.",
        ),
        "extra_args": attr.string_list(
            default = [],
            doc = "Extra flags passed to every lean invocation.",
        ),
    },
    doc = "Defines a Lean4 toolchain with lean and lake paths.",
)

# ---------------------------------------------------------------------------
# LeanInfo provider — carries olean artifacts downstream
# ---------------------------------------------------------------------------

LeanInfo = provider(
    doc = "Carries compiled Lean4 artifacts (.olean files) for dependency resolution.",
    fields = {
        "srcs": "depset of Lean source files.",
        "oleans": "depset of compiled .olean files.",
        "lib_dir": "Directory containing compiled .olean outputs.",
    },
)

# ---------------------------------------------------------------------------
# lean_library rule
# ---------------------------------------------------------------------------

def _lean_library_impl(ctx):
    toolchain = None
    if hasattr(ctx, "toolchains"):
        tc = ctx.toolchains.get("//tools/bazel/rules_lean:toolchain_type")
        if tc:
            toolchain = tc.lean

    lean = toolchain.lean if toolchain else "lean"
    lean_path = toolchain.lean_path if toolchain else ""
    extra_args = toolchain.extra_args if toolchain else []

    srcs = ctx.files.srcs

    # Collect olean dirs from dependencies.
    dep_dirs = []
    for dep in ctx.attr.deps:
        if LeanInfo in dep:
            dep_dirs.append(dep[LeanInfo].lib_dir)

    # Output directory for compiled .olean files.
    out_dir = ctx.actions.declare_directory(ctx.attr.name + "_oleans")

    # Build LEAN_PATH from toolchain + dependencies.
    lean_path_parts = [d.path for d in dep_dirs]
    if lean_path:
        lean_path_parts.append(lean_path)
    lean_path_env = ":".join(lean_path_parts) if lean_path_parts else ""

    # Compile each .lean file → .olean in the output directory.
    src_paths = [f.path for f in srcs]

    ctx.actions.run_shell(
        outputs = [out_dir],
        inputs = srcs + dep_dirs,
        command = """
set -euo pipefail
mkdir -p {out_dir}
export LEAN_PATH="{lean_path}"
for src in {srcs}; do
    OLEAN="{out_dir}/$(basename "$src" .lean).olean"
    {lean} {extra_args} --output="$OLEAN" "$src"
done
""".format(
            out_dir = out_dir.path,
            lean_path = lean_path_env,
            srcs = " ".join(src_paths),
            lean = lean,
            extra_args = " ".join(extra_args),
        ),
        mnemonic = "LeanCompile",
        progress_message = "Compiling Lean4 library %s" % ctx.label,
    )

    return [
        DefaultInfo(
            files = depset([out_dir]),
            runfiles = ctx.runfiles(files = [out_dir]),
        ),
        LeanInfo(
            srcs = depset(srcs),
            oleans = depset([out_dir]),
            lib_dir = out_dir,
        ),
    ]

lean_library = rule(
    implementation = _lean_library_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = [".lean"],
            mandatory = True,
            doc = "Lean4 source files to compile.",
        ),
        "deps": attr.label_list(
            default = [],
            doc = "Other lean_library targets this library depends on.",
        ),
    },
    toolchains = ["//tools/bazel/rules_lean:toolchain_type"],
    doc = """Compile Lean4 source files into .olean artifacts.

    Each .lean file is compiled with `lean --output=<olean>`. Dependency
    .olean directories are added to LEAN_PATH so that `import` statements
    resolve correctly.
    """,
)

# ---------------------------------------------------------------------------
# lean_verify rule
# ---------------------------------------------------------------------------

def _lean_verify_impl(ctx):
    toolchain = None
    if hasattr(ctx, "toolchains"):
        tc = ctx.toolchains.get("//tools/bazel/rules_lean:toolchain_type")
        if tc:
            toolchain = tc.lean

    lean = toolchain.lean if toolchain else "lean"
    lean_path = toolchain.lean_path if toolchain else ""
    extra_args = toolchain.extra_args if toolchain else []

    srcs = ctx.files.srcs

    # Collect olean dirs from dependencies.
    dep_dirs = []
    dep_files = []
    for dep in ctx.attr.deps:
        if LeanInfo in dep:
            dep_dirs.append(dep[LeanInfo].lib_dir)
        dep_files.extend(dep[DefaultInfo].files.to_list())

    # Output: verification log.
    proof_log = ctx.actions.declare_file(ctx.attr.name + ".lean.log")

    # Build LEAN_PATH.
    lean_path_parts = [d.path for d in dep_dirs]
    if lean_path:
        lean_path_parts.append(lean_path)
    lean_path_env = ":".join(lean_path_parts) if lean_path_parts else ""

    src_paths = [f.path for f in srcs]

    ctx.actions.run_shell(
        outputs = [proof_log],
        inputs = srcs + dep_files,
        command = """
set -euo pipefail
export LEAN_PATH="{lean_path}"
STATUS=0
for src in {srcs}; do
    echo "=== Checking $src ===" >> {log}
    if {lean} {extra_args} "$src" >> {log} 2>&1; then
        echo "  OK" >> {log}
    else
        echo "  FAILED" >> {log}
        STATUS=1
    fi
done
if [ "$STATUS" -ne 0 ]; then
    echo "Lean4 verification failed. See log:" >&2
    cat {log} >&2
    exit 1
fi
echo "All proofs verified." >> {log}
""".format(
            lean_path = lean_path_env,
            srcs = " ".join(src_paths),
            lean = lean,
            extra_args = " ".join(extra_args),
            log = proof_log.path,
        ),
        mnemonic = "LeanVerify",
        progress_message = "Verifying Lean4 proofs %s" % ctx.label,
    )

    return [DefaultInfo(
        files = depset([proof_log]),
        runfiles = ctx.runfiles(files = [proof_log]),
    )]

lean_verify = rule(
    implementation = _lean_verify_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = [".lean"],
            mandatory = True,
            doc = "Lean4 proof files to verify.",
        ),
        "deps": attr.label_list(
            default = [],
            doc = "lean_library targets needed by these proofs.",
        ),
    },
    toolchains = ["//tools/bazel/rules_lean:toolchain_type"],
    doc = """Verify Lean4 proof files.

    Each .lean source is type-checked by `lean`. The action fails if any
    proof contains sorry or has type errors, ensuring that only fully
    verified proofs pass the build.
    """,
)
