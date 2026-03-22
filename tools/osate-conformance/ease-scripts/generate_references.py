# OSATE Conformance Reference Data Generator
# In OSATE Script Shell (Python Py4J), run:
#   with open('/Volumes/Home/git/pulseengine/spar/tools/osate-conformance/ease-scripts/generate_references.py') as f:
#       c = compile(f.read(), 'gen.py', 'exec')
#   Then: exec(c)

import os
import json

SPAR_ROOT = "/Volumes/Home/git/pulseengine/spar"
TEST_DATA = os.path.join(SPAR_ROOT, "test-data", "osate2")
OUTPUT_DIR = os.path.join(SPAR_ROOT, "tools", "osate-conformance", "reference-data")

MODELS = [
    ("BasicHierarchy.aadl", "BasicHierarchy", "Top", "Impl"),
    ("BasicBinding.aadl", "BasicBinding", "Sys", "Impl"),
    ("BasicEndToEndFlow.aadl", "BasicEndToEndFlow", "Sys", "Impl"),
    ("FlightSystem.aadl", "FlightSystem", "FlightSystem", "Impl"),
]

# EASE provides Java classes directly via java.* syntax
URI = org.eclipse.emf.common.util.URI
NullProgressMonitor = org.eclipse.core.runtime.NullProgressMonitor
OsateResourceUtil = org.osate.aadl2.modelsupport.resources.OsateResourceUtil
InstantiateModel = org.osate.aadl2.instantiation.InstantiateModel

def ensure_dir(path):
    if not os.path.exists(path):
        os.makedirs(path)

def count_components(inst):
    n = 1
    children = inst.getComponentInstances()
    for i in range(children.size()):
        n += count_components(children.get(i))
    return n

def count_connections(inst):
    n = inst.getConnectionInstances().size()
    children = inst.getComponentInstances()
    for i in range(children.size()):
        n += count_connections(children.get(i))
    return n

def count_features(inst):
    n = inst.getFeatureInstances().size()
    children = inst.getComponentInstances()
    for i in range(children.size()):
        n += count_features(children.get(i))
    return n

def walk_tree(inst):
    node = {
        "name": str(inst.getName()),
        "category": str(inst.getCategory()),
        "features": [],
        "connections": [],
        "children": [],
    }
    feats = inst.getFeatureInstances()
    for i in range(feats.size()):
        f = feats.get(i)
        node["features"].append({
            "name": str(f.getName()),
            "direction": str(f.getDirection()),
            "category": str(f.getCategory()),
        })
    conns = inst.getConnectionInstances()
    for i in range(conns.size()):
        c = conns.get(i)
        src = str(c.getSource().getInstanceObjectPath()) if c.getSource() else "?"
        dst = str(c.getDestination().getInstanceObjectPath()) if c.getDestination() else "?"
        node["connections"].append({"name": str(c.getName()), "src": src, "dst": dst})
    children = inst.getComponentInstances()
    for i in range(children.size()):
        node["children"].append(walk_tree(children.get(i)))
    return node

print("=" * 60)
print("OSATE Reference Data Generator")
print("=" * 60)

ensure_dir(os.path.join(OUTPUT_DIR, "instances"))
ensure_dir(os.path.join(OUTPUT_DIR, "analysis"))

monitor = NullProgressMonitor()

for filename, pkg, typ, impl_name in MODELS:
    filepath = os.path.join(TEST_DATA, filename)
    if not os.path.exists(filepath):
        print("SKIP: " + filename)
        continue

    print("Processing: " + filename + " [" + pkg + "::" + typ + "." + impl_name + "]")

    try:
        uri = URI.createFileURI(filepath)
        rs = OsateResourceUtil.getResourceSet()
        resource = rs.getResource(uri, True)

        classifier = None
        contents = resource.getContents()
        for ci in range(contents.size()):
            pkg_obj = contents.get(ci)
            section = pkg_obj.getOwnedPublicSection()
            if section is not None:
                classifiers = section.getOwnedClassifiers()
                for j in range(classifiers.size()):
                    cl = classifiers.get(j)
                    name = str(cl.getName()) if cl.getName() else ""
                    if name == typ + "." + impl_name:
                        classifier = cl
                        break

        if classifier is None:
            print("  ERROR: classifier not found")
            continue

        instance = InstantiateModel.instantiate(classifier, monitor)
        if instance is None:
            print("  ERROR: instantiation returned null")
            continue

        cc = count_components(instance)
        cn = count_connections(instance)
        cf = count_features(instance)
        print("  Components: " + str(cc) + "  Connections: " + str(cn) + "  Features: " + str(cf))

        base = os.path.splitext(filename)[0]

        with open(os.path.join(OUTPUT_DIR, "instances", base + ".json"), "w") as f:
            json.dump(walk_tree(instance), f, indent=2)

        with open(os.path.join(OUTPUT_DIR, "analysis", base + ".json"), "w") as f:
            json.dump({"component_count": cc, "connection_count": cn, "feature_count": cf}, f, indent=2)

        print("  Saved.")

    except Exception as e:
        print("  ERROR: " + str(e))

print("Done!")
