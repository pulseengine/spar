#!/usr/bin/env python3
"""
Migrate STPA shorthand fields to canonical `links:` entries.

Rivet's schema declares link-fields for STPA types (e.g. `uca.controller →
issued-by`, `uca.hazards → leads-to-hazard`). Authors used the shorthand
form (`controller: CTRL-X` at the top level), which rivet's `stpa-yaml`
source format does not expand into the `links:` graph. Result: 625
cardinality ERRORs in `rivet validate` despite 0 broken cross-refs.

This script inserts a canonical `links:` block after each artifact's
`type:` line, derived from the shorthand fields. The shorthand fields
themselves are preserved (so authors and the stpa-yaml source format
keep working). After migration, rivet's link-counter sees the explicit
entries and the cardinality errors go to zero.

Text-based, line-by-line — preserves all comments and formatting.
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

# Mapping: (artifact_type, shorthand_field) -> canonical link type.
# For scalar shorthand (`controller: CTRL-X`), the value is a single ID.
# For list shorthand (`hazards: [H-1, H-2]`), the value is parsed as a list.
LINK_MAP: dict[tuple[str, str], str] = {
    # hazard
    ("hazard", "losses"):               "leads-to-loss",
    # sub-hazard
    ("sub-hazard", "parent"):           "refines",
    # system-constraint
    ("system-constraint", "hazards"):   "prevents",
    # uca
    ("uca", "controller"):              "issued-by",
    ("uca", "hazards"):                 "leads-to-hazard",
    # controller-constraint
    ("controller-constraint", "controller"): "constrains-controller",
    ("controller-constraint", "ucas"):       "inverts-uca",
    ("controller-constraint", "hazards"):    "prevents",
    # loss-scenario
    ("loss-scenario", "uca"):                "caused-by-uca",
    ("loss-scenario", "ucas"):               "caused-by-uca",
    ("loss-scenario", "hazards"):            "leads-to-hazard",
    # control-action
    ("control-action", "source"):            "issued-by",
    ("control-action", "target"):            "acts-on",
}

# Fields that carry list values. Others are scalar.
LIST_FIELDS = {"losses", "hazards", "ucas"}

# Regex: start of an artifact block
RE_ID = re.compile(r'^(?P<indent>\s*)- id:\s*(?P<id>\S+)\s*$')
# `type: <name>`
RE_TYPE = re.compile(r'^(?P<indent>\s+)type:\s*(?P<type>\S+)\s*$')
# Scalar field: `  controller: CTRL-X`
RE_SCALAR = re.compile(r'^(?P<indent>\s+)(?P<field>[a-z][a-z0-9-]*):\s*(?P<value>[A-Za-z][A-Za-z0-9_-]*)\s*$')
# Inline-list field: `  hazards: [H-1, H-2]`
RE_INLINE_LIST = re.compile(r'^(?P<indent>\s+)(?P<field>[a-z][a-z0-9-]*):\s*\[(?P<items>[^\]]*)\]\s*$')
# Block-list field start: `  hazards:` followed by `    - H-1` lines
RE_BLOCK_LIST_HEAD = re.compile(r'^(?P<indent>\s+)(?P<field>[a-z][a-z0-9-]*):\s*$')
RE_BLOCK_LIST_ITEM = re.compile(r'^(?P<indent>\s+)-\s+(?P<value>[A-Za-z][A-Za-z0-9_-]*)\s*$')
# Existing `links:` block head — if present, skip (already canonical)
RE_LINKS_HEAD = re.compile(r'^(?P<indent>\s+)links:\s*$')


def parse_list_items(raw: str) -> list[str]:
    """Parse the inside of `[a, b, c]` (whitespace-tolerant)."""
    return [x.strip() for x in raw.split(",") if x.strip()]


def process_file(path: Path) -> tuple[str, int]:
    """
    Return (new_text, inserted_link_count).
    Inserts a `links:` block after `type:` in each artifact that has
    matching shorthand fields AND does not already declare `links:`.
    """
    lines = path.read_text().splitlines(keepends=True)
    out: list[str] = []
    inserted = 0

    i = 0
    while i < len(lines):
        line = lines[i]
        m_id = RE_ID.match(line)
        if not m_id:
            out.append(line)
            i += 1
            continue

        # Found an artifact. Scan to end-of-block (next `- id:` at same
        # indent, or next package-level key, or EOF). Inside, find `type:`
        # and collect shorthand fields matching the link map.
        base_indent = m_id.group("indent")
        block_start = i
        i += 1

        type_line_idx: int | None = None
        artifact_type: str | None = None
        has_links_block = False
        collected: list[tuple[str, str]] = []  # (link_type, target_id)

        while i < len(lines):
            cur = lines[i]
            # Next artifact at same indent — end of this block
            next_id = RE_ID.match(cur)
            if next_id and next_id.group("indent") == base_indent:
                break
            # Top-level key (e.g. `artifacts:`, `links:` at file top) at zero indent
            if cur and not cur[0].isspace() and cur.strip() and not cur.startswith("#"):
                break

            m_type = RE_TYPE.match(cur)
            if m_type and type_line_idx is None:
                type_line_idx = i
                artifact_type = m_type.group("type")
                i += 1
                continue

            if RE_LINKS_HEAD.match(cur):
                has_links_block = True

            # Shorthand fields — collect if this artifact type maps them
            if artifact_type is not None:
                m_scalar = RE_SCALAR.match(cur)
                if m_scalar:
                    field = m_scalar.group("field")
                    if field not in LIST_FIELDS and (artifact_type, field) in LINK_MAP:
                        link_t = LINK_MAP[(artifact_type, field)]
                        collected.append((link_t, m_scalar.group("value")))

                m_inline = RE_INLINE_LIST.match(cur)
                if m_inline:
                    field = m_inline.group("field")
                    if (artifact_type, field) in LINK_MAP:
                        link_t = LINK_MAP[(artifact_type, field)]
                        for v in parse_list_items(m_inline.group("items")):
                            collected.append((link_t, v))

                m_blist = RE_BLOCK_LIST_HEAD.match(cur)
                if m_blist and (artifact_type, m_blist.group("field")) in LINK_MAP:
                    link_t = LINK_MAP[(artifact_type, m_blist.group("field"))]
                    # Walk inline items on subsequent lines
                    j = i + 1
                    while j < len(lines):
                        m_item = RE_BLOCK_LIST_ITEM.match(lines[j])
                        if not m_item:
                            break
                        collected.append((link_t, m_item.group("value")))
                        j += 1

            i += 1

        # Emit the collected lines for this block. If we have collected
        # links AND no existing `links:` block, insert after `type:`.
        block_end = i  # i points at the next block's first line (or EOF)
        block = lines[block_start:block_end]

        if collected and not has_links_block and type_line_idx is not None:
            # Indent for the inserted `links:` key — same as `type:` indent.
            type_indent = RE_TYPE.match(lines[type_line_idx]).group("indent")
            item_indent = type_indent + "  "  # +2 spaces
            built = [f"{type_indent}links:\n"]
            for link_t, target in collected:
                # Block-form flow: rivet's stpa-yaml source expects each
                # link as a mapping over two lines, not an inline flow
                # mapping. (Semantically equivalent YAML; rivet's parser
                # insists on the block form.)
                built.append(f"{item_indent}- type: {link_t}\n")
                built.append(f"{item_indent}  target: {target}\n")
            inserted += len(collected)

            # Relative index of the type line inside `block`
            rel_type_idx = type_line_idx - block_start
            block = (
                block[: rel_type_idx + 1]   # everything up to and including `type:`
                + built                     # the new links block
                + block[rel_type_idx + 1 :] # rest of the artifact
            )

        out.extend(block)

    return "".join(out), inserted


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    stpa_dir = root / "safety" / "stpa"
    files = sorted(stpa_dir.glob("*.yaml"))
    total_inserted = 0
    for f in files:
        new_text, n = process_file(f)
        if n > 0:
            f.write_text(new_text)
            print(f"{f.relative_to(root)}: +{n} canonical link entries")
            total_inserted += n
        else:
            print(f"{f.relative_to(root)}: unchanged")
    print(f"TOTAL: {total_inserted} canonical link entries inserted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
