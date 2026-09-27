# KeyTao 增量 1 构建计划（Flutter 原地替换 Tauri，Android + macOS）

**先回答用户**：现在的 flutter_app 只是 Phase 0 冒烟页，所以看起来像测试器。原因有三个：
- `flutter_app/lib/main.dart:52-62` 用 `systemTemp` 加 `userRootOverride` 初始化 core。
- applicationId 是 `ink.rea.keytao`（`flutter_app/android/app/build.gradle.kts:19`）。
- 因此它碰不到真实的 `/sdcard/keytao` 和 `~/Library/keytao`。

增量 1 把它改成真正的 KeyTao App：
- Android 用同一个包名 `ink.rea.keytao_app` 覆盖安装。
- 键盘、候选窗、部署服务的 Kotlin 原样搬过来，不重写。
- macOS 替换 `/Applications/KeyTao.app`，输入法本体 pkg 不动。

**每个代理都要遵守**：
- 只做核心功能，不加说明小字。
- 除了 KeytaoNativeBridge.kt 那一行，不改 IME 的 Kotlin 行为。
- 只写下表里标明的测试。
- 验收时不要反复调用方案下载接口。

## (1) Android 工程接管（flutter_app/android）

| # | 目标 | 文件 | 验收（本机可跑） |
|---|---|---|---|
| A1 | 包身份和构建配置 | `app/build.gradle.kts`：<br>- `namespace`(:8) 和 `applicationId`(:19) 改为 `ink.rea.keytao_app`<br>- compileSdk 36 / minSdk 24 / targetSdk 36 写死，不用 flutter.*（:9,:22,:23）；保留 ndk 27.0.12077973<br>- 显式加 `id("org.jetbrains.kotlin.android")`<br>- 依赖：core-ktx、documentfile、junit 4.13.2、org.json 20240303<br>- release 签名读 `keystore.properties`（keyAlias/password/storeFile），文件不存在时退回 debug 签名（旧 :31-43）<br>- 开 minify，接上从 `proguard-rules.pro:1-7` 移植的规则，删掉 app.tauri 行<br>- 删 `kotlin/ink/rea/keytao/MainActivity.kt`，在 `ink/rea/keytao_app/` 下建一个空的 `MainActivity : FlutterActivity`<br>- `scripts/sync-version.mjs` 同时写 `pubspec.yaml` 的 version，公式与 `tauri.properties:2-3` 一致 | `node scripts/sync-version.mjs && flutter build apk --debug`；`aapt2 dump badging` 显示 `package=ink.rea.keytao_app`，versionCode ≥ 1002001 |
| A2 | 原样搬迁 IME、部署服务和运行时代码 | - 把 25 个 kt 从 `src-tauri/gen/android/.../ink/rea/keytao_app/` 移到 `app/src/main/kotlin/ink/rea/keytao_app/`，唯一改动是 `KeytaoNativeBridge.kt:8` 改成 `"keytao_app_bridge"`<br>- 14 个单测一起移过来<br>- res 移过来：`values/strings.xml`、`xml/keytao_input_method.xml`、`xml/file_paths.xml`、`raw/keytao_android_ime.json`、启动图标<br>- 清单原样搬：权限（旧 :3-9）、`requestLegacyExternalStorage`（:15）、IME 服务（:36-48）、部署服务（:50-55）、FileProvider（:57-65）；label 改 `@string/app_name`；Activity 改 singleTask + adjustResize，删掉 `taskAffinity=""`<br>- MainActivity 的 onCreate/onResume 保留三个钩子：`KeytaoStorageMigration.start`、`KeytaoAndroidPaths.retryResolution`、`KeytaoRuntimeLog.adoptAppLoggerIfEnabled`（旧 MainActivity.kt:9-29） | - `./gradlew :app:testDebugUnitTest` 全绿<br>- `flutter build apk --release --split-per-abi` 成功<br>- 符号核对：`llvm-nm -D` 从 so 里取出的 `KeytaoNativeBridge_*` 名单，与 `external fun` 名单 diff 为空（43 个，名单见下）<br>- `aapt2 dump xmltree` 能看到 `.KeytaoInputMethodService`（`:ime`）、`.KeytaoRimeDeployService`（`:rime_deployer`）、`ink.rea.keytao_app.fileprovider`、`subtypeId 0x0a01b2c3`、`settingsActivity=ink.rea.keytao_app.MainActivity`<br>- `grep -r app.tauri flutter_app/android` 为 0 |
| A3 | APK assets | - `build.gradle.kts` 用 `sourceSets.main.assets.srcDirs` 加入同步出的目录和 addon-schemas<br>- `scripts/android-librime-runtime.sh` 加一个只同步 assets 的模式，不写 jniLibs（:782-787）<br>- preBuild 同步任务只移植 assets 部分（旧 :91-110） | - `unzip -l` 能看到 `assets/keytao-rime-data/`、`assets/keytao-rime-runtime.txt`、`assets/addon-schemas/easy_en/`<br>- `lib/arm64-v8a/librime.so` 只有 1 份<br>- mergeNativeLibs 没有重复报错 |

