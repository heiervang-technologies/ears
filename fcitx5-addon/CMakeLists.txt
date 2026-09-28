cmake_minimum_required(VERSION 3.16)
project(fcitx5-ears-ghost VERSION 0.1.0 LANGUAGES CXX)

find_package(Fcitx5Core REQUIRED)
find_package(Fcitx5Utils REQUIRED)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_library(earsghost SHARED earsghost.cpp)
target_link_libraries(earsghost PRIVATE Fcitx5::Core Fcitx5::Utils)
set_target_properties(earsghost PROPERTIES PREFIX "lib")
