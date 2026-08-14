# OSATE conformance

Validates spar against OSATE 2.18.0 as the reference AADL implementation, in
**both directions**:

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
