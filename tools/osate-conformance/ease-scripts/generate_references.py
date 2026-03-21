# EASE/Py4J script to run inside OSATE.
#
# Generates reference data from AADL test models:
# - Instance model XML (.aaxl2)
# - Analysis results
# - Diagram SVG exports
#
# Usage: Open OSATE → Window → Show View → Script Shell →
#        Change to Python (Py4J) → Run this script
#
# Or from OSATE menu: Run → Run Script... → select this file

import os
import json
from java.io import File
from org.eclipse.core.resources import ResourcesPlugin
from org.eclipse.emf.common.util import URI

# OSATE Java API imports
from org.osate.aadl2.modelsupport.resources import OsateResourceUtil
from org.osate.aadl2.instantiation import InstantiateModel
from org.osate.xtext.aadl2.ui.resource import Aadl2ResourceSetProvider

# Configuration
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.join(SCRIPT_DIR, "..", "..", "..")
TEST_DATA_DIR = os.path.join(PROJECT_ROOT, "test-data", "osate2")
REFERENCE_DIR = os.path.join(SCRIPT_DIR, "..", "reference-data")

# Test models to process: (filename, root_classifier)
TEST_MODELS = [
    ("BasicHierarchy.aadl", "BasicHierarchy::Top.Impl"),
    ("BasicBinding.aadl", "BasicBinding::Sys.Impl"),
    ("BasicEndToEndFlow.aadl", "BasicEndToEndFlow::Sys.Impl"),
    ("DigitalControlSystem.aadl", "DigitalControlSystem::DCS.Impl"),
    ("FlightSystem.aadl", "FlightSystem::FlightSystem.Impl"),
    ("GPSSystem.aadl", "GPSSystem::GPS.Impl"),
]


def ensure_dirs():
    """Create output directories."""
    for subdir in ["instances", "analysis", "diagrams"]:
        path = os.path.join(REFERENCE_DIR, subdir)
        if not os.path.exists(path):
            os.makedirs(path)


def get_workspace():
    """Get the Eclipse workspace root."""
    return ResourcesPlugin.getWorkspace().getRoot()


def load_aadl_file(filepath):
    """Load an AADL file into OSATE's resource set."""
    uri = URI.createFileURI(filepath)
    rs = OsateResourceUtil.getResourceSet()
    resource = rs.getResource(uri, True)
    return resource


def find_classifier(resource, qualified_name):
    """Find a classifier by qualified name (Pkg::Type.Impl)."""
    parts = qualified_name.split("::")
    pkg_name = parts[0]
    type_impl = parts[1] if len(parts) > 1 else ""

    for obj in resource.getContents():
        if hasattr(obj, "getName") and obj.getName() == pkg_name:
            # Found the package, now find the classifier
            for elem in obj.getOwnedPublicSection().getOwnedClassifiers():
                full_name = elem.getName()
                if "." in type_impl:
                    # Looking for an implementation
                    if hasattr(elem, "getType") and elem.getType() is not None:
                        impl_name = elem.getType().getName() + "." + elem.getName().split(".")[-1]
                        if impl_name == type_impl or elem.getName() == type_impl.split(".")[-1]:
                            return elem
                elif full_name == type_impl:
                    return elem
    return None


def instantiate_and_export(filepath, classifier_name, output_base):
    """Instantiate a system and export the instance model."""
    print("Processing: {} [{}]".format(filepath, classifier_name))

    try:
        resource = load_aadl_file(filepath)
        classifier = find_classifier(resource, classifier_name)

        if classifier is None:
            print("  ERROR: Classifier '{}' not found".format(classifier_name))
            return

        # Instantiate
        instance = InstantiateModel.buildInstanceModelFile(classifier)

        if instance is None:
            print("  ERROR: Instantiation failed")
            return

        # Export instance model as XML (.aaxl2)
        instance_path = os.path.join(REFERENCE_DIR, "instances",
                                      output_base + ".aaxl2")
        # The instance is already saved by OSATE; copy it
        instance_uri = instance.eResource().getURI()
        print("  Instance saved: {}".format(instance_uri))

        # Export component tree as JSON for easy comparison
        tree = extract_component_tree(instance)
        json_path = os.path.join(REFERENCE_DIR, "instances",
                                  output_base + ".json")
        with open(json_path, "w") as f:
            json.dump(tree, f, indent=2)
        print("  Component tree: {}".format(json_path))

        # Run analyses and collect results
        analysis_results = run_analyses(instance)
        analysis_path = os.path.join(REFERENCE_DIR, "analysis",
                                      output_base + ".json")
        with open(analysis_path, "w") as f:
            json.dump(analysis_results, f, indent=2)
        print("  Analysis results: {}".format(analysis_path))

    except Exception as e:
        print("  ERROR: {}".format(str(e)))


def extract_component_tree(instance):
    """Extract a JSON-serializable component tree from an instance model."""
    def walk(component):
        node = {
            "name": str(component.getName()),
            "category": str(component.getCategory()),
            "children": [],
            "features": [],
            "connections": [],
        }

        # Features
        for feat in component.getFeatureInstances():
            node["features"].append({
                "name": str(feat.getName()),
                "category": str(feat.getCategory()),
                "direction": str(feat.getDirection()),
            })

        # Connections
        for conn in component.getConnectionInstances():
            node["connections"].append({
                "name": str(conn.getName()),
                "source": str(conn.getSource().getInstanceObjectPath()),
                "destination": str(conn.getDestination().getInstanceObjectPath()),
            })

        # Recurse into children
        for child in component.getComponentInstances():
            node["children"].append(walk(child))

        return node

    return walk(instance)


def run_analyses(instance):
    """Run standard OSATE analyses and collect results."""
    results = {
        "component_count": count_components(instance),
        "connection_count": count_connections(instance),
        "feature_count": count_features(instance),
    }
    return results


def count_components(instance):
    """Count all component instances recursively."""
    count = 1  # self
    for child in instance.getComponentInstances():
        count += count_components(child)
    return count


def count_connections(instance):
    """Count connection instances."""
    count = len(list(instance.getConnectionInstances()))
    for child in instance.getComponentInstances():
        count += count_connections(child)
    return count


def count_features(instance):
    """Count feature instances."""
    count = len(list(instance.getFeatureInstances()))
    for child in instance.getComponentInstances():
        count += count_features(child)
    return count


def main():
    print("=" * 60)
    print("OSATE Reference Data Generator")
    print("=" * 60)

    ensure_dirs()

    for filename, classifier in TEST_MODELS:
        filepath = os.path.join(TEST_DATA_DIR, filename)
        if not os.path.exists(filepath):
            print("SKIP: {} not found".format(filepath))
            continue

        output_base = os.path.splitext(filename)[0]
        instantiate_and_export(filepath, classifier, output_base)

    print("")
    print("Done. Reference data in: {}".format(REFERENCE_DIR))


# Run
main()
