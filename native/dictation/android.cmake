# whisper-rs builds C++ using CMake rather than cargo-ndk. Match the Android app.
set(ANDROID_ABI arm64-v8a CACHE STRING "Android ABI")
set(ANDROID_PLATFORM android-26 CACHE STRING "Minimum Android API")
set(ANDROID_STL c++_shared CACHE STRING "Packaged C++ runtime")
include("$ENV{NDK_HOME}/build/cmake/android.toolchain.cmake")
