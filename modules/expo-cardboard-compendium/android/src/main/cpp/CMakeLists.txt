cmake_minimum_required(VERSION 3.18)
project(coqe_jni C)
set(COQE_SO ${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libcoqe.so)
if(NOT EXISTS ${COQE_SO})
  message(FATAL_ERROR "libcoqe.so missing for ABI ${ANDROID_ABI} (${COQE_SO}); build it with scripts/build-native.sh android")
endif()
add_library(coqe SHARED IMPORTED)
set_target_properties(coqe PROPERTIES IMPORTED_LOCATION ${COQE_SO})
# The Rust cdylib has no SONAME. Without this, CMake records the build machine's absolute path to the
# file as libcoqe_jni.so's DT_NEEDED entry and dlopen fails on the device ("library /home/.../libcoqe.so not found").
set_target_properties(coqe PROPERTIES IMPORTED_NO_SONAME TRUE)
add_library(coqe_jni SHARED coqe_jni.c)
target_link_libraries(coqe_jni coqe)
