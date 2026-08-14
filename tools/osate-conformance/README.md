# OSATE conformance

Cross-checks spar against OSATE 2.18.0 in **both directions**.

> **OSATE is a test, not the authority.** The authority is SAE AS5506. OSATE is
> the reference implementation and by far the most useful oracle we can
> actually run, which is why everything here is measured against it — but it is
> an implementation, and implementations have bugs. A disagreement is a
> *question*, not a verdict: either spar is wrong, or OSATE is, and deciding
> which requires the standard rather than a re-run.
>
> This matters concretely because it is easy to build the opposite in by
> accident. If "agrees with OSATE" is the pass condition, every OSATE bug
> becomes a bug spar is obliged to reproduce, and the gate will fail a correct
> spar. So `osate_agreement.rs` treats spar-stricter-than-OSATE as something to
> **adjudicate** (`ADJUDICATED_STRICTER`, with the clause) rather than
> something forbidden, and `categories.rs` records which of its entries are
> unverified against the standard text.
>
> Where possible, prefer two independent signals over one: the 548-file corpus
> is data rather than code, so "the corpus never does X" and "the validator
> rejects X" are closer to independent than two runs of the same validator.

| direction | question | corpus | gate |
|---|---|---|---|
| **A** | can spar read what OSATE wrote? | 548 vendored `osate/examples` | `crates/spar-cli/tests/osate_corpus.rs` |
| **B** | does OSATE accept what *we* wrote? | 117 first-party `.aadl` | `crates/spar-cli/tests/osate_agreement.rs` |

Direction B is the one that bites a user — they open our model in OSATE and it
errors. See #420 for the current findings and #246 for the plug-fest design.

## Status of the pieces in this directory

Be aware that not all of this is live; the parts that are not are called out
rather than left to be discovered.

| file | status |
|---|---|
| `download-osate.sh` | **works** — 353 MB from CMU, slow but resumes |
| `headless/` | **works** — the headless oracle, see below |
| `generate-references.sh` | GUI-only **by design**: launches OSATE and prints manual steps |
| `ease-scripts/generate_references.py` | **bit-rotted, do not use** — needs a manually installed EASE plugin, hardcodes `/Volumes/Home/...`, and its model table (`Top.Impl`, `Sys.Impl`) does not match the actual fixtures (`sys.impl`, `s.i`), so it would fail on at least 2 of its 4 models even in the GUI |
| `compare.py` | half-live: the spar side is real, the OSATE side consumes JSON only the broken EASE flow produced |

The `headless/` bundle exists because **stock OSATE ships no headless
application** — there is no `IApplication` extension anywhere in osate2. EASE
is a dead end (release site pinned at 0.9.0/2022, no Rhino IU, JS engine needs
the defunct WST/JSDT). So we supply the missing application ourselves.

## Setup

```bash
./tools/osate-conformance/download-osate.sh      # ~353 MB, resumable
./tools/osate-conformance/headless/build-and-run.sh
```

The build script compiles against OSATE's own jars using its **bundled JustJ
Java 21** — a host `javac 17` cannot read OSATE 2.18 class files and fails with
an opaque "bad class file" rather than a version message. It then registers the
bundle in `bundles.info` (backup kept at `bundles.info.bak`).

**Do not use `osate2.app/Contents/MacOS/osate`.** That native launcher hangs
forever on macOS *after* the application completes — the work finishes and the
process never exits, because the main thread stays parked in Cocoa. Invoke
Equinox directly; `build-and-run.sh` prints the exact command with paths filled
in.

## Regenerating the Direction-B baseline

`test-data/interop/baseline/osate-first-party.tsv` holds the committed OSATE
verdicts. CI has no OSATE, so this file *is* the oracle — the test re-derives
only spar's column.

```bash
# per corpus directory; pass a DIRECTORY, never a single file, because models
# that `with` siblings need the whole project in scope
"$JAVA" -Xmx4g -jar "$LAUNCHER" -application spar.osate.headless.app \
  -data /tmp/osate-ws-$$ -configuration "$ECLIPSE/configuration" -nosplash \
  validate test-data/parser
```

Output is machine-readable:

```
DIAG|<file>|ERROR|<line>|<message>
VERDICT|<file>|ACCEPT|0
VALIDATE_DONE|<n>
```

Then update the TSV rows. Two rules, both learned by getting them wrong:

* **Use a fresh `-data` workspace per run.** A stale one caches prior verdicts.
* **Confirm a `VERDICT` line per file.** Do not infer acceptance from an exit
  code — Eclipse tooling exits 0 having done nothing routinely. Feed it a
  `test-data/negative/` file as a positive control to prove the pipeline can
  still produce a diagnostic at all.

