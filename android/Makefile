ROOT=$(abspath ..)
ANDROID_HOME ?= $(HOME)/Android/Sdk
NDK_VERSION=30.0.16248370
export ANDROID_HOME
export ANDROID_NDK_HOME=$(ANDROID_HOME)/ndk/$(NDK_VERSION)
GRADLE=./gradlew --quiet


all: check build


# Builds ffi/ for both CPUs into app/src/main/jniLibs/, which is git-ignored.
ffi:
	cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o app/src/main/jniLibs build -p keryx-ffi --release


check: ffi
	cargo clippy -p keryx-ffi --no-deps -- -D warnings
	cargo fmt -p keryx-ffi --check
	$(MAKE) -C ../moq lint
	$(GRADLE) ktlintCheck lintDebug


fmt:
	cargo fmt -p keryx-ffi
	$(GRADLE) ktlintFormat


build: ffi
	$(GRADLE) assembleDebug


# Installs on the phone over adb, which reaches this machine's keryx serve at 127.0.0.1:7778.
install: build
	adb reverse tcp:7778 tcp:7778
	adb install -r app/build/outputs/apk/debug/app-debug.apk


clean:
	$(GRADLE) clean
	rm -rf app/src/main/jniLibs


.PHONY: all ffi check fmt build install clean
