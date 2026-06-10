# OSATE corpus provenance

## Source

- Upstream: [`osate/examples`](https://github.com/osate/examples) (SEI/CMU).
- Pinned commit: `8db5647c7d9d7a55edbb23b1d3d792c6f5315733`.
- Vendored: 2026-05-28.

## What was taken, and what was not

Vendored **verbatim, without modification**: all 548 `.aadl` source models
(every model in the upstream tree — none cherry-picked) plus the three
`Notice.txt` SEI copyright files.

**Not** vendored: the ~45 MB of non-source byproducts in the upstream tree —
compiled instance binaries (`.aadlbin`), serialized instance models
(`.aaxl2`, `.aadl2`), generated code (`.java`, `.c`, `.h`), diagrams
(`.aadl_diagram`), and IDE state (`.project`, `.aadlsettings`, `.gitignore`).
These are build/IDE artefacts, not AADL source. Dropping them keeps the
vendored tree at ~4.3 MB without modifying or omitting any `.aadl` model.
The `.aaxl2` instance models will be vendored later, from the same pin, when
the plug-fest's Tier-B (OSATE-in-Docker AAXL2 round-trip) lands.

## Licensing

The SEI `Notice.txt` files (preserved alongside the models) grant:

> **External use:** This material may be reproduced **in its entirety,
> without modification**, and freely distributed in written or electronic
> form without requesting formal permission. Permission is required for any
> other external and/or commercial use.

This vendoring relies on that grant: every `.aadl` file is reproduced
verbatim and unmodified, and the SEI copyright/No-Warranty notices travel
with them. The operative constraint — **without modification** — is enforced
by the plug-fest's design: we never edit a corpus file; when spar fails to
parse one, we fix **spar's parser**, never the model (see
`crates/spar-cli/tests/osate_corpus.rs` and the `baseline/` ratchet).

Some upstream files carry no SEI header (third-party/contributed models in
the same repo). If a clearly-licensed *subset* is ever needed for
redistribution beyond this verbatim reproduction (e.g. transformed fixtures
under `adjudicated/`), the licensing of those specific files must be
re-checked first — see issue #246.

## Updating the pin

```sh
# Fetch the upstream tree at a new commit into a scratch clone, then
# re-vendor the .aadl + Notice.txt files verbatim (no edits), update the
# commit hash above, and re-bless the baseline:
SPAR_CORPUS_BLESS=1 cargo test -p spar --test osate_corpus
git add test-data/interop/
```