**关于符号数量**：任务里写的是 67 个，实测是 43 个。`KeytaoNativeBridge.kt` 有 43 个 `external fun`，`android_jni/mod.rs` 有 43 个 `no_mangle`，两边一致。旧 so 多出的 22 个都是 Tauri/wry 胶水。67 这个数找不到出处，以 43 为准。前缀都是 `Java_ink_rea_keytao_1app_KeytaoNativeBridge_`：

```
nativeLogEnabled nativeLogEvent nativeLogFlush nativeResolveThemeJson nativeWriteThemeUi nativeDefaultKeyboardYaml nativeResolveKeyboardJson nativeEngineAvailable nativeDeployStep nativeInit nativeReinitialize nativeCreateSession nativeDestroySession nativeSessionState nativeProcessKey nativeProcessEnter nativeSelectCandidate nativeHighlightCandidate nativeDeleteCandidate nativeCandidateIsUserPhrase nativeSelectCandidateGlobal nativeAllCandidates nativeListSchemas nativeSchemaSwitches nativeCurrentSchema nativeSelectSchema nativeGetOption nativeSetOption nativeChangePage nativeReset nativeCommitComposition nativeClearComposition nativeSetInputPolicy nativeInputPolicyComposing nativeInputPolicyLearning nativeGetAsciiMode nativeSetAsciiMode nativeTextToKeysym nativeIsEnterKey nativeShouldBypassKey nativeUtf16OffsetFromChars nativeReloadStampSignature nativeReloadStampPath
```

## (2) 桥接 API 补齐（crates/keytao-app-bridge）

| # | 目标 | 文件 | 验收 |
|---|---|---|---|
| B1 | 部署 | - `api/core.rs` 加 `deploy_default()`：macOS 走 `keytao_app_core::deploy::rime_deploy_default`，并发出 DeployProgress 事件<br>- `host.rs:42-51` 的 `deploy_paths` 改为调用 `macos_app_shared_data_dir`<br>- Android 部署不经过桥接，由 Dart 调通道 `deployImeData`（与 `ime_shell.rs:21-25` 同路径） | `cargo test -p keytao-app-bridge`（新增 1 个用例，测共享数据候选顺序）；`flutter_rust_bridge_codegen generate` 后 `flutter analyze` 无错 |
| B2 | 安装 | - 新增 `fetch_latest_release()`（core `scheme.rs:33`）<br>- 新增 `install_scheme_from_url(url)`（对应 lib.rs:876）<br>- Android 拆成 `prepare_android_install` 和 `finish_android_install`，签名照搬 `lib.rs:884-910` 的两次 core 调用<br>- InstallProgress 复用 `types.rs:61-74` | 同上 |
| B3 | 设置 | - `get/set_android_ime_input_settings`，DTO 带全部 23 个字段（ime_settings.rs:410-470）<br>- `get/set_ime_ui_settings`、`set_ime_embedded_composition`、`get/set_desktop_english_mode`<br>- `addon_schema_status` 只读 | cargo test：1 个用例，get → 改 1 项 → set → get，其余 22 项不变 |
| B4 | 状态、关于、日志 | - `macos_ime_status`（ime_status/macos.rs:60-122）<br>- `get_component_versions`，去掉 Tauri 那一行<br>- `get/set_runtime_log_settings`、`read_runtime_log`、`clear_runtime_log` | cargo test + codegen + `flutter analyze` |

## (3) Android 平台通道

| # | 目标 | 文件 | 验收 |
|---|---|---|---|
| C1 | `keytao/android` MethodChannel | - 新建 `KeytaoAndroidChannel.kt`，从 `ScopedStoragePlugin.kt` 拆出来，去掉 Tauri 注解，在 `MainActivity.configureFlutterEngine` 里注册<br>- 方法：`imeStatus`(:74)、`keytaoRoot`(:83)、`appDataDir`、`storagePermissionStatus`(:279，含 migrationError/deployError)、`openStoragePermissionSettings`(:291，用 onActivityResult 触发 retryResolution)、`openInputMethodSettings`(:340)、`showInputMethodPicker`(:352)、`smartExtractZipToPrivate`(:722)、`deployImeData`(:890，进度用 invokeMethod 回主线程)、`adoptAppLogger`(init_core 之后调)<br>- 删除 openApp 和 writeImeReloadStamp；其余方法留到第二步<br>- Dart 侧对应 `lib/src/platform/android_host.dart` | `./gradlew :app:compileReleaseKotlin`；`grep -rn '@TauriPlugin\|app.tauri' flutter_app/android` 为 0；`flutter analyze` |

