# Android IME 实现说明

本文只记录 `src-tauri/gen/android/app` 里的 Android 系统输入法前端实现，并按当前代码同步。

跨平台通用契约见 [输入法通用层实现规范](../../../../docs/ime-common-layer.md)；本文只补充 Android `InputMethodService`、`InputConnection`、移动端键盘配置和 JNI bridge 的平台差异。

## 代码地图

- `src/main/AndroidManifest.xml`：注册主 App activity、`KeytaoInputMethodService` 和 FileProvider。
- `src/main/res/xml/keytao_input_method.xml`：Android input-method metadata，声明 `zh_CN` keyboard subtype 和设置页入口。
- `src/main/res/raw/keytao_android_ime.json`：内置移动端键盘布局、键位 hint、上下滑动作、数字/符号页和高度配置。
- `src/main/java/ink/rea/keytao_app/KeytaoInputMethodService.kt`：Android `InputMethodService` 前端，负责系统生命周期、硬键/软键分发、`InputConnection` 提交和打开 App 页面。
- `src/main/java/ink/rea/keytao_app/KeytaoImeEngine.kt`：Android 侧 engine facade，解析私有用户目录、shared data 目录、reload stamp，并通过 JNI 调用通用 runtime。
- `src/main/java/ink/rea/keytao_app/KeytaoNativeBridge.kt`：加载 `keytao_app_lib` 并包装 native JNI 方法。
- `src/main/java/ink/rea/keytao_app/AndroidKeyMapper.kt`：把 Android `KeyEvent` 转为 X11 keysym + Rime modifier mask。
- `src/main/java/ink/rea/keytao_app/KeytaoImeState.kt`：解析 Rust 返回的 `ImeState` JSON、通用 `CandidatePanelModel` 和 `ModeHintModel`。
- `src/main/java/ink/rea/keytao_app/KeytaoKeyboardView.kt`：自绘移动端键盘和候选栏，只消费 state、theme、panel model 和移动端键盘配置。
- `src/main/java/ink/rea/keytao_app/KeytaoTheme.kt`：把 `keytao-theme` 的 resolved JSON 映射到 Android `Paint` 需要的颜色、字号和尺寸。
- `src/main/java/ink/rea/keytao_app/KeytaoAndroidPaths.kt`：统一 Android 用户目录、主题、移动端配置和 reload stamp 路径。
- `src/main/java/ink/rea/keytao_app/KeytaoAndroidImeConfig.kt`：加载 `/storage/emulated/0/keytao/android_ime.json`，失败时 fallback 到内置 raw 配置。
- `src/main/java/ink/rea/keytao_app/ScopedStoragePlugin.kt`：Android 文件安装 adapter；默认把方案、主题和 reload stamp 写到 Android 用户存储的 `keytao` 目录。
- `src-tauri/src/lib.rs`：Android JNI bridge，直接创建 `keytao_core::ImeRuntime` / `ImeRuntimeSession`，并调用 `keytao-theme` 生成主题和 UI model JSON。
- `scripts/android-librime-runtime.sh`：Android ABI runtime 管理脚本，导入/校验 `librime.so` 闭包，并同步到 Gradle `jniLibs` 和 assets。
- `crates/librime-sys/build.rs`：本地 patched `librime-sys`，Android target 会按 ABI 自动查找 `vendor/librime/android/<abi>`，并要求 Android NDK sysroot。

Tauri 主 App 不处理 Android 输入法按键热路径。它负责下载安装方案、触发部署/reload stamp、展示状态和打开配置页面；系统输入由 Android `InputMethodService` 负责。

## Android 官方契约对齐点

Android 系统输入法必须通过 framework 注册和运行：

- `AndroidManifest.xml` 中的 service 使用 `android.permission.BIND_INPUT_METHOD`，intent filter 使用 `android.view.InputMethod`。
- `android.view.im` metadata 指向 `res/xml/keytao_input_method.xml`。
- service 继承 `InputMethodService`，通过 `onCreateInputView()` 返回输入法窗口 view。
- 文本提交、composition 和 editor action 只通过当前 `InputConnection` 操作。
- 不能直接写目标 App 的 View，也不能假设目标 App 一定支持完整 preedit。

参考：

