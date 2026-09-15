#!/usr/bin/env bash
set -euo pipefail
ndk_bin="${ANDROID_NDK_ROOT:?Set ANDROID_NDK_ROOT}/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ndk_bin/aarch64-linux-android26-clang"
export CC_aarch64_linux_android="$ndk_bin/aarch64-linux-android26-clang"
export AR_aarch64_linux_android="$ndk_bin/llvm-ar"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$ndk_bin/armv7a-linux-androideabi26-clang"
export CC_armv7_linux_androideabi="$ndk_bin/armv7a-linux-androideabi26-clang"
export AR_armv7_linux_androideabi="$ndk_bin/llvm-ar"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$ndk_bin/x86_64-linux-android26-clang"
export CC_x86_64_linux_android="$ndk_bin/x86_64-linux-android26-clang"
export AR_x86_64_linux_android="$ndk_bin/llvm-ar"
for pair in aarch64-linux-android:arm64-v8a armv7-linux-androideabi:armeabi-v7a x86_64-linux-android:x86_64; do
  target="${pair%%:*}"
  abi="${pair##*:}"
  cargo build --release --locked -p local-android --target "$target"
  mkdir -p "apps/android/app/src/main/jniLibs/$abi"
  cp "target/$target/release/liblocal_android.so" "apps/android/app/src/main/jniLibs/$abi/"
done

