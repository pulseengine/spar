#!/usr/bin/env python3
"""Compare spar output against OSATE reference data.

Usage:
    python3 tools/osate-conformance/compare.py [--model BasicHierarchy]

Runs spar on the same test models that OSATE processed and compares:
- Component counts (total, per category)
- Feature counts
- Connection counts
- Component tree structure (names, categories, hierarchy)
"""

import argparse
import json
import os
import subprocess
import sys
import xml.etree.ElementTree as ET

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.join(SCRIPT_DIR, "..", "..")
REFERENCE_DIR = os.path.join(SCRIPT_DIR, "reference-data")
TEST_DATA_DIR = os.path.join(PROJECT_ROOT, "test-data", "osate2")

# Same models as the EASE script
TEST_MODELS = [
    ("BasicHierarchy.aadl", "BasicHierarchy::Top.Impl"),
    ("BasicBinding.aadl", "BasicBinding::Sys.Impl"),
    ("BasicEndToEndFlow.aadl", "BasicEndToEndFlow::Sys.Impl"),
    ("DigitalControlSystem.aadl", "DigitalControlSystem::DCS.Impl"),
    ("FlightSystem.aadl", "FlightSystem::FlightSystem.Impl"),
    ("GPSSystem.aadl", "GPSSystem::GPS.Impl"),
]