- [Creating an input method](https://developer.android.com/develop/ui/views/touch-and-input/creating-input-method)
- [InputMethodService](https://developer.android.com/reference/kotlin/android/inputmethodservice/InputMethodService)
- [InputConnection](https://developer.android.com/reference/android/view/inputmethod/InputConnection)

## 系统注册

`AndroidManifest.xml` 当前注册：

```xml
<service
    android:name=".KeytaoInputMethodService"
    android:exported="true"
    android:label="@string/keytao_ime_name"
    android:permission="android.permission.BIND_INPUT_METHOD">
    <intent-filter>
        <action android:name="android.view.InputMethod" />
    </intent-filter>
    <meta-data
        android:name="android.view.im"
        android:resource="@xml/keytao_input_method" />
</service>
```

`keytao_input_method.xml` 声明：

- `settingsActivity="ink.rea.keytao_app.MainActivity"`，系统设置页可回到 KeyTao 主 App。
- `supportsSwitchingToNextInputMethod="true"`，允许系统输入法切换器切到下一个输入法。
- subtype 使用 `zh_CN` / `keyboard`，并声明 `isAsciiCapable=true`。

Android 不需要像 macOS TIS 或 Windows TSF 那样写系统注册表/输入源数据库；安装 APK 后，用户仍需要在系统输入法设置里启用 KeyTao 输入法。

主 App 的 `MainActivity` 声明 `android:windowSoftInputMode="adjustResize"`，并在 `onCreate()` 设置 `SOFT_INPUT_ADJUST_RESIZE`。Tauri WebView 侧还用 `visualViewport` 维护 `--android-ime-inset-bottom`，在 Android 软键盘显示时给页面底部增加滚动留白并把当前输入控件滚到可见区域。这个避让属于宿主 App 对 IME inset 的响应；IME service 本身只提供标准 input view 高度，不在输入法层伪造 App 布局。

## 初始化和进程模型

正常流程：

1. Android 系统按需创建 `KeytaoInputMethodService`。
2. `onCreate()` 创建 `KeytaoImeEngine(applicationContext)`。
3. `KeytaoImeEngine` 通过 `KeytaoAndroidPaths.userRoot()` 确保 `/storage/emulated/0/keytao` 存在。
4. 如果 APK assets 中存在 `keytao-rime-data/default.yaml`，先解包到 `/storage/emulated/0/keytao/rime-data`。
5. 查找 shared data 目录，要求目录中至少有 `default.yaml`。
6. `KeytaoNativeBridge.engineAvailable()` 确认 native library 已加载。
7. `nativeInit(userDir, sharedDir)` 在 Rust 侧创建 `ImeRuntime::with_dirs()` 并执行 `init()`。
8. `nativeCreateSession()` 创建独立 `ImeRuntimeSession`，service 持有 session handle。
9. `onDestroy()` 调用 `nativeDestroySession(session)`。

当前实现是一进程一个 service session。Android framework 通常一个输入法 service 同时服务当前 focus editor；如果后续要支持多 display、多窗口并发或更细粒度 input context，可以把 session 生命周期从 service 级下沉到 editor/input connection 级。

## 用户目录和 shared data

Android IME 用户目录固定为 Android 用户存储根目录下的 `keytao`：

```text
/storage/emulated/0/keytao
```

其中常见文件：

- `default.yaml`、`keytao.schema.yaml`、`*.dict.yaml`、`lua/`、`opencc/`：Rime 方案和运行时数据。
- `theme.yaml`：用户主题，交给通用 `keytao-theme` 解析。
- `android_ime.json`：移动端键盘布局和手势配置。
- `keytao-ime.reload`：App 部署后写入的 reload stamp。

`KeytaoImeEngine.findSharedDataDir()` 目前按顺序查找：

1. `/storage/emulated/0/keytao`
2. `/storage/emulated/0/keytao/rime-data`
3. `/storage/emulated/0/keytao/shared`
4. `filesDir/rime-data`
5. `noBackupFilesDir/keytao/rime-data`

这些目录至少要包含 `default.yaml`。如果 APK 内置了 `src/main/assets/keytao-rime-data`，`KeytaoImeEngine` 会在初始化时复制到 `/storage/emulated/0/keytao/rime-data`，所以 release 包可以自带基础 shared data。若 native runtime 或 shared data 不可用，`nativeReady=false`，Android 软键会提示运行库未就绪，硬键交还系统，不在 Kotlin 层伪造 Rime 状态。

## Android ABI runtime 闭包

Android 复用 `keytao-core` 的 Rime 调用逻辑，但 native `librime` 二进制必须按 Android ABI 提供。仓库不提交 `.so` 二进制；本地或 CI 应把闭包导入到：

```text
vendor/librime/android/<abi>/
  include/rime_api.h
  lib/librime.so
  lib/*.so
  rime-data/default.yaml
```

支持 ABI：

| Android ABI | Rust target |
| --- | --- |
| `arm64-v8a` | `aarch64-linux-android` |
| `armeabi-v7a` | `armv7-linux-androideabi` |
| `x86` | `i686-linux-android` |
| `x86_64` | `x86_64-linux-android` |

脚本入口：

```sh
# 导入自编 Android librime SDK
scripts/android-librime-runtime.sh import-sdk --abi arm64-v8a --source /path/to/android-librime-sdk

# 或从已有 Android APK 提取 native .so；APK 必须包含纯 lib/<abi>/librime.so，
# 不能是 librime_jni.so 或其它输入法自己的 JNI wrapper。
scripts/android-librime-runtime.sh import-apk --abi arm64-v8a --apk /path/to/pure-librime-arm64.apk

# 或从 Fcitx5 Android Rime 插件 release 下载纯 Android librime.so 作为 bootstrap 来源
scripts/android-librime-runtime.sh import-fcitx5-rime --abi arm64-v8a --version 0.1.2

# 同步到 Android 工程：jniLibs/<abi> 和 assets/keytao-rime-data
scripts/android-librime-runtime.sh sync --all

# Tauri 生成 Android glue 后构建 APK
pnpm tauri android init --ci --skip-targets-install
pnpm build:android

# 打印单 ABI Rust 构建环境
source <(scripts/android-librime-runtime.sh env --abi arm64-v8a)
cargo check -p keytao-core --target aarch64-linux-android
```

Gradle `preBuild` 会自动执行：

```text
scripts/android-librime-runtime.sh sync --all --allow-missing
```

如果没有导入 runtime，它只打印 warning，方便普通 Kotlin/Gradle 配置阶段继续；真正构建 Android Rust target 时，本地 patched `librime-sys` 会强制要求：

- matching ABI 的 `vendor/librime/android/<abi>/include/rime_api.h`
- matching ABI 的 `vendor/librime/android/<abi>/lib/librime.so`
- Android NDK sysroot：`ANDROID_NDK_HOME`、`ANDROID_NDK_ROOT` 或 `NDK_HOME`

## 通用 runtime 接入

Android 直接在 `src-tauri/src/lib.rs` 暴露 JNI，不经过 `keytao-core-ffi` C ABI。原因是 Android 主库本身已经是 Rust/Tauri native library，JNI 可以直接调用 Rust crate。

JNI 方法和通用层映射：

| JNI 方法 | 通用层方法 |
| --- | --- |
| `nativeInit(userDir, sharedDir)` | `ImeRuntime::with_dirs()` + `init()` |
| `nativeReload()` | `ImeRuntime::reload()` |
| `nativeCreateSession()` | `ImeRuntime::create_session()` |
| `nativeDestroySession(session)` | drop `ImeRuntimeSession` |
| `nativeSessionState(session)` | `ImeRuntimeSession::state()` |
| `nativeProcessKey(session, keyval, modifiers)` | `ImeRuntimeSession::process_key_result()` |
| `nativeSelectCandidate(session, index)` | `ImeRuntimeSession::select_candidate()` |
| `nativeChangePage(session, backward)` | `ImeRuntimeSession::change_page()` |
| `nativeReset(session)` | `ImeRuntimeSession::reset()` |
| `nativeGetAsciiMode(session)` | `ImeRuntimeSession::is_ascii_mode()` |
| `nativeSetAsciiMode(session, enabled)` | `ImeRuntimeSession::set_ascii_mode()` |

Rust 返回 JSON，Kotlin 只反序列化：

- 原始 `ImeState` 字段：`committed`、`preedit`、`cursor`、`candidates`、`highlightedCandidateIndex`、`page`、`isLastPage`、`selectKeys`、`asciiMode`。
- `accepted`：本次按键是否被 librime 接受。
- `candidatePanel`：由 `keytao-theme::ResolvedImeTheme::candidate_panel_model()` 生成。
- `modeHint`：由 `keytao-theme::ResolvedImeTheme::mode_hint_model()` 生成。

因此 Android Kotlin 层不直接读取 librime context/menu/status，也不自己决定候选 label、候选高亮或 mode hint 文案。

## 按键模型

硬键路径在 `KeytaoInputMethodService.onKeyDown()` / `onKeyUp()`：

1. Shift key down 只记录 pending，不立即切换中英。
2. 如果 Shift 按住期间出现其它 keyDown，清掉 pending，避免 Shift+letter 被当成 solo Shift。
3. `asciiMode && !hasComposition` 时硬键交回系统，让目标 App 接收英文输入和快捷键。
4. `AndroidKeyMapper.fromAndroidKeyEvent()` 把 Android key event 转为 X11 keysym + Rime modifier mask。
5. 没有 composition 时，Space、Return、Backspace、Delete、Tab、Escape、Home/End/PageUp/PageDown/方向键以及 Ctrl/Alt 组合键直接放行。
6. 其它按键调用 `nativeProcessKey()`。
7. `accepted=false && !hasComposition` 时放行给系统。
8. `accepted=true` 或有 composition 时按 `ImeState` 应用到 `InputConnection`。

Android keysym 基线：

| Android key | X11 keysym |
| --- | --- |
| Space | `0x0020` |
| Backspace | `0xff08` |
| Tab | `0xff09` |
| Return | `0xff0d` |
| Escape | `0xff1b` |
| Home / End | `0xff50` / `0xff57` |
| Left / Up / Right / Down | `0xff51` / `0xff52` / `0xff53` / `0xff54` |
| PageUp / PageDown | `0xff55` / `0xff56` |
| Delete | `0xffff` |
| Shift_L / Shift_R release | `0xffe1` / `0xffe2` + `RIME_RELEASE_MASK` |

modifier mask 只保留：

- Shift：`0x0001`
- Control：`0x0004`
- Alt：`0x0008`
- Release：`1 << 30`

CapsLock、NumLock、meta、鼠标状态不进入 Rime mask。

## Shift 与中英切换

硬键 solo Shift release 当前按通用基线处理：

1. Shift key down 记录 `shiftPressedWithoutKey=true`。
2. 如果期间没有其它 keyDown，Shift key up 发送 `XK_Shift_L` 或 `XK_Shift_R`，mask 为 `RIME_RELEASE_MASK`。
3. 如果 Rime 接受，应用返回状态。
4. 如果 Rime 不接受，调用 `setAsciiMode(!currentState.asciiMode)` 作为 fallback。

软键盘上的 `中/英` 键不模拟 Shift；它直接调用 `setAsciiMode()`，因为移动端用户预期是显式模式键，而不是硬件键盘的 Shift release 行为。

软键盘 Shift 是 Android adapter 本地状态，不进入通用 `ImeState`：

1. `OFF`：默认小写。
2. `ONCE`：点一次 Shift，下一次字母输入大写，输入后自动回到 `OFF`。
3. `LOCKED`：在系统双击窗口内连续点两次 Shift，进入持续大写；再次点 Shift 回到 `OFF`。

## 软键盘与移动端配置

移动端键盘配置不进入通用 `ImeState`，属于 Android adapter 配置。加载顺序：

1. `/storage/emulated/0/keytao/android_ime.json`
2. `res/raw/keytao_android_ime.json`

配置字段：

| 字段 | 含义 |
| --- | --- |
| `keyboardHeightDp` | 键盘区域高度 |
| `candidateBarHeightDp` | 候选栏高度 |
| `keyboardBottomInsetDp` | 底部预留高度，用于避开 Android 输入法切换器、导航手势条等系统覆盖区域 |
| `swipeThresholdDp` | 上下滑识别阈值 |
| `rows` | 键盘行和键位 |
| `label` | 键帽主文本 |
| `value` | 默认输入文本 |
| `rimeValue` | 中文状态下优先送入 Rime 的按键序列；Rime 不接受时回退到 `value` |
| `hint` | 键帽右上角提示 |
| `weight` | 行内宽度权重 |
| `style` | 可选样式类；当前内置支持 `accent`，后续按同一字段扩展键帽样式 |
| `action` | 点击动作 |
| `swipeUp` | 上滑动作；未显式配置时，会复用当前中英状态下的长按动作，再 fallback 到单字符 `hint` |
| `swipeDown` | 下滑动作 |
| `longPress` | 长按动作；未显式配置时，单字符 `hint` 会作为长按输入 fallback |
| `asciiLongPress` | 英文状态下覆盖长按动作 |
| `numberRows` | 独立数字键盘行配置 |
| `symbolRows` | 独立符号键盘行配置，`#+=` 默认切到这一层 |
| `asciiLabel` / `asciiValue` | 英文模式下覆盖键帽显示和直接输入值 |
| `asciiAction` | 英文模式下覆盖点击动作 |

当前支持动作：

- `input`：输入文本，单字符会转成 X11 keysym 送 Rime；多字符或 ascii mode 下直接提交。
- `directInput`：绕过 Rime，直接 `commitText(value, 1)`；用于移动端底部标点长按等必须立即上屏的键。
- `rimeInput`：将 `value` 按 Unicode code point 拆成按键序列送入 Rime；`fallbackValue` 用于 Rime 不接受或英文状态下的直接提交。
- `backspace`：有 composition 时送 `XK_BackSpace`；没有 composition 时直接 `deleteSurroundingText(1, 0)`，不再同步调用 Rime，避免长按回删宿主输入框正文时阻塞 UI。
- `enter`：有 composition 时送 `XK_Return`，否则执行 editor action 或发送 Enter key event。
- `space`：有 composition 时送 `XK_Space`，否则直接提交空格。
- `shift`：切换 Android 软键盘三态大小写状态：`OFF` / `ONCE` / `LOCKED`。
- `mode`：调用 `setAsciiMode()`。
- `openPage`：先隐藏当前输入法窗口，再打开/置顶主 App 并携带 `keytao_page` extra。
- `keyboardPicker`：调用 `InputMethodManager.showInputMethodPicker()`。
- `keyboardMode`：切换移动端软键盘层，例如 `numbers` / `letters`。
- `nextCandidatePage` / `previousCandidatePage`：调用 `changePage(false/true)`。
- `reset`：调用 `reset()` 清当前 composition。
- `rimeMenu`：发送 `XK_F4` / `0xffc1` 给 librime，打开 Rime schema / options 菜单。
- `panel`：Android 本地功能面板命令，例如 `home` / `rime` / `selection` / `clipboard` / `emoji` / `close`；不进入 Rime。
- `edit`：Android `InputConnection` 编辑命令，例如 `toggleSelection`、`selectLeft`、`selectRight`、`selectAll`、`copy`、`cut`、`paste`、`lineStart`、`lineEnd`、`tab`；不进入 Rime。

内置配置当前把顶排 q-p 的 hint 长按/上滑映射为 1-0，并在 a-l / z-m 上提供常用符号长按/上滑输入。未单独声明 `swipeUp` 时，Android 会复用长按动作，所以 `m` 上滑/长按 `=` 会走 Rime 输入路径，底部 `!` / `?` 上滑/长按则按配置走 `directInput` 立即上屏。`123` 映射为数字键盘，数字页 `#+=` 映射为符号键盘，`ABC` 映射回字母键盘。模式键点击切换中英，空格键只负责空格和输入态重置，不再承担打开主题页动作。

中英状态会影响软键盘符号：例如字母页的 `，` / `。` 在英文模式下显示并输入 `,` / `.`；符号页也通过 `asciiLabel` / `asciiValue` 在中文符号和英文符号之间切换。

## Composition 与提交

Android 平台层只负责官方 `InputConnection` adapter，状态来源只能是通用层返回的 `ImeState`。

`applyState(state)` 顺序：

1. `beginBatchEdit()`。
2. 如果 `state.committed` 非空，直接 `commitText(state.committed, 1)`；Android 会用 commit 文本替换当前 composing 文本。
3. 如果 `state.preedit` 非空，调用 `setComposingText(state.preedit, 1)`。
4. 如果 `state.preedit` 为空且当前仍存在 composition，调用 `commitText("", 1)` 清空 composing 文本。
5. `endBatchEdit()`。
6. 清掉 transient commit，刷新键盘 view。

这个顺序和其它平台一致，尤其用于顶功场景：一次按键可能同时返回旧字的 `committed` 和新字的 `preedit`，Android 不能先设置新 preedit 再提交旧文本，也不能在提交候选前 `finishComposingText()`，否则编辑器会先把字母 preedit 当成正文落下，再把候选追加到后面。

当前 Android 没有使用 `ImeState.cursor` 设置 composing selection。`InputConnection.setComposingText(text, 1)` 只把光标放在 composing 文本末尾；如果后续需要精准 preedit cursor，需要改用 `Spannable` 或额外 selection API，并实测不同 editor 的兼容性。

## 候选栏和主题

Android 自绘候选栏当前落成四层：

1. `keytao-theme` 解析 `/storage/emulated/0/keytao/theme.yaml`，合并默认主题并校验范围。
2. JNI `nativeResolveThemeJson()` 返回 resolved theme JSON，`KeytaoTheme.kt` 映射到 Android 颜色、字号、padding、圆角。
3. JNI state JSON 附带 `CandidatePanelModel`、`ModeHintModel`、`schemaName` 和 `pageSize`。其中候选 label、选中态、翻页能力和中英文案来自通用 `keytao-theme`；普通按键热路径不携带完整候选列表。
4. 完整候选只在用户点击展开键时通过后台线程调用 `nativeAllCandidates()` 拉取，Rust 侧走 librime candidate iterator，避免每次输入都扫描全量候选，也避免点击展开阻塞 UI。
5. `KeytaoKeyboardView` 只用 Android `Canvas` 绘制背景、候选、preedit fallback、键帽、hint 和模式提示。

候选栏顺序参考 macOS 候选窗：只要 Rime 返回候选，就从左侧直接显示候选项，不把当前 preedit 字母放在第一位；只有没有候选但仍存在 preedit 时，才把 preedit 作为弱提示显示。选中候选使用主题色做 label、左侧强调条和选中边框点缀，避免整块面板只是一种浅色。

候选栏不是横向滚动条，而是“左侧最大可见候选 + 右侧固定展开键”。普通候选栏只消费 Rime 当前页候选，即最多是 schema/menu 配置里的 page size；`KeytaoKeyboardView` 按当前屏幕宽度测量候选 chip，能放下几个就显示几个，超出的候选不挤压右侧展开键。点击展开键后，输入法总高度不变：顶部候选栏保持，下面的键盘区域收起并替换为可上下滑动的候选网格。网格会先用当前页里首行放不下的剩余候选立即渲染，再异步补入完整候选；用户正在滚动时不重置滚动位置。

键盘内容层切换，包括字母/数字/符号切换和候选展开/收起，统一走 140ms 淡入 + 轻微下移动画。动画只在切换期间使用 `saveLayerAlpha()`，平时直接绘制，避免给按键热路径增加额外合成成本。

展开区点击候选走 `selectCandidateGlobal()`，直接调用 librime `select_candidate` 的全局候选 index；默认候选栏点击仍走当前页本地选择键语义。这样 Rime F4 schema/options 菜单可以显示并点击超过 `menu/page_size` 的 switch，例如第 7 个之后的开关，而不需要 Android 自己解释 Rime 菜单内容。

空格键默认显示当前真实方案名。Rime 菜单打开时 status 可能临时报出 `.default` 这类内部 schema，Android engine 会保留最近一次非内部 schema 名作为显示名，避免空格键在菜单态闪成内部配置名。

没有候选和 preedit 时，候选栏变成功能工具栏：左侧依次提供 `功能`、中英切换、`选择`、`剪贴板`、`Emoji`，右侧显示 KeyTao logo，不再用候选栏末尾的 `中` / `英` 文本表达模式，也不在顶部放独立 `符号` 入口。默认中英切换按钮不使用选中高亮，只显示当前语言主字，并在旁边用小号文字提示点击后会切换到的语言。`功能` 按钮会打开和候选展开共用的下方面板，默认显示功能首页；其中 `Rime` 子页才发送 `XK_F4`，让 Rime 自己生成 schema / options 菜单。`选择` 子页提供多选、左右扩展选区、全选、复制、剪切、粘贴、行首、行尾和 Tab；`剪贴板` 子页显示 Android 公开接口可读取的当前系统剪贴板和输入法会话内复制/剪切历史；`Emoji` 子页提供常用表情直接上屏。功能面板顶部左侧是 `返回`，右侧是 `设置`。符号页仍通过数字页 `#+=` 进入，符号页顶部工具栏显示 `中文` / `英文` tab，点击后通过 `setAsciiMode()` 同步 Rime 状态。

可主题化范围：

- 面板背景、边框、gap、圆角。
- 候选背景、选中背景、前景、选中前景、label/comment 颜色。
- 选中 label/comment 颜色、候选边框、选中边框、border width、inline gap。
- 字号、preedit 字号、label/comment 字号。
- 候选 padding。
- 模式提示文案。

Android 目前不复用 Linux/Windows 的 BGRA renderer，因为 Android 输入法窗口本身就是 View 层级，直接映射到 Canvas 更符合平台。但它必须复用通用主题语义和 UI model，不能在 Kotlin 中重新定义候选 label 或分页含义。

## Reload 与部署

主 App 部署或安装方案后写：

```text
/storage/emulated/0/keytao/keytao-ime.reload
```

Android 当前有两个写入入口：

- `android_smart_extract`：先调用 `ScopedStoragePlugin.smartExtractZipToPrivate()`，把方案 zip 解压/合并到 `/storage/emulated/0/keytao` 并写 reload stamp，再按旧逻辑写用户选择的外部 SAF 目录，保持用户外部备份目录兼容。
- `rime_deploy_default`：Android 分支不直接持有输入上下文，只调用 `ScopedStoragePlugin.writeImeReloadStamp()`。

IME 侧在 `onStartInputView()` 调用 `engine.reloadIfNeeded()`：

1. 读取 `keytao-ime.reload` 的 `length:lastModified` 签名。
2. 签名变化时调用 `nativeReload()`。
3. Rust 侧执行 `ImeRuntime::reload()`，重新部署并递增 generation。
4. 已有 session 在下一次访问时由 `ImeRuntimeSession` 懒刷新内部 `Engine`。
5. Android 重新读取 state 并刷新键盘 view、主题和键盘配置。

当前 reload 检测只在 input view 启动时进行；如果输入法已经打开且 App 同时部署，用户可能需要切换输入焦点或重新拉起键盘才会触发刷新。后续可在软键动作或定时轻量检查中补一次，但不能在 draw/touch 热路径里执行 deploy。

## 方案安装合并

`ScopedStoragePlugin.smartExtractZipToPrivate()` 复用已有 Android 安装合并规则，当前目标目录是 `/storage/emulated/0/keytao`：

1. 打开传入 zip。
2. 查找 `default.custom.yaml` / `default-custom.yaml`，和用户目录已有配置合并。
3. 查找根目录 `rime.lua`，和用户目录已有 `rime.lua` 合并。
4. 保留因 Lua 合并产生的重命名文件。
5. 解压其它文件到 `/storage/emulated/0/keytao`。
6. 写 `keytao-ime.reload`。
7. 返回 `mergedSchemas`、`logs` 和关键文件校验。

写用户目录时使用 `safePrivateFile(root, relativePath)` 防止 zip slip。这里仍是 Android 文件安装 adapter；Rime 部署和 session 刷新仍由通用 runtime 完成。

## App 对接点

Tauri 主 App 相关命令：

- `android_smart_extract`：安装方案到 Android 用户目录 `/storage/emulated/0/keytao` 和用户选择的外部目录。
- `rime_deploy_default`：Android 上写 reload stamp，让 IME 下次激活时 reload。
- `openPage` 软键动作：启动 `MainActivity`，通过 `keytao_page` extra 指定页面，例如 `settings` 或 `theme`。

正式 Android UI 不应该把系统输入法热路径搬进 React 页面。React 可以做方案管理、主题编辑、键盘配置编辑、诊断和引导用户打开系统输入法设置。

## 与其它平台的关键差异

| 维度 | Android IME | Linux daemon | macOS IMK | Windows TSF |
| --- | --- | --- | --- | --- |
| 系统形态 | APK 内 `InputMethodService` | 独立 daemon + 多协议后端 | `/Library/Input Methods/KeyTao.app` | TSF TIP DLL |
| 输入上下文 | 当前 service 持有一个 session | 后端/上下文独立 session | 每个 `IMKInputController` 一个 session | TSF context/service state 持有 session |
| 提交接口 | `InputConnection` | Wayland/IBus/XIM 原生协议 | `IMKTextInput` | TSF edit session |
| UI | Android 自绘 `View`，键盘和候选同屏 | SHM/X11 overlay 或系统 lookup table | AppKit `NSPanel` | Win32 layered window |
| 主题 | JNI resolved theme + common UI model + Canvas | common UI model + BGRA renderer | resolved theme JSON + AppKit DTO | common UI model + BGRA renderer |
| reload | input view 启动时比较用户目录 stamp | daemon watcher | 激活/按键前比较 stamp | focus/key event 前比较 stamp |
| 安装 | APK + 用户启用输入法；方案写 `/storage/emulated/0/keytao` | deb/rpm 安装 daemon/runtime | pkg 安装 App + IME bundle | installer 注册 TSF DLL |

Android 特有部分是软键盘布局、hint、上下滑手势和打开 App 页面动作。这些属于移动端 adapter 能力，不应塞进 `ImeState` 或通用 `theme.yaml`。

## 当前已接入能力

- Android `InputMethodService` 注册和 `zh_CN` subtype metadata。
- 自绘软键盘、候选栏、键位 hint、上下滑动作和键帽 `style` 样式类。
- 键帽轻阴影、按压下沉和顶部高光，保持 48dp 级触控目标的同时增强可点击层次。
- 硬键和软键统一转为 X11 keysym + Rime modifier mask。
- JNI 直连 `keytao-core::ImeRuntime` / `ImeRuntimeSession`。
- commit + new preedit 的固定 `InputConnection` 应用顺序。
- 候选点击选择、候选翻页 action、reset、ascii mode 切换。
- 硬键无 composition bypass 和 Ctrl/Alt 快捷键放行。
- 硬键 solo Shift release 中英切换和 fallback。
- `keytao-theme` resolved theme 接入。
- `CandidatePanelModel` / `ModeHintModel` 接入。
- 候选栏默认按屏幕宽度裁剪、右侧固定展开键、展开网格替换键盘区、排除首行候选、上下滑动和全局候选点击命中。
- 完整候选非阻塞按需加载：普通 `ImeState` 不再携带全量候选，展开时先显示当前页剩余候选，再后台调用 `nativeAllCandidates()`。
- 键盘层和展开层切换动画。
- 空闲候选栏工具栏、KeyTao logo、常驻选择/剪贴板/Emoji 入口、功能面板首页和 Rime F4 子页入口。
- 功能面板 `选择` 子页：多选、左右扩展选区、全选、复制、剪切、粘贴、行首、行尾、Tab。全选/复制/剪切/粘贴走 Android `performContextMenuAction()`，左右扩选和行首/行尾走轻量 `sendKeyEvent()`，不再同步 `getExtractedText()` 拉取全文，避免 WebView/Tauri 编辑器点击选择功能时卡主线程。
- 功能面板 `剪贴板` 子页：读取当前系统剪贴板，并显示输入法会话内复制/剪切历史。
- 功能面板 `Emoji` 子页：常用 emoji 直接上屏。
- 数字页和符号页分离，符号键支持中英状态覆盖。
- 回退键长按 repeat：按住后连续派发 `backspace`，松手或移出按键停止；没有 composition 时直接删除宿主输入框正文，不再同步进入 Rime。
- `/storage/emulated/0/keytao/keytao-ime.reload` reload stamp。
- 方案 zip 写入 Android 用户目录并兼容外部 SAF 目录。
- Android ABI runtime 导入/校验/同步脚本。
- APK assets 内置 `keytao-rime-data` 到用户目录的解包路径。
- Tauri command 查询 Android 输入法启用/当前选中状态，打开系统输入法设置，并弹出系统输入法选择器。
- React Android 首启引导：未启用或未选中 KeyTao 时停在配置页，返回 App 后自动重新检测，满足条件后进入主界面。
- Release CI Android job：安装 NDK，导入四个 ABI 的 runtime，构建 split APK，并上传到 GitHub Release。

## 未实现或待补齐

1. Android ABI `librime` 发行源仍需产品化
   CI 当前使用 Fcitx5 Android Rime 插件里的纯 `librime.so` bootstrap 四个 ABI，并通过脚本拒绝 `librime_jni.so`、`JNI_OnLoad` 和第三方输入法 Java wrapper 符号。这个路径可解决当前 APK 闭包和启动闪退问题；正式长期方案建议换成可复现的自建 Android librime/OpenCC/rime-plugins SDK，并记录源码版本、patch、构建参数和产物校验。

2. Android NDK 未安装时不能完成 Rust target 检查
   `cargo check -p keytao-core --target aarch64-linux-android` 需要 NDK sysroot，并会明确提示设置 `ANDROID_NDK_HOME`、`ANDROID_NDK_ROOT` 或 `NDK_HOME`。Release CI 已安装 `ndk;27.0.12077973`；本地仍需自行安装并导出环境变量。

3. 原始 Gradle 入口仍依赖 Tauri 生成文件
   `tauri.settings.gradle` 是 Tauri 生成物且未提交，直接 `./gradlew :app:testDebugUnitTest` 仍会被生成文件缺失挡住。当前正式入口是 `pnpm tauri android init --ci --skip-targets-install` 后再走 Tauri/Gradle；后续可补一个测试专用 Gradle include 入口。

4. preedit cursor 尚未精确应用
   `ImeState.cursor` 已从 Rust 返回，但 Android 当前只用 `setComposingText(preedit, 1)`，没有把 cursor 映射到 composing selection。

5. reload 触发时机仍偏保守
   现在只在 `onStartInputView()` 检查 stamp。输入法已经打开时，App 部署后的刷新可能要等下一次启动 input view。

6. 诊断入口不足
   目前没有 Android 专用 `ime_status` 命令展示 native library loaded、engine init error、user/shared dir、reload signature、schema 是否存在、最近 JNI 错误等。

7. 移动端键盘配置没有 UI 编辑器
   `android_ime.json` 已支持声明式 `rows` / `numberRows` / `symbolRows`、自定义键位、`weight` 宽度、`style` 样式类、hint、上下滑动作、数字/符号页和中英符号覆盖，但主 App 还没有专门的可视化编辑/校验界面。

8. 手势能力仍很基础
   当前支持上滑复用 hint/长按符号数字、下滑动作、展开候选区上下滚动和回退键长按 repeat。还没有长按弹出、左右滑移动光标、候选翻页手势、键盘高度拖拽等移动端常见能力。

9. 周边文本和 editor subtype 深度适配未接
   当前没有读取 surrounding text，也没有按 `EditorInfo.inputType` 细分数字、密码、URL、email、搜索、换行等布局和提交策略。

10. 系统级剪贴板历史不可完整枚举
    Android 公开 `ClipboardManager` 只能可靠读取当前 primary clip；当前剪贴板页显示当前系统剪贴板和 KeyTao IME 会话内复制/剪切历史。若要展示系统键盘那种完整历史，需要用户显式授权的辅助服务、厂商私有能力或 KeyTao 自己持久化后续剪贴板变化。

11. 多 session / 多 display 并发未细化
    当前 service 级一个 session 对普通手机输入足够，但多窗口、多 display、外接硬键盘复杂场景可能需要按 input context 管理 session。

## 复用审计

已经复用通用层：

- librime 初始化、部署、session、reload generation：`keytao-core::ImeRuntime`。
- 按键处理、candidate、page、reset、ascii mode：`ImeRuntimeSession`。
- `ImeState` 抽取：Rust 通用层。
- modifier mask 过滤：`ImeRuntimeSession::process_key_result()` 内部通用逻辑。
- 主题解析、默认值和范围校验：`keytao-theme`。
- 候选 label、highlight、page navigation、mode hint 文案：`CandidatePanelModel` / `ModeHintModel`。
- App 部署后的 reload stamp 名称和语义：`keytao-ime.reload`。

仍保留在 Android 平台层是合理的：

- `InputMethodService` 生命周期和 `InputConnection` 调用顺序。
- Android `KeyEvent` 到 X11 keysym 的转换。
- 软键盘布局、键帽、hint、上下滑动作、打开 App 页面。
- Android Canvas 绘制和触摸命中。
- Android 用户目录和 SAF 外部目录写入。

还可以继续收敛的部分：

- `shouldBypassHardwareKey()` 与 solo Shift 规则目前在 Kotlin 中按通用规范实现；未来可以暴露 `keytao-core::key_policy` 的 JNI policy helper，避免每个平台重复维护 bypass 表。
- reload stamp 的签名比较目前在 Kotlin 中；未来可把 stamp path、mtime/size 检测和 reload request 抽到通用 runtime helper。
- shared data/runtime 查找可以随 Android 发行闭包落成后收敛到 `keytao-core::default_shared_data_dir()` 或一个 Android 专用 Rust helper，减少 Kotlin 目录猜测。
- Android 诊断 JSON 应尽量由 Rust 返回 core/runtime 状态，Kotlin 只补 Android framework 状态。

## 排查入口

当前可查：

- 用户目录：`adb shell ls -la /storage/emulated/0/keytao`
- reload stamp：`/storage/emulated/0/keytao/keytao-ime.reload`
- 主题文件：`/storage/emulated/0/keytao/theme.yaml`
- 移动端键盘配置：`/storage/emulated/0/keytao/android_ime.json`
- Android logcat：过滤 `Keytao`、`InputMethodService`、`librime`、`Rime`
- 系统输入法设置：确认 KeyTao 已启用并被选中

后续应增加 App 内诊断命令，至少返回：

- native library 是否加载成功。
- `nativeInit()` 失败原因。
- user/shared data dir 和 `default.yaml` 是否存在。
- Android ABI `librime` / OpenCC / plugin 是否可加载。
- 当前 reload stamp 签名。
- 当前 schema、ascii mode、候选数量和最近一次 process key 结果。
