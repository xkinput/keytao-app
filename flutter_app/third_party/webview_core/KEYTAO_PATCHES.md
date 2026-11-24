# webview_core 1.1.0

The [pub.dev release archive](https://pub.dev/api/archives/webview_core-1.1.0.tar.gz) has SHA-256 `fd08e38644efef31932d73bf0b2e2ec0e5f76d37cedd26adc2de3df24f03f84e`.
It contains upstream commit [`194c44e`](https://github.com/infradise/webview_core/commit/194c44e14114fea6a4441511689ffd611811c056).
The package retains the upstream license, notice, and source. Examples and upstream release documentation are omitted.

## KeyTao patches

- Limit the Flutter debug-binding check to debug builds. Flutter only sets the debug binding type in assertions, so the original check suppresses native lifecycle events and navigation decisions in profile and release builds.
- Match the Android Gradle plugin and Kotlin versions used by the KeyTao Android app. The upstream Android Gradle plugin requires a newer Gradle wrapper.
- Include the WebView2 SDK license and notice in Windows builds with the statically linked loader.

Use the upstream package directly once it supports release navigation callbacks and KeyTao's Android build toolchain.