## (4) Flutter UI

| # | 目标 | 文件 | 验收 |
|---|---|---|---|
| D1 | 启动和外壳 | - 替换 `main.dart:52-62` 的测试器<br>- macOS 的 BridgeConfig：dataDir 为 `~/Library/Application Support/ink.rea.keytao-app`，cacheDir 为 `~/Library/Caches/ink.rea.keytao-app`，resourceDir 为可执行文件旁边的 `../Resources`，不设 override<br>- Android：用 `keytaoRoot` 作 override，dataDir 与 Tauri 的 app_data_dir 同一位置<br>- 外壳：Header（带版本号）+ 三个标签页（输入法 / 关于 / 调试）<br>- 配色取自 `KeytaoTheme.kt`，分亮色和暗色 | `flutter analyze`；`grep -n systemTemp lib` 为 0；`flutter build macos --debug` 跑起来后，目录显示 `~/Library/keytao` |
| D2 | 分页引导 | - `lib/src/onboarding/`，用 PageView<br>- Android 页序：存储权限 → 数据迁移 → 装方案 → 部署 → 启用 KeyTao → 切换到 KeyTao → 完成<br>- macOS 页序：检测 IME 组件 → 装方案 → 部署 → 完成（提示注销后在系统设置里添加）<br>- 回到前台（resumed）时重查状态<br>- 只在 `onboarding().completed == false` 时显示；全部就绪后调 `complete_onboarding`<br>- 老用户由 `runtime.rs:51` 的 initialize_onboarding 自动跳过 | `flutter test`：1 个用例，completed 为 true/false 时分别不进 / 进引导 |
| D3 | 方案页 | - 4 个方案按钮，自动选中逻辑同 App.tsx:96-105<br>- 键道6 的 GitHub/Gitee 来源切换<br>- 版本、刷新、目录、本地状态<br>- 安装：Android 走 prepare → smartExtractZipToPrivate → finish；macOS 走 `install_scheme_from_url`<br>- 部署（显示步骤）、检查本地、打开目录（仅 macOS）、测试输入框、操作日志弹窗 | `flutter analyze`；owner 手测：安装 → 部署后 check_local_schema 为 installed && deployed |
| D4 | 键盘设置和候选窗设置 | - Android 键盘设置 11 项，限幅同 App.tsx:1456-1473，带恢复默认；保存时回写完整 23 字段<br>- 英文模式：easy_en 未就绪时只能选 ASCII<br>- macOS 候选窗：配色方案、横排/竖排、英文模式、嵌入模式、字号 10-36、4 个主题色预设、打开主题文件 | `flutter test`：1 个用例，改 1 项后提交的 DTO 里其余字段不变 |
| D5 | 关于和调试页 | - 关于：版本表（KeyTao / Flutter / librime / OpenCC）、平台、键道目录、GitHub 链接<br>- 调试：运行日志开关和级别、刷新、清空、最近 N 行 | `flutter analyze` |

## (5) macOS 工程

| # | 目标 | 文件 | 验收 |
|---|---|---|---|
| E1 | 身份和沙箱 | - `AppInfo.xcconfig:8,11` 设 `PRODUCT_NAME=KeyTao`、`PRODUCT_BUNDLE_IDENTIFIER=ink.rea.keytao-app`<br>- `project.pbxproj:398,412,426` 改 RunnerTests 的 id<br>- 两个 entitlements 都设 `app-sandbox=false` 和 `cs.disable-library-validation`；Debug 另加 `cs.allow-jit` | `flutter build macos`；PlistBuddy 读出 CFBundleIdentifier 正确；`codesign -d --entitlements -` 里没有打开沙箱 |
| E2 | 资源和 dylib 布局 | - Runner 加一个 Run Script，用 ditto 拷 `rime-data`、`addon-schemas` 到 `Contents/Resources`<br>- `librime.1.dylib` 平铺到 `Frameworks/`，lua/octagram 插件放 `Frameworks/rime-plugins/`，并做 `install_name_tool -id`（同 build-macos.sh:94-125）<br>- 删掉 `hook/build.dart:54-56` 的 macOS bundledLibraries | `otool -L .../keytao_app_bridge.framework/keytao_app_bridge` 显示 `@rpath/librime.1.dylib`；`ls Contents/Resources/rime-data` 和 `Frameworks/rime-plugins/librime-lua.dylib` 都存在；没有 `librime.framework` |

## (6) 给 owner 的构建产物

