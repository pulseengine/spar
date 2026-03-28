# rules_wasm_component

Bazel rules for building WASM components from Rust source code.

## Rules

### `wasm_component`

Compiles a `rust_library` target into a wasm32-wasip2 WASM component.

**Attributes:**

| Attribute | Type     | Required | Description                                      |
|-----------|----------|----------|--------------------------------------------------|
| `lib`     | label    | yes      | The `rust_library` target to compile.            |
| `wit`     | label    | no       | WIT interface file describing the component world.|
| `deps`    | label_list | no    | Additional link-time dependencies.               |
| `target`  | string   | no       | Rust target triple (default: `wasm32-wasip2`).   |

**Example:**

```starlark
load("//tools/bazel/rules_wasm_component:defs.bzl", "wasm_component")

wasm_component(
    name = "controller_component",
    lib = ":controller",
    wit = "//wit:controller.wit",
    target = "wasm32-wasip2",
)
```

### `wasm_component_toolchain`

Registers paths to `cargo` and `wasm-tools` binaries.

**Attributes:**

| Attribute    | Type   | Default          | Description                |
|--------------|--------|------------------|----------------------------|
| `cargo`      | string | `"cargo"`        | Path to the cargo binary.  |
| `wasm_tools` | string | `"wasm-tools"`   | Path to wasm-tools binary. |
| `rust_target`| string | `"wasm32-wasip2"`| Default compilation target.|

## Toolchain setup

In your `MODULE.bazel`:

```starlark
register_toolchains("//tools/bazel/rules_wasm_component:default_toolchain")
```

## How it works

1. `cargo build --target wasm32-wasip2 --release` produces a core `.wasm` module.
2. `wasm-tools component new` embeds WIT metadata, producing a standards-compliant WASM component.
