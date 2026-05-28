# AADL interop corpus

Tier-A of the spar AADL plug-fest — see **issue #246** for the full design
(Tiers A/B/C, OSATE-in-Docker round-trips, instance-equivalence).

## What this is

`osate-examples/` is the SEI/CMU [`osate/examples`](https://github.com/osate/examples)
model collection, **vendored verbatim** (all 548 `.aadl` source models, plus
the SEI `Notice.txt` files) at commit `8db5647`. Provenance and the SEI
reproduction grant are in [`PROVENANCE.md`](PROVENANCE.md). It is our body of
conformance fixtures: we parse it, and we fix **spar** — never the models —
when spar can't.

The `crates/spar-cli` test `osate_corpus.rs` parses every `.aadl` with spar's
own parser (`spar_syntax::parse`) and ratchets the result: spar must never
regress on a file it currently accepts.

## The two baselines (`baseline/`)

The set of files spar does not yet parse is split by *why*:

- **`spar-gaps.txt`** — genuine spar parser gaps (12 at last bless). These
  are real targets: we fix the parser and watch the list shrink. Examples:
  property-set `applies to` forms, named-constant property-type ranges,
  nested-list property values in connection blocks.
- **`unadjudicated.txt`** — `bugtrack-*` regression fixtures (17). These are
  OSATE's *own* bug-repro models and may be intentionally malformed; they are
  **not** treated as spar bugs until the Tier-B OSATE oracle confirms OSATE
  itself accepts them. Quarantining them keeps a parser gap from being
  conflated with a bad fixture.

A file failing that is absent from **both** baselines is a regression the
test rejects. Current signal: **519 / 548 parse** (core 284/310, emv2
187/190, behavior 5/5, annex-other 43/43).

## Workflow: fix spar, never the corpus

```sh
# After improving the parser, re-bless to lock progress in:
SPAR_CORPUS_BLESS=1 cargo test -p spar --test osate_corpus
git add test-data/interop/baseline/
```

The corpus files are reproduced under the SEI "without modification" grant
(see PROVENANCE.md) — editing them would both break the ratchet's meaning and
fall outside that grant.

## Running

```sh
cargo test -p spar --test osate_corpus -- --nocapture
```

The dedicated `.github/workflows/aadl-interop.yml` job runs it on
parser-relevant PRs + `workflow_dispatch`.
