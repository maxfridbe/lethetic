#!/usr/bin/env python3
"""Configure Whisper's Android C++ build and package the matching NDK runtime."""
import os
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]


def prepare(ndk: Path, generated: Path) -> dict[str, str]:
    candidates = list((ndk / "toolchains/llvm/prebuilt").glob("*/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"))
    if len(candidates) != 1:
        raise ValueError("Expected one ARM64 libc++_shared.so in NDK_HOME")
    destination = generated / "app/src/main/jniLibs/arm64-v8a/libc++_shared.so"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(candidates[0], destination)
    sysroot = candidates[0].parents[3]
    return {
        "CMAKE_TOOLCHAIN_FILE": str(ROOT / "native/dictation/android.cmake"),
        "BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android": f"--sysroot={sysroot} -I{sysroot}/usr/include/aarch64-linux-android -D__ANDROID_API__=26",
    }


if __name__ == "__main__":
    values = prepare(Path(os.environ["NDK_HOME"]), ROOT / "native/src-tauri/gen/android")
    if output := os.environ.get("GITHUB_ENV"):
        with open(output, "a", encoding="utf-8") as stream:
            for key, value in values.items():
                stream.write(f"{key}={value}\n")
    for key, value in values.items():
        print(f"{key}={value}")
