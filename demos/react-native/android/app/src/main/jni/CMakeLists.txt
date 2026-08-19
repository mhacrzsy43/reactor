cmake_minimum_required(VERSION 3.13)
project(appmodules)

# Keep React Native's complete New Architecture entry point, then compile the
# Reactor JSI diagnostic registration into the same app library.
include(${REACT_ANDROID_DIR}/cmake-utils/ReactNative-application.cmake)