## The second independent implementation: Ocarina

OSATE is one implementation. **Ocarina** (OpenAADL / Telecom ParisTech, Ada) is
another, sharing no code with OSATE's Xtext grammar, which is what lets a
disagreement be adjudicated rather than assumed. `crates/spar-cli/tests/
three_way_conformance.rs` gates against the two jointly.

### Building it on macOS aarch64

```bash
# 1. Ada toolchain. nixpkgs has gnat but marks gprbuild BROKEN on
#    aarch64-darwin, and it genuinely HANGS (measured: 0.10s of CPU in 104
#    minutes with no child processes). Use Alire's PREBUILT toolchain instead.
curl -sLO https://github.com/alire-project/alire/releases/download/v2.1.1/alr-2.1.1-bin-aarch64-macos.zip
unzip -q alr-2.1.1-bin-aarch64-macos.zip
./bin/alr -n toolchain --select gnat_native gprbuild   # gnat 16.1.0, gprbuild 26.0.0

# 2. Ocarina itself, from a FRESH checkout.
git clone --depth 1 https://github.com/OpenAADL/ocarina
cd ocarina
./support/reconfig && ./configure --prefix=<prefix> && make -j4 && make install
```

Two things that cost hours:

* **Do not `git clean -fdx` the tree.** It removes `objects/` and `libs/`
  directories git cannot track when empty, and later removes `mknodes`-
  generated sources. The build then fails in ways that look like Ocarina's
  problem and are not. Build from a fresh clone.
* **`make` exits non-zero and that is fine.** The transfo/python components
  fail on this platform; `make install` still produces a working `ocarina`.
  Check for the BINARY, not the exit code.

### Running it

```bash
ocarina -aadlv2 -f <file>     # exit 0 = accepted
```

**`-f` is load-bearing.** It supplies the predefined property sets; without it
Ocarina falsely rejects six of our first-party files (37 accept vs 43). Same
under-supplied-inputs trap as handing OSATE a single file instead of its
directory.

Regenerate `test-data/interop/baseline/three-way.tsv` by re-running both tools
over the paths in `osate-first-party.tsv` — the three-way test asserts the two
baselines cover the same corpus, so neither can drift ahead of the other.

## Re-deriving the subcomponent-category table

`crates/spar-parser/src/grammar/categories.rs` encodes which component
categories may contain which. That table is **measured, not transcribed**:

```bash
python3 tools/osate-conformance/headless/category-probe.py --generate /tmp/probe
"$JAVA" -Xmx4g -jar "$LAUNCHER" -application spar.osate.headless.app \
  -data /tmp/ws-probe -configuration "$ECLIPSE/configuration" -nosplash \
  validate /tmp/probe > /tmp/probe-verdicts.txt
python3 tools/osate-conformance/headless/category-probe.py --table /tmp/probe-verdicts.txt
```

196 pairs, currently **72 legal / 124 illegal**. `--table` refuses to emit a
table from an incomplete run (missing verdicts would shrink the legal set,
making spar too strict) and cross-checks against the vendored corpus, where any
observed pair is legal by construction.

Transcribing AS5506B by hand is the tempting alternative and the dangerous one:
a slip in the *forbidding* direction makes spar reject valid AADL, which the
agreement gate treats as an invariant, not a ratchet.

## What the Direction-B gate enforces

```text
                 OSATE accepts   OSATE rejects
  spar accepts         63              54        <- ratchet, may only fall
  spar rejects          0              20        <- HARD ZERO
```

**too-strict is an invariant, not a ratchet.** spar rejecting AADL that OSATE
accepts means we refuse a model the reference implementation considers valid;
there is no acceptable non-zero value.

**too-permissive ratchets down from 54.** Lower `MAX_TOO_PERMISSIVE` in
`osate_agreement.rs` as #420's categories are fixed. The test fails if the
count drops *below* the floor too, so a win is locked in rather than left as
slack.

Caveat carried deliberately: the OSATE column came from **full validation**
while spar's comes from `parse`, so some of the 54 may be caught by later spar
stages. #420 tracks re-running those at matched stage.

## Determinism of `.aaxl2` output

Verified byte-identical across runs and against the committed GUI-generated
reference (`test-data/osate2/instances/BasicBinding_s_i_Instance.aaxl2`). No
timestamps, no UUIDs. Three caveats for anyone diffing them:

* `componentInstance` order is **not** declaration order (stable, but not the
  order you wrote).
* All cross-references are **positional** EMF fragments
  (`#/0/@ownedPublicSection/@ownedClassifier.N/...`), so any edit to the source
  `.aadl` shifts them.
* `platform:/plugin` URIs pin the OSATE version's plugin paths.
