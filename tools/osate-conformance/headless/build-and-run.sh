#!/usr/bin/env bash
# Build the `spar.osate.headless` bundle against OSATE's own plugins, register
# it in the OSATE install, and run it.
#
# WHY THIS EXISTS
# ---------------
# Stock OSATE ships NO headless application: there is no `IApplication` /
# `applications` extension anywhere in the osate2 tree. The repo's older
# `ease-scripts/` path is a dead end -- the EASE release site is pinned at
# 0.9.0 (2022), has no Rhino IU, and its JS engine needs the defunct WST/JSDT.
# Sireum's Phantom proves headless instantiation is possible but emits AIR
# JSON, not `.aaxl2`.
#
# So we add the missing application ourselves: ~250 lines mirroring Phantom's
# setup chain (Aadl2StandaloneSetup -> ErrorModelStandaloneSetup ->
# contributed-AADL preload), plus two things Phantom does not need -- a real
# linked IProject so `platform:/resource` saves resolve, and serialization at
# OSATE's canonical `instances/` URI.
#
# VALIDATED: output is byte-identical to the GUI-generated reference committed
# at test-data/osate2/instances/BasicBinding_s_i_Instance.aaxl2, and identical
# across runs (no timestamps, no UUIDs).
set -euo pipefail

OSATE_DIR="${OSATE_DIR:-$HOME/osate2}"
APP="$OSATE_DIR/osate2.app"
ECLIPSE="$APP/Contents/Eclipse"
PLUGINS="$ECLIPSE/plugins"
HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="${BUILD_DIR:-${TMPDIR:-/tmp}/spar-osate-headless-build}"
JAR="$PLUGINS/spar.osate.headless_1.0.0.jar"

[ -d "$PLUGINS" ] || {
  echo "ERROR: OSATE plugins not found at $PLUGINS" >&2
  echo "       Install first:  tools/osate-conformance/download-osate.sh" >&2
  echo "       Or point OSATE_DIR at an existing install." >&2
  exit 1
}

# OSATE 2.18 class files are Java 21; a host javac 17 cannot read them, and the
# failure is an opaque 'bad class file' rather than a version message.
JAVAC="$(ls -d "$PLUGINS"/org.eclipse.justj.openjdk.*/jre/bin/javac 2>/dev/null | head -1)"
JAVA="$(ls -d "$PLUGINS"/org.eclipse.justj.openjdk.*/jre/bin/java 2>/dev/null | head -1)"
LAUNCHER="$(ls "$PLUGINS"/org.eclipse.equinox.launcher_*.jar 2>/dev/null | head -1)"
[ -n "$JAVAC" ] && [ -n "$JAVA" ] && [ -n "$LAUNCHER" ] || {
  echo "ERROR: could not locate OSATE's bundled JDK or the Equinox launcher." >&2
  exit 1
}

echo "==> Compiling with OSATE's own javac ($("$JAVAC" -version 2>&1))"
rm -rf "$BUILD"
mkdir -p "$BUILD/classes"
"$JAVAC" -nowarn -proc:none -d "$BUILD/classes" -cp "$PLUGINS/*" \
  "$HERE/src/spar/osate/headless/InstantiateApp.java"

echo "==> Packaging bundle"
cp -r "$HERE/META-INF" "$BUILD/classes/"
cp "$HERE/plugin.xml" "$BUILD/classes/"
(cd "$BUILD/classes" && zip -q -r "$BUILD/spar.osate.headless_1.0.0.jar" .)
cp "$BUILD/spar.osate.headless_1.0.0.jar" "$JAR"

BINFO="$ECLIPSE/configuration/org.eclipse.equinox.simpleconfigurator/bundles.info"
if ! grep -q "spar.osate.headless" "$BINFO"; then
  echo "==> Registering bundle (backup at $BINFO.bak)"
  cp "$BINFO" "$BINFO.bak"
  echo "spar.osate.headless,1.0.0,plugins/spar.osate.headless_1.0.0.jar,4,true" >>"$BINFO"
fi

cat <<EOF

==> Built and registered.

IMPORTANT: do NOT use $APP/Contents/MacOS/osate. That native launcher HANGS
FOREVER on macOS after the application completes -- the main thread stays
parked in Cocoa, so the work finishes and the process never exits. Invoke
Equinox directly instead:

  JAVA="$JAVA"
  LAUNCHER="$LAUNCHER"

  # instantiate one or more roots into <projectDir>/instances/*.aaxl2
  "\$JAVA" -Xmx4g -jar "\$LAUNCHER" -application spar.osate.headless.app \\
    -data /tmp/osate-ws -configuration "$ECLIPSE/configuration" -nosplash \\
    instantiate <projectDir> "Pkg::Type.Impl"

  # parse + full Xtext validation; emits DIAG| and VERDICT| lines
  "\$JAVA" -Xmx4g -jar "\$LAUNCHER" -application spar.osate.headless.app \\
    -data /tmp/osate-ws -configuration "$ECLIPSE/configuration" -nosplash \\
    validate <projectDir> [--isolate]

Pass a DIRECTORY, not a file: models that \`with\` siblings need the whole
project, and under-supplying inputs manufactures false divergences.

Use a fresh -data workspace per run; a stale one caches previous verdicts.
EOF