def run_spar(aadl_file, root_classifier):
    """Run spar instance --format json and return parsed JSON."""
    cmd = [
        "cargo", "run", "--release", "-p", "spar", "--",
        "instance", "--root", root_classifier, "--format", "json",
        aadl_file,
    ]
    result = subprocess.run(
        cmd, capture_output=True, text=True,
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None, result.stderr
    try:
        return json.loads(result.stdout), None
    except json.JSONDecodeError as e:
        return None, f"JSON parse error: {e}\nstdout: {result.stdout[:500]}"


def run_spar_analyze(aadl_file, root_classifier):
    """Run spar analyze --format json and return parsed JSON."""
    cmd = [
        "cargo", "run", "--release", "-p", "spar", "--",
        "analyze", "--root", root_classifier, "--format", "json",
        aadl_file,
    ]
    result = subprocess.run(
        cmd, capture_output=True, text=True,
        cwd=PROJECT_ROOT,
    )
    if result.returncode != 0:
        return None, result.stderr
    try:
        return json.loads(result.stdout), None
    except json.JSONDecodeError as e:
        return None, f"JSON parse error: {e}"


def load_osate_reference(model_base):
    """Load OSATE reference JSON for a model."""
    json_path = os.path.join(REFERENCE_DIR, "instances", f"{model_base}.json")
    if not os.path.exists(json_path):
        return None
    with open(json_path) as f:
        return json.load(f)


def load_osate_analysis(model_base):
    """Load OSATE analysis reference JSON."""
    json_path = os.path.join(REFERENCE_DIR, "analysis", f"{model_base}.json")
    if not os.path.exists(json_path):
        return None
    with open(json_path) as f:
        return json.load(f)


def count_spar_components(node):
    """Count components in spar's instance JSON tree."""
    if node is None:
        return 0
    count = 1
    for child in node.get("children", []):
        count += count_spar_components(child)
    return count


def count_spar_features(node):
    """Count features in spar's instance JSON tree."""
    if node is None:
        return 0
    count = len(node.get("features", []))
    for child in node.get("children", []):
        count += count_spar_features(child)
    return count


def count_spar_connections(node):
    """Count connections in spar's instance JSON tree."""
    if node is None:
        return 0
    count = len(node.get("connections", []))
    for child in node.get("children", []):
        count += count_spar_connections(child)
    return count


def compare_trees(osate_tree, spar_tree, path="root"):
    """Compare component trees structurally. Returns list of differences."""
    diffs = []

    if osate_tree is None or spar_tree is None:
        diffs.append(f"{path}: one tree is None")
        return diffs

    # Compare name
    osate_name = osate_tree.get("name", "").lower()
    spar_name = spar_tree.get("name", "").lower()
    if osate_name != spar_name:
        diffs.append(f"{path}: name mismatch: OSATE='{osate_name}' spar='{spar_name}'")

    # Compare category
    osate_cat = osate_tree.get("category", "").lower()
    spar_cat = spar_tree.get("category", "").lower()
    if osate_cat != spar_cat:
        diffs.append(f"{path}: category mismatch: OSATE='{osate_cat}' spar='{spar_cat}'")

    # Compare child count
    osate_children = osate_tree.get("children", [])
    spar_children = spar_tree.get("children", [])
    if len(osate_children) != len(spar_children):
        diffs.append(
            f"{path}: child count mismatch: OSATE={len(osate_children)} "
            f"spar={len(spar_children)}"
        )

    # Compare children by name
    osate_by_name = {c["name"].lower(): c for c in osate_children}
    spar_by_name = {c["name"].lower(): c for c in spar_children}

    for name in sorted(set(osate_by_name.keys()) | set(spar_by_name.keys())):
        if name not in osate_by_name:
            diffs.append(f"{path}/{name}: exists in spar but not OSATE")
        elif name not in spar_by_name:
            diffs.append(f"{path}/{name}: exists in OSATE but not spar")
        else:
            child_diffs = compare_trees(
                osate_by_name[name], spar_by_name[name], f"{path}/{name}"
            )
            diffs.extend(child_diffs)

    return diffs


def compare_model(filename, classifier, verbose=False):
    """Compare one model between OSATE and spar."""
    model_base = os.path.splitext(filename)[0]
    aadl_path = os.path.join(TEST_DATA_DIR, filename)

    print(f"\n{'='*60}")
    print(f"Model: {filename} [{classifier}]")
    print(f"{'='*60}")

    if not os.path.exists(aadl_path):
        print(f"  SKIP: {aadl_path} not found")
        return None

    # Load OSATE reference
    osate_ref = load_osate_reference(model_base)
    osate_analysis = load_osate_analysis(model_base)

    if osate_ref is None:
        print("  SKIP: No OSATE reference data (run generate-references.sh first)")
        return None

    # Run spar
    spar_instance, err = run_spar(aadl_path, classifier)
    if spar_instance is None:
        print(f"  FAIL: spar instance failed: {err}")
        return False

    spar_analysis, err = run_spar_analyze(aadl_path, classifier)

    # Compare counts
    osate_comp_count = osate_analysis.get("component_count", 0) if osate_analysis else 0
    osate_conn_count = osate_analysis.get("connection_count", 0) if osate_analysis else 0
    osate_feat_count = osate_analysis.get("feature_count", 0) if osate_analysis else 0

    spar_instance_node = spar_instance.get("instance") if spar_instance else None
    spar_comp_count = count_spar_components(spar_instance_node)
    spar_conn_count = count_spar_connections(spar_instance_node)
    spar_feat_count = count_spar_features(spar_instance_node)

    passed = True

    # Component count
    if osate_comp_count == spar_comp_count:
        print(f"  PASS: component count = {spar_comp_count}")
    else:
        print(f"  DIFF: component count: OSATE={osate_comp_count} spar={spar_comp_count}")
        passed = False

    # Connection count
    if osate_conn_count == spar_conn_count:
        print(f"  PASS: connection count = {spar_conn_count}")
    else:
        print(f"  DIFF: connection count: OSATE={osate_conn_count} spar={spar_conn_count}")
        passed = False

    # Feature count
    if osate_feat_count == spar_feat_count:
        print(f"  PASS: feature count = {spar_feat_count}")
    else:
        print(f"  DIFF: feature count: OSATE={osate_feat_count} spar={spar_feat_count}")
        passed = False

    # Structural tree comparison
    tree_diffs = compare_trees(osate_ref, spar_instance_node)
    if not tree_diffs:
        print(f"  PASS: component tree structure matches")
    else:
        print(f"  DIFF: {len(tree_diffs)} structural difference(s):")
        for d in tree_diffs[:10]:
            print(f"    - {d}")
        if len(tree_diffs) > 10:
            print(f"    ... and {len(tree_diffs) - 10} more")
        passed = False

    return passed


def main():
    parser = argparse.ArgumentParser(description="Compare spar vs OSATE reference data")
    parser.add_argument("--model", help="Specific model to compare (e.g., BasicHierarchy)")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    results = {}

    for filename, classifier in TEST_MODELS:
        model_base = os.path.splitext(filename)[0]
        if args.model and args.model != model_base:
            continue
        result = compare_model(filename, classifier, args.verbose)
        if result is not None:
            results[model_base] = result

    # Summary
    print(f"\n{'='*60}")
    print("SUMMARY")
    print(f"{'='*60}")

    if not results:
        print("No models compared (missing reference data or test files)")
        sys.exit(2)

    passed = sum(1 for v in results.values() if v)
    failed = sum(1 for v in results.values() if not v)
    print(f"  Passed: {passed}")
    print(f"  Failed: {failed}")
    print(f"  Total:  {len(results)}")

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
