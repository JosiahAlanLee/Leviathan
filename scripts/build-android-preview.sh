#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

# Keep debug signing but omit debug symbols and enable enough optimization for
# an initial real-time audio test. NDK r27 needs explicit 16 KiB ELF alignment.
export CARGO_PROFILE_DEV_OPT_LEVEL=1 CARGO_PROFILE_DEV_DEBUG=0
alignment="-Clink-arg=-Wl,-z,max-page-size=16384"
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}"$'\x1f'"${alignment}"
else
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }${alignment}"
fi
cargo apk build -p leviathan-app --lib

# cargo-apk bundles Rust, not JVM sources. Add the thin NativeActivity host
# before the final zipalign/sign step. This is still a debug APK, not an AAB.
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:?Set ANDROID_HOME or ANDROID_SDK_ROOT}}"
tools="$sdk/build-tools/35.0.0"
jar="$sdk/platforms/android-35/android.jar"
app=android/src/main/java/io/github/josiahalanlee/leviathan/DocumentActivity.java
apk="${CARGO_TARGET_DIR:-target}/debug/apk/leviathan-app.apk"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/classes" "$work/dex"
if [[ -n "${LEVIATHAN_ECJ_JAR:-}" ]]; then
    java -jar "$LEVIATHAN_ECJ_JAR" -1.8 -nowarn -bootclasspath "$jar" -d "$work/classes" "$app"
else
    javac -source 8 -target 8 -Xlint:-options -bootclasspath "$jar" -d "$work/classes" "$app"
fi
mapfile -d '' classes < <(find "$work/classes" -name '*.class' -print0)
"$tools/d8" --lib "$jar" --min-api 29 --output "$work/dex" "${classes[@]}"
python3 - "$apk" "$work/unsigned.apk" "$work/dex/classes.dex" <<'PYAPK'
import sys, zipfile
source, output, dex = sys.argv[1:]
with zipfile.ZipFile(source) as old, zipfile.ZipFile(output, "w") as new:
    for entry in old.infolist():
        if not entry.filename.startswith("META-INF/"):
            new.writestr(entry, old.read(entry))
    new.write(dex, "classes.dex", zipfile.ZIP_DEFLATED)
PYAPK
"$tools/zipalign" -f 4 "$work/unsigned.apk" "$work/aligned.apk"
export LEVIATHAN_APK_KEY_PASS="${CARGO_APK_DEV_KEYSTORE_PASSWORD:-android}"
"$tools/apksigner" sign --ks "${CARGO_APK_DEV_KEYSTORE:-$HOME/.android/debug.keystore}" \
    --ks-pass env:LEVIATHAN_APK_KEY_PASS --v4-signing-enabled false --out "$work/leviathan-app.apk" "$work/aligned.apk"
"$tools/apksigner" verify "$work/leviathan-app.apk"
mv "$work/leviathan-app.apk" "$apk"
rm -f "$apk.idsig"
