# rules_wasm_component — Bazel rules for building WASM components from Rust.
#
# Generated BUILD.bazel files from `spar codegen` load these rules to compile
# Rust crates into wasm32-wasip2 components using cargo + wasm-tools.
#
# Usage in BUILD.bazel:
#   load("//tools/bazel/rules_wasm_component:defs.bzl", "wasm_component")
#
#   wasm_component(
#       name = "my_component",
#       lib = ":my_rust_lib",
#       wit = "//wit:my_component.wit",
#       target = "wasm32-wasip2",
#   )

# ---------------------------------------------------------------------------
# Toolchain type + resolution
# ---------------------------------------------------------------------------

WasmComponentToolchainInfo = provider(
    doc = "Provider for the WASM component toolchain.",
    fields = {
        "cargo": "Path to the cargo binary.",
        "wasm_tools": "Path to the wasm-tools binary.",
        "rust_target": "Default Rust compilation target (e.g., wasm32-wasip2).",
    },
)

def _wasm_component_toolchain_impl(ctx):
    toolchain_info = platform_common.ToolchainInfo(
        wasm_component = WasmComponentToolchainInfo(
            cargo = ctx.attr.cargo,
            wasm_tools = ctx.attr.wasm_tools,
            rust_target = ctx.attr.rust_target,
        ),
    )
    return [toolchain_info]

wasm_component_toolchain = rule(
    implementation = _wasm_component_toolchain_impl,
    attrs = {
        "cargo": attr.string(
            default = "cargo",
            doc = "Path to the cargo binary. Defaults to PATH lookup.",
        ),
        "wasm_tools": attr.string(
            default = "wasm-tools",
            doc = "Path to the wasm-tools binary. Defaults to PATH lookup.",
        ),
        "rust_target": attr.string(
            default = "wasm32-wasip2",
            doc = "Rust target triple for WASM compilation.",
        ),
    },
    doc = "Defines a WASM component toolchain with cargo and wasm-tools paths.",
)

# ---------------------------------------------------------------------------
# wasm_component rule
# ---------------------------------------------------------------------------

def _wasm_component_impl(ctx):
    # Resolve toolchain (fall back to defaults if not registered).
    toolchain = None
    if hasattr(ctx, "toolchains"):
        tc = ctx.toolchains.get("//tools/bazel/rules_wasm_component:toolchain_type")
        if tc:
            toolchain = tc.wasm_component

    cargo = toolchain.cargo if toolchain else "cargo"
    wasm_tools = toolchain.wasm_tools if toolchain else "wasm-tools"
    rust_target = ctx.attr.target if ctx.attr.target else (
        toolchain.rust_target if toolchain else "wasm32-wasip2"
    )

    # Collect source files from the library dependency.
    lib_info = ctx.attr.lib[DefaultInfo]
    lib_files = lib_info.files.to_list()

    # Declare outputs.
    core_wasm = ctx.actions.declare_file(ctx.attr.name + ".core.wasm")
    component_wasm = ctx.actions.declare_file(ctx.attr.name + ".wasm")

    # Collect all input files.
    inputs = []
    inputs.extend(lib_files)
    if ctx.file.wit:
        inputs.append(ctx.file.wit)
    for dep in ctx.attr.deps:
        inputs.extend(dep[DefaultInfo].files.to_list())

    # Step 1: cargo build → core WASM module.
    #
    # In a real workspace the library target will already have been compiled
    # by rules_rust; here we run cargo to pick up any extra features needed
    # for the cdylib output.
    cargo_args = [
        cargo,
        "build",
        "--release",
        "--target", rust_target,
        "--message-format=json",
    ]

    ctx.actions.run_shell(
        outputs = [core_wasm],
        inputs = inputs,
        command = """
set -euo pipefail
{cargo_cmd}
# Locate the cdylib produced by cargo.
WASM_FILE=$(find target/{target}/release/deps -name '*.wasm' | head -1)
if [ -z "$WASM_FILE" ]; then
    echo "ERROR: cargo build produced no .wasm file" >&2
    exit 1
fi
cp "$WASM_FILE" {output}
""".format(
            cargo_cmd = " ".join(cargo_args),
            target = rust_target,
            output = core_wasm.path,
        ),
        mnemonic = "CargoBuildWasm",
        progress_message = "Compiling %s to %s" % (ctx.label, rust_target),
    )

    # Step 2: wasm-tools component new → WASM component.
    #
    # Embeds the WIT world into the core module, producing a fully-typed
    # WASM component that can be composed or run under wasmtime.
    wit_args = []
    if ctx.file.wit:
        wit_args = ["--wit", ctx.file.wit.path]

    ctx.actions.run_shell(
        outputs = [component_wasm],
        inputs = [core_wasm] + ([ctx.file.wit] if ctx.file.wit else []),
        command = """
set -euo pipefail
{wasm_tools} component new {core} {wit_flag} -o {output}
""".format(
            wasm_tools = wasm_tools,
            core = core_wasm.path,
            wit_flag = " ".join(wit_args),
            output = component_wasm.path,
        ),
        mnemonic = "WasmComponentNew",
        progress_message = "Packaging WASM component %s" % ctx.label,
    )

    return [DefaultInfo(
        files = depset([component_wasm]),
        runfiles = ctx.runfiles(files = [component_wasm]),
    )]

wasm_component = rule(
    implementation = _wasm_component_impl,
    attrs = {
        "lib": attr.label(
            mandatory = True,
            doc = "The rust_library target to compile into a WASM component.",
        ),
        "wit": attr.label(
            allow_single_file = [".wit"],
            doc = "WIT interface file describing the component's world.",
        ),
        "deps": attr.label_list(
            default = [],
            doc = "Additional dependencies needed at link time.",
        ),
        "target": attr.string(
            default = "wasm32-wasip2",
            doc = "Rust target triple. Defaults to wasm32-wasip2.",
        ),
    },
    toolchains = ["//tools/bazel/rules_wasm_component:toolchain_type"],
    doc = """Compile a Rust library into a wasm32-wasip2 WASM component.

    This rule runs two steps:
      1. `cargo build --target <target>` to produce a core .wasm module.
      2. `wasm-tools component new` to embed WIT metadata and produce a
         standards-compliant WASM component.

    The output .wasm file can be composed with other components via wac,
    or run directly under a WASI-compatible runtime such as wasmtime.
    """,
)
