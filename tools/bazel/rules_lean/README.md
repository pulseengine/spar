# rules_lean

Bazel rules for Lean4 proof compilation and verification.

## Rules

### `lean_library`

Compiles Lean4 source files into `.olean` artifacts.

**Attributes:**

| Attribute | Type       | Required | Description                              |
|-----------|------------|----------|------------------------------------------|
| `srcs`    | label_list | yes      | Lean4 source files to compile.           |
| `deps`    | label_list | no       | Other `lean_library` targets to depend on.|

**Example:**

```starlark
load("//tools/bazel/rules_lean:defs.bzl", "lean_library", "lean_verify")

lean_library(
    name = "scheduling_rta",
    srcs = ["Proofs/Scheduling/RTA.lean"],
)

lean_verify(
    name = "verify_cpu_scheduling",
    srcs = ["proofs/maincpu_scheduling.lean"],
    deps = [":scheduling_rta"],
)
```

### `lean_verify`

Checks Lean4 proof files, failing if any contain `sorry` or type errors.

**Attributes:**

| Attribute | Type       | Required | Description                              |
|-----------|------------|----------|------------------------------------------|
| `srcs`    | label_list | yes      | Lean4 proof files to verify.             |
| `deps`    | label_list | no       | `lean_library` targets needed by proofs. |

### `lean_toolchain`

Registers paths to `lean` and `lake` binaries.

**Attributes:**

| Attribute    | Type        | Default   | Description                          |
|--------------|-------------|-----------|--------------------------------------|
| `lean`       | string      | `"lean"`  | Path to the lean binary.             |
| `lake`       | string      | `"lake"`  | Path to the lake build tool.         |
| `lean_path`  | string      | `""`      | Colon-separated LEAN_PATH additions. |
| `extra_args` | string_list | `[]`      | Extra flags for every invocation.    |

## Toolchain setup

In your `MODULE.bazel`:

```starlark
register_toolchains("//tools/bazel/rules_lean:default_toolchain")
```

## How it works

1. `lean_library` compiles each `.lean` file to `.olean` and exports a directory
   that downstream targets add to `LEAN_PATH`.
2. `lean_verify` type-checks each proof file. The action fails if Lean reports
   any errors, ensuring only fully-verified proofs pass the build.
