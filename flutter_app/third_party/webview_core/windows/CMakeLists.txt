cmake_minimum_required(VERSION 3.18)

set(PROJECT_NAME "webview_core")
project(${PROJECT_NAME} LANGUAGES CXX)

cmake_policy(VERSION 3.18...3.28)

set(PLUGIN_NAME "webview_core_plugin")

set(WEBVIEW2_VERSION "1.0.3650.58")
set(WEBVIEW2_SHA256 "911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60")
set(WEBVIEW2_CACHE_ROOT "${CMAKE_CURRENT_BINARY_DIR}/webview2_sdk_${WEBVIEW2_VERSION}")
set(WEBVIEW2_NUPKG "${WEBVIEW2_CACHE_ROOT}/Microsoft.Web.WebView2.${WEBVIEW2_VERSION}.nupkg")
set(WEBVIEW2_EXTRACT_DIR "${WEBVIEW2_CACHE_ROOT}/extracted")

if(NOT EXISTS "${WEBVIEW2_EXTRACT_DIR}/build/native/include/WebView2.h")
  file(MAKE_DIRECTORY "${WEBVIEW2_CACHE_ROOT}")
  file(DOWNLOAD
    "https://www.nuget.org/api/v2/package/Microsoft.Web.WebView2/${WEBVIEW2_VERSION}"
    "${WEBVIEW2_NUPKG}"
    EXPECTED_HASH SHA256=${WEBVIEW2_SHA256}
    TLS_VERIFY ON
  )
  file(ARCHIVE_EXTRACT
    INPUT "${WEBVIEW2_NUPKG}"
    DESTINATION "${WEBVIEW2_EXTRACT_DIR}"
  )
endif()

if(CMAKE_VS_PLATFORM_NAME)
  string(TOLOWER "${CMAKE_VS_PLATFORM_NAME}" _WV2_TARGET_ARCH)
else()
  string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" _WV2_TARGET_ARCH)
endif()

if(_WV2_TARGET_ARCH MATCHES "arm64|aarch64")
  set(WEBVIEW2_ARCH "arm64")
elseif(_WV2_TARGET_ARCH MATCHES "x86$|win32|i386|i686")
  set(WEBVIEW2_ARCH "x86")
else()
  set(WEBVIEW2_ARCH "x64")
endif()

set(WEBVIEW2_INCLUDE_DIR "${WEBVIEW2_EXTRACT_DIR}/build/native/include")
set(WEBVIEW2_STATIC_LIB "${WEBVIEW2_EXTRACT_DIR}/build/native/${WEBVIEW2_ARCH}/WebView2LoaderStatic.lib")

list(APPEND PLUGIN_SOURCES
  "webview_core_plugin.cpp"
)

add_library(${PLUGIN_NAME} SHARED
  "include/webview_core/webview_core_plugin_c_api.h"
  ${PLUGIN_SOURCES}
)

apply_standard_settings(${PLUGIN_NAME})

set_target_properties(${PLUGIN_NAME} PROPERTIES
  CXX_VISIBILITY_PRESET hidden
)

target_compile_definitions(${PLUGIN_NAME} PRIVATE
  FLUTTER_PLUGIN_IMPL
  NOMINMAX
  WIN32_LEAN_AND_MEAN
)

target_include_directories(${PLUGIN_NAME} INTERFACE
  "${CMAKE_CURRENT_SOURCE_DIR}/include"
)

target_include_directories(${PLUGIN_NAME} PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/include"
  "${WEBVIEW2_INCLUDE_DIR}"
)

target_link_libraries(${PLUGIN_NAME} PRIVATE
  flutter
  flutter_wrapper_plugin
  "${WEBVIEW2_STATIC_LIB}"
  comctl32
  d3d11
  dxgi
  ole32
  runtimeobject
  shlwapi
  user32
  version
)

# KeyTao: install the redistribution notices for the statically linked loader.
foreach(WEBVIEW2_NOTICE IN ITEMS LICENSE.txt NOTICE.txt)
  set(WEBVIEW2_NOTICE_DESTINATION "${CMAKE_CURRENT_BINARY_DIR}/WebView2Loader-${WEBVIEW2_NOTICE}")
  configure_file("${WEBVIEW2_EXTRACT_DIR}/${WEBVIEW2_NOTICE}" "${WEBVIEW2_NOTICE_DESTINATION}" COPYONLY)
  list(APPEND WEBVIEW2_NOTICES "${WEBVIEW2_NOTICE_DESTINATION}")
endforeach()
set(webview_core_bundled_libraries "${WEBVIEW2_NOTICES}" PARENT_SCOPE)
