#!/bin/bash

export API=25

WORKDIR="$(pwd)/"build
export WORKDIR

ROOT_DIR="$(pwd)/../.."

mkdir -p build

export ANDROID_NDK_ZIP=${WORKDIR}/android-ndk-r25c.zip
export ANDROID_NDK_ROOT=${WORKDIR}/android-ndk-r25c
ANDROID_NDK_SHA256="53af80a1cce9144025b81c78c8cd556bff42bd0e"
if [ ! -e "$ANDROID_NDK_ZIP" ]; then
  curl https://dl.google.com/android/repository/android-ndk-r25c-linux.zip -o "${ANDROID_NDK_ZIP}"
fi
echo $ANDROID_NDK_SHA256 "$ANDROID_NDK_ZIP" | sha1sum -c || exit 1
unzip "$ANDROID_NDK_ZIP" -d "$WORKDIR"

export ANDROID_NDK_HOME=$ANDROID_NDK_ROOT

export TOOLCHAIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64
export TARGET=aarch64-linux-android
export API=33
export AR=$TOOLCHAIN/bin/llvm-ar
export CC=$TOOLCHAIN/bin/$TARGET$API-clang
export AS=$CC
export CXX=$TOOLCHAIN/bin/$TARGET$API-clang++
export LD=$TOOLCHAIN/bin/ld
export RANLIB=$TOOLCHAIN/bin/llvm-ranlib
export STRIP=$TOOLCHAIN/bin/llvm-strip
export PATH=$PATH:$ANDROID_HOME/cmdline-tools/latest/bin
export PATH=$PATH:$TOOLCHAIN/bin

# inject absolute linker path into cargo config
LINKER_PATH="$TOOLCHAIN"/bin/aarch64-linux-android33-clang
cp -R .cargo build/
sed -i "s#replaceme#${LINKER_PATH}#g" build/.cargo/config

cp -R "$ROOT_DIR"/native/tor-ffi build/
cd build/tor-ffi || exit

# inject vendored openssl required for android cross compilation
sed -i "s/\[dependencies\]/\[dependencies\]\\
openssl = { version = \"0.10\", features = [\"vendored\"] }/" Cargo.toml

rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
cargo ndk -t armeabi-v7a -t arm64-v8a -t x86_64 -o "$ROOT_DIR"/android/src/main/jniLibs build --release
