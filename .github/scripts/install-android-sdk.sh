#!/usr/bin/env bash
# Puts what `cargo xtask android package` needs on the job, cached in $HOME/.cache between runs on the
# self-hosted runners (every download pinned by sha256):
# - a JDK 17+ (the runner's own, else Temurin 21) -> JAVA_HOME
# - the Android SDK command-line tools, then platform android-36 and build-tools 36.0.0 -> ANDROID_HOME
# The NDK comes from install-android-ndk.sh. Exports through $GITHUB_ENV (printed when run locally).
set -euo pipefail
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) ;;
  *) echo "::error::the Android SDK step supports Linux x86_64 runners only"; exit 1 ;;
esac
cache="${XDG_CACHE_HOME:-$HOME/.cache}"
mkdir -p "$cache"
tmp="$(mktemp -d "${RUNNER_TEMP:-/tmp}/sdk.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
export_var() { if [ -n "${GITHUB_ENV:-}" ]; then echo "$1=$2" >> "$GITHUB_ENV"; else echo "$1=$2"; fi; }
fetch() { # url sha256 dest
  curl -sSfLo "$3" "$1"
  echo "$2  $3" | sha256sum -c -
}

# JDK
java_major() { "$1" -version 2>&1 | sed -n 's/.*version "\([0-9]*\).*/\1/p' | head -1; }
jdk=""
if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/javac" ] && [ "$(java_major "$JAVA_HOME/bin/java")" -ge 17 ]; then
  jdk="$JAVA_HOME"
else
  jdk="$cache/temurin-21.0.8+9"
  if [ ! -x "$jdk/bin/javac" ]; then
    fetch "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.8%2B9/OpenJDK21U-jdk_x64_linux_hotspot_21.0.8_9.tar.gz" \
      f2dc5418092c43003db8f9005c4a286e1c0104fea96ccdd49e8ebd037cac9219 "$tmp/jdk.tgz"
    mkdir -p "$tmp/jdk" && tar -xzf "$tmp/jdk.tgz" -C "$tmp/jdk"
    rm -rf "$jdk" && /bin/mv -f "$tmp/jdk/"jdk-* "$jdk"
  fi
fi
"$jdk/bin/java" -version 2>&1 | head -1
export_var JAVA_HOME "$jdk"

# SDK
sdk="${ANDROID_SDK_CACHE:-$cache/android-sdk}"
tools="$sdk/cmdline-tools/latest/bin"
if [ ! -x "$tools/sdkmanager" ]; then
  fetch https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip \
    7ec965280a073311c339e571cd5de778b9975026cfcbe79f2b1cdcb1e15317ee "$tmp/clt.zip"
  unzip -q "$tmp/clt.zip" -d "$tmp/clt"
  mkdir -p "$sdk/cmdline-tools" && rm -rf "$sdk/cmdline-tools/latest"
  /bin/mv -f "$tmp/clt/cmdline-tools" "$sdk/cmdline-tools/latest"
fi
if [ ! -x "$sdk/build-tools/36.0.0/aapt2" ] || [ ! -f "$sdk/platforms/android-36/android.jar" ]; then
  yes | JAVA_HOME="$jdk" "$tools/sdkmanager" --sdk_root="$sdk" --licenses >/dev/null || true
  JAVA_HOME="$jdk" "$tools/sdkmanager" --sdk_root="$sdk" "platforms;android-36" "build-tools;36.0.0"
fi
export_var ANDROID_HOME "$sdk"
