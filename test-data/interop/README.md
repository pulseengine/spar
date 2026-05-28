# AADL interop corpus

Tier-A of the spar AADL plug-fest — see **issue #246** for the full design
(Tiers A/B/C, OSATE-in-Docker round-trips, instance-equivalence).

## What this is

`osate-examples/` is a **git submodule** pointing at
[`osate/examples`](https://github.com/osate/examples) — the SEI/CMU OSATE
example-model collection — pinned at commit `8db5647`. The
`tests/osate_corpus.rs` test in `spar-cli` parses every `.aadl` file in it
with spar's own parser and ratchets the result: spar must never regress on
a file it currently accepts.

Baseline at pin time: **516 / 548 files parse** (core 281/310, emv2 187/190,
behavior 5/5, annex-other 43/43). The set of currently-unparsed files lives
in `osate-corpus-expected-failures.txt`; a file leaving that list is
progress, a file failing that is *not* on it is a regression the test
rejects.

## Provenance and licensing — READ BEFORE EXTENDING

- The upstream repo carries **no explicit `LICENSE` file** and its README
  states it is **no longer maintained**. We therefore do **not** restate a
  license for it here, and we do **not** copy its content into this repo's
  git history.
- It is referenced as a **submodule** specifically so the example files are
  *fetched from upstream at build time*, never redistributed inside spar's
  own blobs. This is a reference, not a redistribution.
- **Before any Tier-B work that copies, transforms, or redistributes a
  subset of these models** (e.g. checking adjudicated fixtures into
  `adjudicated/`), the licensing must be resolved — either by curating only
  models with a clear license, or by hand-authoring spec-shaped fixtures
  from the public AS5506 annex examples.

## Updating the pin

```sh
git -C test-data/interop/osate-examples fetch origin
git -C test-data/interop/osate-examples checkout <new-sha>
git add test-data/interop/osate-examples
# re-bless the baseline against the new corpus:
SPAR_CORPUS_BLESS=1 cargo test -p spar --test osate_corpus
git add test-data/interop/osate-corpus-expected-failures.txt
```

## Running locally

```sh
git submodule update --init test-data/interop/osate-examples
cargo test -p spar --test osate_corpus -- --nocapture
```

The test **skips with a warning** (does not fail) if the submodule is not
checked out, so a non-recursive clone is not a hard error.
