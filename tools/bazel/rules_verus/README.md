# rules_verus

Bazel rules for Verus formal verification of Rust code.

## Rules

### `verus_verify`

Runs Verus verification on Rust source files.

**Attributes:**

| Attribute | Type       | Required | Description                              |
|-----------|------------|----------|------------------------------------------|
| `srcs`    | label_list | yes      | Rust source files to verify.             |
| `deps`    | label_list | no       | Dependencies (e.g., `verus_library` targets). |

**Example:**

```starlark
load("//tools/bazel/rules_verus:defs.bzl", "verus_verify")

verus_verify(
    name = "verify_scheduler",
    srcs = ["src/scheduler.rs"],
    deps = [":scheduling_lib"],
)
```

### `verus_library`

Compiles and verifies a Rust library with Verus, producing a `.verified` marker.

**Attributes:**

| Attribute | Type       | Required | Description                    |
|-----------|------------|----------|--------------------------------|
| `srcs`    | label_list | yes      | Rust source files to verify.   |

### `verus_toolchain`

Registers paths to the Verus binary.

**Attributes:**

| Attribute    | Type        | Default    | Description                         |
|--------------|-------------|------------|-------------------------------------|
| `verus`      | string      | `"verus"`  | Path to the verus binary.           |
| `verus_root` | string      | `""`       | Verus installation root.            |
| `extra_args` | string_list | `[]`       | Extra flags for every invocation.   |

## Toolchain setup

In your `MODULE.bazel`:

```starlark
register_toolchains("//tools/bazel/rules_verus:default_toolchain")
```

## How it works

Verus is invoked on each source file. The action produces a verification log
and fails if any proof obligation cannot be discharged by the SMT solver.
Results are cached by Bazel, so re-verification only happens when sources change.
