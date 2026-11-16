#!/usr/bin/env bash
# Puts a pinned Android NDK on the job: unpacked once into $HOME/.cache (kept between runs on the
# self-hosted runners), sha256-verified, and exported as ANDROID_NDK_HOME through $GITHUB_ENV.
# Used by ci.yml (`cargo xtask android check`) and release.yml (the Android build).
set -euo pipefail
v="${ANDROID_NDK_VERSION:-r28c}"
sum="${ANDROID_NDK_SHA256:-dfb20d396df28ca02a8c708314b814a4d961dc9074f9a161932746f815aa552f}"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) ;;
  *) echo "::error::the Android NDK step supports Linux x86_64 runners only"; exit 1 ;;
esac
cache="${XDG_CACHE_HOME:-$HOME/.cache}"
dir="$cache/android-ndk-$v"
if [ ! -f "$dir/source.properties" ]; then
  mkdir -p "$cache"
  tmp="$(mktemp -d "${RUNNER_TEMP:-/tmp}/ndk.XXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  curl -sSfLo "$tmp/ndk.zip" "https://dl.google.com/android/repository/android-ndk-$v-linux.zip"
  echo "$sum  $tmp/ndk.zip" | sha256sum -c -
  unzip -q "$tmp/ndk.zip" -d "$tmp/out"
  rm -rf "$dir"
  /bin/mv -f "$tmp/out/android-ndk-$v" "$dir"
fi
grep Pkg.Revision "$dir/source.properties"
if [ -n "${GITHUB_ENV:-}" ]; then echo "ANDROID_NDK_HOME=$dir" >> "$GITHUB_ENV"; else echo "ANDROID_NDK_HOME=$dir"; fi
