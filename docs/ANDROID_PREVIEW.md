# Leviathan Android preview

First test device: Samsung Galaxy Tab A9+ Wi-Fi (SM-X210). This is an ARM64
debug/sideload build for Android 10 (API 29) or newer, not a Play Store release.
The preview keeps the same tracker, OtisKit and project files as desktop.

## Build and install

The Android preview workflow publishes a signed debug APK as its
`leviathan-android-arm64-debug` artifact. Unzip the artifact before sideloading.
Keep the signing certificate and application ID stable for upgrades; a build
with a different certificate will not install over an existing preview of that
app. Do not uninstall an existing preview without saving any important projects.

For a local build on Linux, use a JDK (with `javac`), Python 3, the Android SDK
(platform 35, build-tools 35.0.0), NDK 27.3.13750724 and Android platform-tools:

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk --version 0.10.0 --locked
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.3.13750724"
bash scripts/build-android-preview.sh
adb install -r target/debug/apk/leviathan-app.apk
```

The script compiles the small NativeActivity subclass in `android/` to DEX,
adds it to the Rust APK, then aligns and signs the result. The raw
`cargo apk build` output is not installable as the editor preview: it lacks
the Java activity. The platform layer uses Android's document provider only
for the item the user selects; it does not need broad storage access.

## Device smoke test

1. Open **Leviathan Portrait**, tap **Enable audio**, then **Play**. Check the
   built-in pattern, portrait transport, six lanes, note keys, and Android
   status/navigation bars. Try the tablet both ways up if supported.
2. Open **Samples → Waveform → Open WAV from Files**. Cancel once, then choose
   a 16-bit mono break with clearly spaced hits. Check both a local document
   and a provider-backed one. Confirm the tracker state survives picker return.
3. Move trim handles, zoom, tap a transient and add split markers. Audition
   each of four slices, then split into four empty OtisKit slots. Configure one
   lane as note-addressed **OtisKit** in **Voice / Trig**, select an empty row,
   and enter hits with **Enter hit** from the Waveform slot selector. Return
   to Grid and listen. Check **Undo split** on a scratch pattern; it restores
   the previous kit without changing the tracker notes.
4. Use **Save a copy**. Open the copy on a desktop without the source WAV,
   then reopen a desktop copy through **Open project** on Android. The kit PCM
   is embedded in the project. Replacing another document in place is not
   implemented on Android.
5. Switch away while playing, return, and unplug/reconnect audio. The document
   picker, audio-focus/recovery, and behavior under activity or process
   recreation still require physical-device verification; host and build
   checks are not a substitute.

See the [visual sampler spec and platform matrix](VISUAL_SAMPLER_SPEC.md).
Shader feedback is disabled in this Android preview; external MIDI and
hardware CV/ByteBus remain outside the tablet smoke scope.
