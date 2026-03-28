# rules_verus — Bazel rules for Verus formal verification of Rust code.
#
# Verus (https://github.com/verus-lang/verus) is a tool for verifying
# correctness of Rust programs using SMT-based reasoning. These rules
# integrate Verus into the Bazel build graph so that verification is
# tracked as a cacheable action.
#
# Usage in BUILD.bazel:
#   load("//tools/bazel/rules_verus:defs.bzl", "verus_verify", "verus_library")
#
#   verus_library(
#       name = "my_verified_lib",
#       srcs = ["src/lib.rs"],
#   )
#
#   verus_verify(
#       name = "my_verify",
#       srcs = ["src/lib.rs"],
#       deps = [":my_verified_lib"],
#   )

# ---------------------------------------------------------------------------
# Toolchain type + resolution
# ---------------------------------------------------------------------------

VerusToolchainInfo = provider(
    doc = "Provider for the Verus verification toolchain.",
    fields = {
        "verus": "Path to the verus binary.",
        "verus_root": "Path to the Verus installation root (for vstd, etc.).",
        "extra_args": "Additional arguments to pass to Verus on every invocation.",
    },
)

def _verus_toolchain_impl(ctx):
    toolchain_info = platform_common.ToolchainInfo(
        verus = VerusToolchainInfo(
            verus = ctx.attr.verus,
            verus_root = ctx.attr.verus_root,
            extra_args = ctx.attr.extra_args,
        ),
    )
    return [toolchain_info]

verus_toolchain = rule(
    implementation = _verus_toolchain_impl,
    attrs = {
        "verus": attr.string(
            default = "verus",
            doc = "Path to the verus binary. Defaults to PATH lookup.",
        ),
        "verus_root": attr.string(
            default = "",
            doc = "Root of the Verus installation (contains vstd crate).",
        ),
        "extra_args": attr.string_list(
            default = [],
            doc = "Extra flags passed to every verus invocation.",
        ),
    },
    doc = "Defines a Verus verification toolchain.",
)

# ---------------------------------------------------------------------------
# VerusInfo provider — carries verified-status downstream
# ---------------------------------------------------------------------------

VerusInfo = provider(
    doc = "Indicates that a target has been verified by Verus.",
    fields = {
        "srcs": "depset of source files that were verified.",
        "proof_log": "File containing the Verus verification log.",
    },
)

# ---------------------------------------------------------------------------
# verus_verify rule
# ---------------------------------------------------------------------------

def _verus_verify_impl(ctx):
    toolchain = None
    if hasattr(ctx, "toolchains"):
        tc = ctx.toolchains.get("//tools/bazel/rules_verus:toolchain_type")
        if tc:
            toolchain = tc.verus

    verus = toolchain.verus if toolchain else "verus"
    extra_args = toolchain.extra_args if toolchain else []

    # Collect inputs.
    srcs = ctx.files.srcs
    dep_files = []
    for dep in ctx.attr.deps:
        dep_files.extend(dep[DefaultInfo].files.to_list())

    inputs = srcs + dep_files

    # Declare the verification log as the output artifact.
    proof_log = ctx.actions.declare_file(ctx.attr.name + ".verus.log")

    # Build the verus command line.
    src_paths = [f.path for f in srcs]
    verus_args = [verus] + extra_args + src_paths

    ctx.actions.run_shell(
        outputs = [proof_log],
        inputs = inputs,
        command = """
set -euo pipefail
{verus_cmd} 2>&1 | tee {log}
# Verus exits 0 on success; non-zero on verification failure.
# The log is always written so Bazel can cache the result.
""".format(
            verus_cmd = " ".join(verus_args),
            log = proof_log.path,
        ),
        mnemonic = "VerusVerify",
        progress_message = "Verifying %s with Verus" % ctx.label,
    )

    return [
        DefaultInfo(files = depset([proof_log])),
        VerusInfo(
            srcs = depset(srcs),
            proof_log = proof_log,
        ),
    ]

verus_verify = rule(
    implementation = _verus_verify_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = [".rs"],
            mandatory = True,
            doc = "Rust source files to verify with Verus.",
        ),
        "deps": attr.label_list(
            default = [],
            doc = "Dependencies needed for verification (e.g., verus_library targets).",
        ),
    },
    toolchains = ["//tools/bazel/rules_verus:toolchain_type"],
    doc = """Run Verus verification on Rust source files.

    Produces a verification log. The action fails if Verus reports
    any verification errors, ensuring that unverified code cannot
    pass the build.
    """,
)

# ---------------------------------------------------------------------------
# verus_library rule
# ---------------------------------------------------------------------------

def _verus_library_impl(ctx):
    toolchain = None
    if hasattr(ctx, "toolchains"):
        tc = ctx.toolchains.get("//tools/bazel/rules_verus:toolchain_type")
        if tc:
            toolchain = tc.verus

    verus = toolchain.verus if toolchain else "verus"
    extra_args = toolchain.extra_args if toolchain else []

    srcs = ctx.files.srcs

    # Declare outputs: verification log + a marker file downstream can depend on.
    proof_log = ctx.actions.declare_file(ctx.attr.name + ".verus.log")
    marker = ctx.actions.declare_file(ctx.attr.name + ".verified")

    src_paths = [f.path for f in srcs]
    verus_args = [verus] + extra_args + ["--crate-type=lib"] + src_paths

    ctx.actions.run_shell(
        outputs = [proof_log, marker],
        inputs = srcs,
        command = """
set -euo pipefail
{verus_cmd} 2>&1 | tee {log}
# Create marker file only on success.
touch {marker}
""".format(
            verus_cmd = " ".join(verus_args),
            log = proof_log.path,
            marker = marker.path,
        ),
        mnemonic = "VerusLibrary",
        progress_message = "Verifying library %s with Verus" % ctx.label,
    )

    return [
        DefaultInfo(
            files = depset([marker]),
            runfiles = ctx.runfiles(files = [marker, proof_log]),
        ),
        VerusInfo(
            srcs = depset(srcs),
            proof_log = proof_log,
        ),
    ]

verus_library = rule(
    implementation = _verus_library_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = [".rs"],
            mandatory = True,
            doc = "Rust source files forming the verified library.",
        ),
    },
    toolchains = ["//tools/bazel/rules_verus:toolchain_type"],
    doc = """Compile and verify a Rust library with Verus.

    This rule verifies all source files and produces a .verified marker
    file that downstream targets can depend on to ensure the library
    has been formally verified.
    """,
)
