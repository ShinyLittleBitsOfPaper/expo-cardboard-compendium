#!/usr/bin/env bash
# Cross-compile libcoqe for the Expo module. Needs rustup + cargo-ndk (Android) / Xcode (iOS). NOT run in CI yet.
set -euo pipefail
cd "$(dirname "$0")/.."
M=modules/expo-scryfall-engine
case "${1:-}" in
  android)
    rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
    cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o $M/android/src/main/jniLibs build --release -p coqc-engine
    ;;
  ios)
    rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
    for t in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do cargo build --release -p coqc-engine --target $t; done
    lipo -create target/aarch64-apple-ios-sim/release/libcoqe.a target/x86_64-apple-ios/release/libcoqe.a -output target/libcoqe-sim.a
    rm -rf $M/ios/libcoqe.xcframework
    xcodebuild -create-xcframework -library target/aarch64-apple-ios/release/libcoqe.a -headers $M/ios/coqe.h \
      -library target/libcoqe-sim.a -headers $M/ios/coqe.h -output $M/ios/libcoqe.xcframework
    ;;
  *) echo "usage: $0 android|ios"; exit 1 ;;
esac
