cmake_minimum_required(VERSION 3.22)
project(opus_jni)

set(OPUS_BUILD_PROGRAMS OFF CACHE BOOL "" FORCE)
set(OPUS_BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(OPUS_BUILD_SHARED_LIBRARY OFF CACHE BOOL "" FORCE)
set(OPUS_X86_PRESUME_SSE OFF CACHE BOOL "" FORCE)
add_subdirectory(opus)

add_library(opus_jni SHARED opus_jni.cpp)
target_include_directories(opus_jni PRIVATE opus/include)
target_link_libraries(opus_jni PRIVATE opus log)
