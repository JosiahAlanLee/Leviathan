# Leviathan Android preview

First test device: Samsung Galaxy Tab A9+ Wi-Fi (SM-X210), in landscape.
This is a debug/sideload build for ARM64 devices on Android 10 (API 29) or newer,
not a Play Store release. The minimum accounts for the app's native MIDI
dependency; external MIDI is not part of this preview's test scope.

## Build and install

The Android preview workflow publishes a debug APK as the
`leviathan-android-arm64-debug` artifact. Unzip the artifact before sideloading.
Signing is for testing only. A later debug APK signed by a different builder
may not install over an earlier one; do not keep your only project copy here.

For local builds, install the Android SDK (platform 35, build-tools 35.0.0),
side-by-side NDK 27.3.13750724, Android platform-tools, and a JDK.

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk --version 0.10.0 --locked
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.3.13750724"
CARGO_PROFILE_DEV_OPT_LEVEL=1 CARGO_PROFILE_DEV_DEBUG=0 cargo apk build -p leviathan-app --lib
adb install -r target/debug/apk/leviathan-app.apk
```

## Tab A9+ smoke test

1. Open Leviathan. It should land on the Grid deck. The transport should show a
   live output device, not an offline clock. Tap **Play** and listen for the
   built-in electro pattern on the tablet speakers or wired headphones.
2. Tap a note cell in an audible lane and use the chromatic buttons above the
   grid. Verify the cell changes, Undo restores it, and the next pass reflects
   the edit without stopping the transport. Scroll and edit while playing and
   listen for clicks or dropouts. Check that editing and playing state remain
   distinct while queuing a pattern from Song.
3. Switch away and back, then unplug/reconnect wired audio. Record what happens;
   correct Android audio focus, stream recovery and lifecycle are still work
   to validate before this preview is suitable for live use.

The preview reuses the desktop score and synthesis engine. Shader feedback is
intentionally disabled to isolate audio and touch performance. Hardware
CV/ByteBus, external MIDI and custom shader hot-reload are not validated on
Android. The current desktop path dialogs are not an Android document picker;
round-tripping projects through the system picker is a follow-up. Use the
bundled pattern for the first test.
