# SysML v2 corpus provenance

## Source

- Upstream: [`Systems-Modeling/SysML-v2-Release`](https://github.com/Systems-Modeling/SysML-v2-Release)
  — the OMG SysML v2 pilot implementation's release repository.
- Pinned commit: `29a3d2acdd49600cff872e7a55962a40400f3335`.
- Vendored: 2026-08-26, under `official/`.

## Why it is vendored and not fetched

`download-official-suite.sh` used to fetch this corpus from `master` at run
time, through the GitHub contents API, with `|| true` on every download and a
hardcoded fallback list when the API call failed. It had never been run: the
three `official/` directories it targets were empty, so **every SysML v2 number
this project has ever quoted was measured against 43 fixtures we wrote
ourselves**.

Fetching also makes the number unreproducible in two directions at once — the
upstream tip moves (the grammar still ships monthly incremental tags), and a
network failure degrades to a smaller corpus that reads as a better parse rate.
Vendoring at a pin fixes both, and matches what `test-data/interop/` already
does for the 548 OSATE models and the 1347 third-party AADL models.

## What was taken, and what was not

Vendored **verbatim, without modification** — 310 model files:

| path | files |
|---|---|
| `official/sysml/validation/` | 56 |
| `official/sysml/training/` | 100 |
| `official/sysml/examples/` | 96 |
| `official/kerml/examples/` | 58 |

**Not** vendored: the rest of the 366 MB upstream tree — the Xtext pilot
implementation itself, its Eclipse plugins, jars, Jupyter kernel, `.project`
and IDE state, and the API service sources. None of those are model source, and
none is needed to grade a parser.

## Licensing

Upstream is **EPL-2.0**; `official/LICENSE` is the upstream file, copied
unaltered. EPL-2.0 permits redistribution provided the licence travels with the
material and copyright notices are not removed or altered — both hold here, and
the models themselves are byte-identical to upstream.

This is the same arrangement as the vendored OSATE corpus, which is also
EPL-2.0. It is **not** the same as `Systems-Modeling/SysML-v2-AADL-Release`,
the AADL domain library, which is **CC-BY-ND 4.0** — that one may be vendored
verbatim but never forked or modified, and is not part of this corpus.

## A trap that has already cost two measurements

**Paths in this corpus contain spaces.** `kerml/examples/Address Book
Example/AddressBookModel.kerml` is typical. Any shell iteration of the form

```sh
for f in $(find test-data/sysml2/official -name '*.sysml'); do ...   # WRONG
```

word-splits those paths and feeds fragments to the tool, which then fails to
open them. Done while measuring the parse rate, this inflates the failure count
with paths that were never files — it produced a wrong headline number twice
before being caught. Use `find -print0` with `read -d ''`, or iterate in a
language with a real list type. `crates/spar-sysml2/tests/official_corpus.rs`
walks the tree with `std::fs` and does not have the problem.

## The baseline this corpus establishes

Measured 2026-08-26 against `f395518`, oracle = the parser's own
`Parse::ok()` (the same predicate the CLI exits non-zero on):

```
.sysml     0 / 252
.kerml     1 /  58
TOTAL      1 / 310   (0.3%)
```

A rate is a property of the corpus as much as the parser, so the failures were
classified rather than counted. There are **five distinct first-error kinds
across 309 failing files**, not 309 problems:

| first error | files | construct |
|---|---|---|
| `expected name` | 161 | quoted names — `package 'Application Layer';` |
| `expected member declaration` | 97 | visibility modifiers — `private import Objects::*;` |
| `expected SEMICOLON` | 41 | KerML `class` definitions |
| `expected package, import, or definition` | 9 | a visibility-modified import at file scope |
| `expected definition after abstract` | 1 | `abstract type A specializes …` |

So the parser is a small number of grammar gaps away from a large jump, which
is what `REQ-SYSML2-VISIBILITY-001` is scoped to close. The two KerML kinds
(`class`, `abstract type`) were **not** in that requirement's original scope and
are a finding of this vendoring — recheck it before planning v0.44.0.

## Refreshing the pin

```sh
./tools/vendor-sysml2-corpus.sh                 # re-vendors at the recorded pin
./tools/vendor-sysml2-corpus.sh <new-commit>    # moves the pin, deliberately
```

Moving the pin is a reviewable change: it will move the ratchet in
`official_corpus.rs`, and the test says so when it fails.
