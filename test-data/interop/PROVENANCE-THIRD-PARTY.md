# Third-party AADL corpora

Four corpora written by neither pulseengine nor the OSATE team, vendored as
Tier-A parse material for the plug-fest (#246, #421).

## Why more than one

Each of these found parser gaps the others structurally could not, so the
number of corpora matters more than the size of any one of them.

The concrete case: `applies to (system, system implementation)` was **rejected
by spar and accepted by OSATE with 0 diagnostics** — a genuine bug in the
direction that makes spar unusable on a valid file. The vendored 548-model
OSATE corpus uses that construct in **zero files**, and so do our own 120. No
amount of parse-rate on either could have surfaced it. It took the Galois
corpus to see it, and once seen, three more owner forms turned up in AADLib and
one in VERDICT.

**Two corpora agreeing on what they do not contain is a shared blind spot, not
a signal.** That is the reason this directory has four entries and should
probably grow.

## What is here

| dir | upstream | pinned | models | licence |
|---|---|---|---|---|
| `case-aadl/` | [GaloisInc/CASE-AADL-Tutorial](https://github.com/GaloisInc/CASE-AADL-Tutorial) | `ee0947b07eaa` | 52 | BSD-3-Clause |
| `aadlib/` | [OpenAADL/AADLib](https://github.com/OpenAADL/AADLib) | `c8a5c7e310b3` | 239 | BSD-3-Clause |
| `fmw/` | [loonwerks/formal-methods-workbench](https://github.com/loonwerks/formal-methods-workbench) | `a5ac418f111a` | 924 | BSD-3 text, **SPDX-unclassified upstream** |
| `verdict/` | [ge-high-assurance/VERDICT](https://github.com/ge-high-assurance/VERDICT) | `344e42b5114c` | 132 | BSD-3-Clause |

Vendored 2026-08-14. All four LICENSE files are three-clause BSD texts. Note
the Rockwell Collins one is written longhand rather than by SPDX identifier,
so GitHub reports that repository as `NOASSERTION` and automated licence
tooling will read this corpus as unclassified. "BSD-3-Clause" for `fmw/` is
our reading of the text, not an upstream declaration. Its redistribution
clause requires the copyright notice be retained, so each directory carries its
upstream `LICENSE` verbatim.

**Taken:** `.aadl` sources only, verbatim, none cherry-picked, plus the licence.
**Not taken:** everything else — tutorial prose, build files, Eclipse project
state, generated code, images. Same rule as `osate-examples/`, and it keeps
1347 models to ~5.7 MB of source.

## The pins are unenforced

The commits above are recorded in prose only — there is no submodule, no
checksum manifest, and no vendoring script. Nothing detects a vendored file
drifting from its upstream. Re-verifying means re-cloning at the pin and
diffing by hand.

## Provenance of the models themselves

* **case-aadl** — DARPA CASE tutorial, AGREE-heavy (33 of 52 files), plus
  Resolute and VERDICT. Written against OSATE 2.10.2.
* **aadlib** — the model library of the **Ocarina** project, i.e. authored
  against the *other* AADL parser. The most useful corpus here for exactly that
  reason: it is not written to OSATE's behaviour.
* **fmw** — Rockwell Collins / loonwerks formal methods workbench. At 924
  models it is larger than OSATE's entire example tree.
* **verdict** — GE high-assurance. Note it vendors
  `CASE_Consolidated_Properties.aadl` 14 times.

## Reading the numbers honestly

A parse-rate is a property of the **corpus** — its duplication, its style, its
era — at least as much as of the parser.

VERDICT's 14 failures were 14 copies of one file: **one** distinct gap. Quoting
"89%" implied fourteen. Report distinct causes; if a rate is quoted, name the
corpus and the commit.

## These are NOT specifications

Two AADLib files (`ping-local.aadl`, `software.aadl`) are rejected by spar
**and** by OSATE, with grammar errors from both — Ocarina-specific syntax. A
third corpus is another test, not another authority. Where corpora disagree
with the tools, the standard decides. See
`crates/spar-cli/tests/osate_agreement.rs` for how that is handled.