| # | 目标 | 文件 | 验收 |
|---|---|---|---|
| F1 | 可覆盖安装的 Android APK | 把 `release.yml:223-333` 改成 Flutter 流程：<br>- NDK 27.0.12077973、Rust android targets、导入 librime<br>- 只同步 assets<br>- keystore 写到 `flutter_app/android`<br>- `flutter build apk --release --split-per-abi`<br>- 改重命名和上传步骤（:322-333） | `aapt2 dump badging` 核对包名和 versionCode；`apksigner verify --print-certs` 的证书 SHA-256 与现网 APK 相同 |
| F2 | macOS pkg | - `build-macos.sh:127-133` 换成 `flutter build macos --build-name <工作区版本号>` + ditto<br>- `:208-215` 的重签顺序改为：rime-plugins → 各个 framework → app<br>- 用 PlistBuddy 写版本号，要 ≥ 1.2.1-alpha.84<br>- `verify-macos-pkg.sh:98` 的可执行文件名改成 KeyTao | `scripts/verify-macos-pkg.sh <pkg>` 通过；`codesign --verify --deep --strict` 通过 |

**顺序**：
- A1 → A2 → A3。
- B1 到 B4、E1 到 E2 与 A 系列改的是不同文件，可以并行。
- C1 等 A2 完成后再做。
- D1 等 B4 和 C1；D2 到 D5 等 D1，其中 D3 还依赖 B1 和 B2。
- F1、F2 最后做。

## 未解决风险

| 风险 | 后果 / 防线 |
|---|---|
| 本机没有 `keystore.properties`，本地出的 release 包是 debug 签名 | 覆盖安装报 UPDATE_INCOMPATIBLE。卸载重装会丢掉输入法启用状态和全部文件访问授权（/sdcard/keytao 里的数据还在）。只有 CI 出的包能覆盖安装 |
| Android 的 dataDir 和 Tauri 的 app_data_dir 不一致 | app-state.json 延续不下来。旧的 WebView localStorage（引导标记、登录 token）Flutter 读不到，老用户跳过引导只靠 initialize_onboarding，账号要重新登录 |
| 编译栈一次跳很多：Kotlin 1.9.25 → 2.4.0，JVM 1.8 → 17，AGP 8.11 → 9.1 | 17.6k 行 Kotlin 第一次编译可能报错，由 A2 暴露 |
| `:ime` 和 `:rime_deployer` 进程也会创建 Application，并运行插件的 ContentProvider 和 androidx.startup | 新加 Flutter 插件前先查它的清单里有没有 provider |
| FlutterActivity 不是 ComponentActivity | 权限和设置回调只能用 onRequestPermissionsResult / onActivityResult |
| macOS pkg 覆盖升级：可执行文件名从 keytao-app 变成 KeyTao，旧 dylib 可能残留 | 签名封印可能损坏，要在装过 Tauri pkg 的机器上实测覆盖安装 |
| 沙箱没关 | 部署进容器目录，IME 继续用旧方案且不报错。用 E1 验收加 reload stamp 的 mtime 核对 |

## 需要 owner 拍板（3 项）

1. **发布签名**：只在 CI 出包（先做 F1），还是把 release keystore 放到本机 `keystore.properties`，让本地也能出可以覆盖安装的 APK？
2. **平台下限**：是否接受放弃 32 位 x86 APK（`release.yml:314` 的 i686），以及 macOS 10.13–11？Flutter 的 macOS 最低版本是 12.0（`project.pbxproj:474`），旧版是 10.13。
3. **键道6 来源切换**：GitHub/Gitee 切换是否放进增量 1？要放就需要 B2 的 `fetch_latest_release` 和按 URL 安装。不放的话只用 `fetch_scheme_release` + `install_latest_scheme`，B2 只剩 Android 的 prepare/finish。

主要文件：`/Users/rea/code/keytao-org/keytao-app/flutter_app/lib/main.dart`、`/Users/rea/code/keytao-org/keytao-app/src-tauri/gen/android/app/src/main/java/ink/rea/keytao_app/KeytaoNativeBridge.kt`、`/Users/rea/code/keytao-org/keytao-app/crates/keytao-app-bridge/src/api/core.rs`
## 项目方决定（2026-09-28）
1. 签名：用本机 debug 签名出包，覆盖安装手机上现有的本机 debug 版（release 签名配置保留为可选：keystore.properties 存在才用）。
2. 键道6 的 GitHub / Gitee 来源切换放进增量 1（B2 的 fetch_latest_release + 按 URL 安装都要做）。
3. 接受：不出 32 位 x86 APK；Mac App 最低 macOS 12。
4. 迁移期间 Android 的 Kotlin 源码从 src-tauri/gen/android **复制**到 flutter_app/android，不删除旧工程；新版验收后再退役 Tauri Android 工程。
