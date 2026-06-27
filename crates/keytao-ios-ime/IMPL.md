# iOS IME 实现说明

本文只记录 `crates/keytao-ios-ime` 里的 iOS 系统键盘 extension 前端实现，并按当前代码同步。

跨平台通用契约见 [输入法通用层实现规范](../../docs/ime-common-layer.md)；本文只补充 iOS `UIInputViewController`、`UITextDocumentProxy`、App Group、移动端键盘配置和 C FFI 的平台差异。

## 代码地图

- `Package.swift`：SwiftPM 源码包，供 Tauri 生成的 iOS Xcode 工程或手工 extension target 引入。
- `Resources/Info.plist`：iOS custom keyboard extension 的 Info.plist 模板，声明 `com.apple.keyboard-service`、`RequestsOpenAccess`、`PrimaryLanguage=zh-Hans` 和 `IsASCIICapable=true`。
- `Sources/CKeytaoCore/module.modulemap`：把 `keytao-core-ffi/include/keytao_core.h` 暴露给 Swift。
- `Sources/KeyTaoIOSIME/KeyTaoKeyboardViewController.swift`：`UIInputViewController` 前端，负责 extension 生命周期、`UITextDocumentProxy` 提交/删除、候选选择和键盘切换。
- `Sources/KeyTaoIOSIME/KeyTaoIOSEngine.swift`：iOS engine facade，解析 App Group 用户目录、shared data、theme/config、reload stamp，并通过 C FFI 调用通用 runtime。
- `Sources/KeyTaoIOSIME/KeyTaoIOSKeyboardView.swift`：UIKit 键盘视图，按移动端配置渲染字母/数字/符号层、候选栏、模式键、长按和上下滑动作。
- `Sources/KeyTaoIOSIME/KeyTaoIOSConfig.swift`：解析用户目录 `ios_ime.json` 或 bundle 内置 `keytao_ios_ime.json`，字段与 Android `android_ime.json` 保持同形。
- `Sources/KeyTaoIOSIME/KeyTaoIOSState.swift`：解析 FFI 返回的 Android-compatible state JSON，包括 `CandidatePanelModel` 和 `ModeHintModel`。
- `Sources/KeyTaoIOSIME/KeyTaoIOSTheme.swift`：解析 `keytao-theme` resolved JSON，并映射到 UIKit 颜色、字号和圆角。
- `Sources/KeyTaoIOSIME/Resources/keytao_ios_ime.json`：内置 iOS 移动端键盘布局，来源与 Android 默认配置同构。

## Apple 官方契约对齐点

iOS 系统键盘必须作为 containing app 内的 custom keyboard extension 发布：

- extension 主类继承 `UIInputViewController`，键盘 UI 添加到 controller 的 primary view。
- extension target 的 `Info.plist` 使用 `NSExtensionPointIdentifier = com.apple.keyboard-service`。
- 必须在需要时提供切换到下一个键盘的入口；当前默认布局包含 `keyboardPicker` 键，并调用 `advanceToNextInputMode()`。
- 文本只能通过 `textDocumentProxy` 的 `insertText()` / `deleteBackward()` 等接口进入宿主输入框。
- iOS 会在 secure text input、phone pad / name phone pad 等场景临时替换为系统键盘；宿主 App 也可以拒绝第三方键盘。
- extension 只能在自己的主 view 内绘制，不能像 macOS/Windows/Linux 那样在光标附近显示独立候选窗，也不能设置宿主输入框的 marked text。
- 默认没有网络、App Group 或 containing app shared container 权限；当前模板设置 `RequestsOpenAccess=true`，用户仍必须在系统设置里显式允许“完全访问”，KeyTao 才能读取 App Group 里的方案、主题和 reload stamp。

官方参考：

- [App Extension Programming Guide: Custom Keyboard](https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/CustomKeyboard.html)
- [Creating a custom keyboard](https://developer.apple.com/documentation/UIKit/creating-a-custom-keyboard)
- [UIInputViewController](https://developer.apple.com/documentation/uikit/uiinputviewcontroller)
- [UITextDocumentProxy](https://developer.apple.com/documentation/uikit/uitextdocumentproxy)
- [Virtual keyboards - Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines/virtual-keyboards)

## 系统注册与工程接入

稳定源码放在 `crates/keytao-ios-ime`，不是 `src-tauri/gen/apple`。原因是 Tauri Apple 工程属于生成物，当前 `.gitignore` 已忽略 `src-tauri/gen/apple/`；iOS extension target 应在生成 Xcode 工程后引用这里的 SwiftPM product 或复制这些源码。

extension target 需要：

1. 以 `Resources/Info.plist` 为模板创建 custom keyboard extension target。
2. 把 principal class 设成 Objective-C 可见的 `KeyTaoKeyboardPrincipalViewController`；Tauri 生成工程会自动生成这个薄子类，并继承 SwiftPM product 中的 `KeyTaoIOSIME.KeyTaoKeyboardViewController`。
3. containing app 与 keyboard extension 同时开启 App Group，例如 `group.ink.rea.keytao-app`。
4. extension entitlement 必须允许 App Group；否则只能使用 extension 自己的容器，无法读取主 App 安装的方案。
5. extension 需要链接 iOS 目标的 `libkeytao_core_ffi` 及其 iOS librime/OpenCC/rime-plugins runtime 闭包。

当前 `librime-sys/build.rs` 已支持 iOS runtime 查找：

```text
RIME_INCLUDE_DIR / RIME_LIB_DIR
KEYTAO_IOS_RIME_ROOT
vendor/librime/ios/<target>
vendor/librime/ios/iphoneos-arm64
vendor/librime/ios/iphonesimulator-arm64
vendor/librime/ios/iphonesimulator-x86_64
```

如果 `lib/librime.a` 存在，iOS 默认按 static link 处理；也可以用 `KEYTAO_RIME_LINK_KIND=static|dylib` 覆盖。bindgen 会按 target 选择 `iphoneos` 或 `iphonesimulator` SDK。

### Simulator 签名规则

真机和 TestFlight/App Store 构建必须保留 App Group entitlement，并由匹配的 provisioning profile 证明 `group.ink.rea.keytao-app`。iOS Simulator 则不同：Tauri/Xcode 的 simulator 包通常是 ad-hoc 签名，如果 `.appex` 带有 `com.apple.security.application-groups` 或自动注入的 `application-identifier`，CoreSimulator 的 AMFI 会拒绝加载键盘，表现为系统键盘切换菜单能看到 “KeyTao 输入法”，但点击后仍停留在 Emoji 或系统键盘。

`scripts/setup-ios-ime-xcode.rb` 因此对 simulator 做了专门分流：

- app 和 extension 都生成空的 simulator entitlement plist。
- `CODE_SIGN_INJECT_BASE_ENTITLEMENTS[sdk=iphonesimulator*] = NO`，避免 Xcode 自动注入 `application-identifier`。
- `CODE_SIGN_STYLE[sdk=iphonesimulator*] = Manual`、`CODE_SIGN_IDENTITY[sdk=iphonesimulator*] = -`、`DEVELOPMENT_TEAM[sdk=iphonesimulator*] = ""`。
- embedded `KeyTaoKeyboard.appex` 在 containing app 构建阶段重新签名；`iphonesimulator` 下不复用 `.xcent`。

这个分流只影响 simulator smoke 验证。真机仍使用 `Resources/KeyTaoKeyboard.entitlements` 和主 App entitlement 中的 App Group。

## 用户目录和 shared data

iOS 输入法优先使用 App Group 目录：

```text
group.ink.rea.keytao-app/keytao
```

常见文件：

- `keytao.schema.yaml`、`default.custom.yaml`、`*.dict.yaml`、`lua/`、`opencc/`：用户方案和运行时数据。
- `rime-data/default.yaml`：基础 shared data fallback。
- `theme.yaml`：用户主题，交给 `keytao-theme` 解析。
- `ios_ime.json`：iOS 移动端键盘布局和动作配置。
- `keytao-ime.reload`：主 App 部署或主题/配置变更后写入的 reload stamp。

如果 App Group 不可用，`KeyTaoIOSPaths.userRoot()` fallback 到 extension 自己的 Application Support 下的 `keytao`。这个 fallback 只适合开发或内置数据测试；正式发行必须使用 App Group，否则主 App 安装的方案不会被 extension 看到。当前 simulator smoke 构建会刻意禁用 App Group entitlement，所以它依赖 bundle `rime-data`、内置 `keytao_ios_ime.json` 和 fallback 直接提交路径来验证“可安装、可切换、可输入”，不代表真机共享目录失效。

shared data 查找顺序：

1. `KEYTAO_RIME_SHARED_DATA_DIR`
2. App Group 用户目录本身
3. `group.../keytao/rime-data`
4. `group.../keytao/shared`
5. extension bundle 里的 `rime-data`

目录至少要包含 `default.yaml`。

## 通用 runtime 接入

iOS 不走 Android JNI，而是复用 `keytao-core-ffi` per-session C ABI。

本次通用层补齐：

- `keytao-core` 的 librime runtime cfg 扩展到 `target_os = "ios"`。
- `keytao-theme::default_user_theme_path()` 增加 iOS fallback。
- `keytao-core-ffi` 不再排除 iOS。
- `src-tauri/src/lib.rs` 增加 iOS App Group adapter：`rime_get_data_dir`、`check_local_schema`、`get_component_versions`、`rime_install_to_default`、`rime_deploy_default` 和输入法 UI 主题设置都读写 `group.ink.rea.keytao-app/keytao`。
- 主 App 会在 App Group 中种子写入默认 `ios_ime.json`，主题保存和方案部署后写 `keytao-ime.reload`。
- 新增 JSON FFI：
  - `keytao_set_theme_paths`
  - `keytao_resolve_theme_json`
  - `keytao_session_state_json`
  - `keytao_session_process_key_json`
  - `keytao_session_select_candidate_json`
  - `keytao_session_select_candidate_global_json`
  - `keytao_session_all_candidates_json`
  - `keytao_session_change_page_json`
  - `keytao_session_reset_json`
  - `keytao_session_set_ascii_mode_json`

这些 JSON 与 Android JNI state JSON 同形，包含：

- 原始 `ImeState` 字段：`committed`、`preedit`、`cursor`、`candidates`、`highlightedCandidateIndex`、`page`、`isLastPage`、`selectKeys`、`asciiMode`、`schemaName`。
- `accepted`：本次按键是否被 librime 接受。
- `candidatePanel`：由 `keytao-theme::ResolvedImeTheme::candidate_panel_model()` 生成。
- `modeHint`：由 `keytao-theme::ResolvedImeTheme::mode_hint_model()` 生成。

因此 Swift 层不直接读取 librime context/menu/status，也不自行决定候选 label、候选高亮、翻页能力或中英文案。

## Composition 与提交

iOS custom keyboard 和 Android `InputConnection` 最大差异是：`UITextDocumentProxy` 没有设置 marked text / composing text 的公开能力。

当前 iOS 应用顺序：

1. FFI 返回 `committed` 非空时，调用 `textDocumentProxy.insertText(committed)`。
2. 不把 `preedit` 插入宿主输入框。
3. `preedit` 和候选只显示在键盘顶部候选栏。
4. `preedit` 为空且无候选时，候选栏恢复为空闲 toolbar。
5. 光标移动、宿主文本变化或 selection 变化时，调用 `reset()` 清掉 extension 内部 composition。

这保持了顶功场景的提交顺序，但牺牲了 Android/macOS/Windows 那种宿主输入框内的 preedit 视觉。除非 Apple 未来给 custom keyboard extension 开放 marked text API，否则 iOS 只能用“键盘内 preedit”的方式实现。

当 engine 因 App Group/schema/runtime 不可用而无法进入正式 Rime session 时，`input` / `rimeInput` 会 fallback 为直接提交对应字符。这个路径只用于 simulator smoke 和首次安装诊断，确保键盘扩展本身可被系统加载、按键可进入宿主输入框；真机和生产 runtime 可用时仍走 `keytao-core-ffi`。

## 软键盘与移动端配置

加载顺序：

1. `group.ink.rea.keytao-app/keytao/ios_ime.json`
2. extension bundle 内置 `keytao_ios_ime.json`
3. Swift fallback 布局

配置字段与 Android 保持同形：

- `keyboardHeightDp`
- `candidateBarHeightDp`
- `keyboardBottomInsetDp`
- `swipeThresholdDp`
- `rows`
- `numberRows`
- `symbolRows`
- `label`
- `value`
- `rimeValue`
- `hint`
- `weight`
- `style`
- `action`
- `swipeUp`
- `swipeDown`
- `longPress`
- `asciiLongPress`
- `asciiLabel` / `asciiValue`
- `asciiAction`

支持动作：

- `input`
- `directInput`
- `rimeInput`
- `backspace`
- `enter`
- `space`
- `shift`
- `mode`
- `keyboardPicker`
- `keyboardMode`
- `nextCandidatePage`
- `previousCandidatePage`
- `reset`
- `rimeMenu`
- `openPage`：尝试打开 `keytao://<page>`，失败时只提示用户打开主 App。
- `edit` / `panel`：当前只提示用户到主 App 使用，未接 iOS 编辑面板。

软键盘 Shift 与 Android 一致，是 adapter 本地状态：

1. `off`
2. `once`
3. `locked`

模式键直接调用 `setAsciiMode()`，不模拟硬件 Shift release。

## 候选栏和主题

主题调度与 Android 保持一致：

1. `keytao-theme` 解析 `theme.yaml`，合并默认主题并校验范围。
2. FFI `keytao_resolve_theme_json()` 返回 resolved theme JSON。
3. FFI state JSON 附带 `CandidatePanelModel` 和 `ModeHintModel`。
4. `KeyTaoIOSKeyboardView` 只把 model 映射到 UIKit 控件，不重新计算 label、页码和 mode hint。

iOS 由于不能在键盘 view 外绘制候选窗，候选栏固定在键盘顶部。没有候选时显示空闲 toolbar：系统键盘切换、当前中英模式、schema 名和 Rime F4 入口。

## Reload 与部署

reload stamp 路径：

```text
group.ink.rea.keytao-app/keytao/keytao-ime.reload
```

iOS extension 在 `viewWillAppear()` / `textDidChange()` 等轻量生命周期里调用 `reloadIfNeeded()`：

1. 读取 reload stamp 的 `size:mtime` 签名。
2. 签名变化时调用 `keytao_reload()`。
3. 通用 runtime 执行 `ImeRuntime::reload()` 并递增 generation。
4. 已有 session 下一次操作时懒刷新内部 `Engine`。
5. Swift 重新读取 state、theme 和 `ios_ime.json`。

主 App 的 iOS 命令已经按 Android 的安装/部署路径接入：

- `rime_get_data_dir`：返回 App Group 下的 `keytao` 用户目录。
- `check_local_schema` / `get_component_versions`：读取同一 App Group 目录。
- `rime_install_to_default`：下载方案 zip，复用通用 `smart_install()` 合并 `default.custom.yaml` 和 `rime.lua`，落盘到 App Group。
- `rime_deploy_default`：调用 `keytao_core::deploy(user, shared)`，shared data 优先查 App Group、`rime-data`、`shared` 和 bundle runtime。
- `get_ime_ui_settings` / `set_ime_ui_settings`：读写 `theme.yaml`，并写 reload stamp。
- 首次安装/部署/保存 UI 时，如果 App Group 中没有 `ios_ime.json`，主 App 会写入与 Swift bundle fallback 同源的默认移动端布局。

## 与 Android 的关键差异

| 维度 | Android IME | iOS keyboard extension |
| --- | --- | --- |
| 系统入口 | `InputMethodService` | `UIInputViewController` extension |
| 运行库桥接 | JNI 直连 `keytao-core` | C ABI `keytao-core-ffi` |
| 用户目录 | `/storage/emulated/0/keytao` | App Group `group.ink.rea.keytao-app/keytao` |
| 提交接口 | `InputConnection` | `UITextDocumentProxy` |
| preedit | `setComposingText()` 写宿主输入框 | 只能显示在键盘候选栏 |
| UI | Android `Canvas` 自绘 | UIKit view/button/scroll view |
| next keyboard | `InputMethodManager.showInputMethodPicker()` | `advanceToNextInputMode()` |
| open access | Android 存储权限 | `RequestsOpenAccess` + 用户允许完全访问 |
| reload | `onStartInputView()` 检查 stamp | `viewWillAppear()` / `textDidChange()` 检查 stamp |

## 当前已接入能力

- iOS custom keyboard extension 源码包和 `Info.plist` 模板。
- UIKit 软键盘、候选栏、字母/数字/符号层。
- 移动端配置 `ios_ime.json`，字段与 Android 默认配置保持同形。
- 点击、长按、上滑、下滑动作。
- 中英模式键、Rime F4、候选选择、候选翻页、reset。
- `advanceToNextInputMode()` 系统键盘切换入口。
- C FFI per-session runtime：init、reload、create/destroy session、process key、select candidate、global select、all candidates、change page、reset、ascii mode。
- `keytao-theme` resolved theme JSON 接入。
- `CandidatePanelModel` / `ModeHintModel` 接入。
- App Group 用户目录和 reload stamp 约定。
- 主 App iOS App Group 安装、部署、schema 检查、版本信息和主题调度命令。
- iOS target 的 `librime-sys` runtime 查找与 bindgen SDK 参数。
- `src-tauri/Info.ios.plist` 声明 `keytao://` URL scheme，`openPage` 可以从键盘 extension 打开 containing app。
- `KeyTaoApp.entitlements` / `KeyTaoKeyboard.entitlements` 声明同一个 App Group。
- Tauri 生成工程中的 `KeyTaoKeyboardPrincipalViewController` principal subclass。
- 主 App 与 keyboard extension 的 AppIcon 资源进入各自 bundle。
- simulator 空 entitlement 分流与 embedded `.appex` 无 entitlement 重签名。
- 键位、候选和 toolbar accessibility identifier，供 UI test 定位 `keytao-key-q`、`keytao-candidate-0` 等控件。
- `scripts/ios-librime-runtime.sh` 导入、校验和 staged iOS librime runtime。
- `scripts/build-ios-ffi.sh` 构建并 staged iOS `libkeytao_core_ffi.a`。
- `scripts/build-ios-simulator-smoke-runtime.sh` 生成仅用于本机模拟器 smoke 验证的 mock runtime。
- `scripts/setup-ios-ime-xcode.rb` patch Tauri 生成的 XcodeGen `project.yml`，嵌入 `KeyTaoKeyboard` extension target。
- `scripts/verify-ios-ime.sh` / `pnpm check:ios-ime` 源码级校验。

## 构建脚本与 runtime

生产构建必须导入真实 iOS librime SDK。仓库不提交 iOS 二进制 runtime，导入目录必须包含：

```text
include/rime_api.h
lib/librime.a       # 必须静态合入 librime-lua
rime-data/default.yaml
```

iOS 键盘扩展按 static runtime 链接，Lua 能力需要通过 `scripts/build-ios-librime.sh` 把 `hchunhui/librime-lua` 合进 `librime.a`。不要照 macOS/Linux 的方式只复制 `rime-plugins/librime-lua.dylib` / `.so`；`scripts/ios-librime-runtime.sh verify` 会检查 `lua_processor` / `lua_translator` 是否已经进入静态库，否则顶功、Lua filter/translator/processor 都会失效。

导入和 staged 生产 runtime：

```bash
scripts/ios-librime-runtime.sh import-sdk --target aarch64-apple-ios --source /path/to/ios-librime-sdk
scripts/build-ios-ffi.sh --target aarch64-apple-ios
pnpm init:ios
pnpm build:ios
```

本机模拟器 smoke runtime 只用于验证 Xcode target、extension bundle、FFI 符号和按键提交路径，不替代真实 librime。它会生成 simulator `libkeytao_core_ffi.a` 和 `librime.a` mock，并把基础 `rime-data` staged 到 `target/keytao-ios-runtime/iphonesimulator-*`：

```bash
pnpm build:ios-simulator-smoke-runtime
pnpm init:ios
xcodebuild \
  -project src-tauri/gen/apple/keytao-app.xcodeproj \
  -target KeyTaoKeyboard \
  -configuration debug \
  -sdk iphonesimulator \
  -arch arm64 \
  CODE_SIGNING_ALLOWED=NO \
  build
```

`scripts/setup-ios-ime-xcode.rb` 会在 Tauri 生成的 `project.yml` 中做这些事：

- 引入本地 SwiftPM package `KeyTaoIOSIME`。
- 增加 `KeyTaoKeyboard` custom keyboard extension target。
- 为 extension 生成 `KeyTaoKeyboardPrincipalViewController.swift`，并把 `NSExtensionPrincipalClass` 指向这个 Objective-C 可见类。
- 真机构建中 containing app 和 extension 共享 `group.ink.rea.keytao-app` App Group。
- simulator 构建中关闭基础 entitlement 注入，并对 embedded `.appex` 做无 entitlement 重签名。
- 为主 App 和 keyboard extension 设置 `ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon` 与 `CFBundleIcons`。
- `KeyTaoKeyboard` extension target 设置 `SKIP_INSTALL=YES`，只作为 `KeyTao.app/PlugIns/KeyTaoKeyboard.appex` 随主 App 安装，不应在桌面出现独立 `KeyTaoKeyboard` 图标。
- 按 `iphoneos` / `iphonesimulator` / `arch` 解析 `KEYTAO_IOS_RUNTIME_DIR`。
- 给 app 和 extension 都注入 `HEADER_SEARCH_PATHS`、`LIBRARY_SEARCH_PATHS` 和必要 linker flags。
- 在 extension 构建产物根目录复制默认 `keytao_ios_ime.json`，并复制 runtime `rime-data`。
- 包裹 Tauri 的 `pnpm tauri ios xcode-script`，让主 App Rust 构建同样使用 iOS runtime 环境。

如果模拟器桌面已经出现 `KeyTaoKeyboard` 或 `KeyTaoUITestHost`，它们是旧构建或 UI test 残留，不是用户安装形态。清理命令：

```bash
xcrun simctl uninstall booted ink.rea.keytao-uitest-host || true
xcrun simctl uninstall booted ink.rea.keytao-app.keyboard || true
```

## 仍需外部输入

1. iOS 版 librime SDK
   仓库不提交生产二进制 SDK。需要用 `scripts/build-ios-librime.sh --target <rust-target>` 构建并导入带 merged `librime-lua` 的 SDK，或用 `scripts/ios-librime-runtime.sh import-sdk --target <rust-target> --source <sdk>` 导入已经静态合入 Lua 的外部 SDK。支持的 target 是 `aarch64-apple-ios`、`aarch64-apple-ios-sim` 和 `x86_64-apple-ios`；脚本会映射到 `iphoneos-arm64`、`iphonesimulator-arm64` 和 `iphonesimulator-x86_64` runtime 目录。导入后 `scripts/build-ios-ffi.sh` 会把 runtime 与 `libkeytao_core_ffi.a` staged 到 `target/keytao-ios-runtime/<runtime>`。模拟器 smoke runtime 只覆盖安装/启动/基础提交路径验证，不能用于真实输入效果验收。

2. XcodeGen / Apple 签名环境
   `src-tauri/gen/apple` 是 Tauri 生成物，不提交到仓库。执行 `pnpm init:ios` 后，`scripts/setup-ios-ime-xcode.rb` 会自动 patch `project.yml` 并重新生成 Xcode 工程。脚本会优先使用 `.cache/bin/xcodegen`，否则使用系统 `xcodegen`。真实设备和 TestFlight/App Store 构建仍需要有效 Apple Team、bundle id 和 App Group provisioning profile；simulator 构建则必须保持无 restricted entitlement，否则键盘扩展会出现在切换菜单但无法加载。

3. 宿主输入框内 preedit 不可用
   这是 Apple custom keyboard extension API 限制，不是当前实现遗漏。当前只能在键盘候选栏显示 preedit。

4. 剪贴板和编辑面板未接
   iOS open-access keyboard 可使用更多能力，但剪贴板、选择、复制、粘贴等行为涉及隐私和宿主兼容性，当前只提示回主 App。

5. iPad/外接键盘适配未细化
   当前重点是触摸软键盘。iPad split/floating keyboard、hardware keyboard passthrough 和多窗口需要单独验证。

## 验证记录

已通过：

```bash
source vendor/librime/macos-universal/env.sh
cargo check -p keytao-core -p keytao-core-ffi
pnpm check:ios-ime
```

Swift 源码类型检查通过。`pnpm check:ios-ime` 会校验主 App/extension plist、entitlement、Swift 源码和 C FFI 头文件，并在存在 `vendor/librime/ios/<target>` 或 `KEYTAO_IOS_RIME_ROOT` 时继续检查 iOS Rust target；没有 iOS 版 librime runtime 时会跳过链接检查并明确提示导入命令。

2026-06-24 本机模拟器 smoke 验证：

```bash
pnpm build:ios-simulator-smoke-runtime
pnpm init:ios
KEYTAO_IOS_DEVELOPMENT_TEAM=2G395DH7KX PATH="$PWD/.cache/bin:$PATH" scripts/setup-ios-ime-xcode.rb
xcodebuild -list -project src-tauri/gen/apple/keytao-app.xcodeproj
KEYTAO_IOS_DEVELOPMENT_TEAM=2G395DH7KX PATH="$PWD/.cache/bin:$PATH" pnpm tauri ios dev 'KeyTao iPhone 17 Pro Clean 26.5' --no-watch --exit-on-panic
xcodebuild test -project .cache/keytao-ios-uitest/KeyTaoKeyboardUITest.xcodeproj -scheme KeyTaoKeyboardUITests -destination 'id=B4F3F4C8-D8DA-4E09-99B3-B6D552855F5E' -configuration Debug -sdk iphonesimulator -only-testing:KeyTaoKeyboardUITests/KeyTaoKeyboardSettingsUITests/testTypeWithKeyTaoKeyboardInHost
```

已确认：

- 本地 `.cache/bin/xcodegen` 可用，版本为 2.45.4。
- `target/keytao-ios-runtime/iphonesimulator-arm64` 和 `iphonesimulator-x86_64` 已生成 smoke runtime。
- Xcode 工程包含 `keytao-app_iOS`、`KeyTaoKeyboard` 和 `KeyTaoIOSIME` target/scheme。
- `KeyTaoKeyboard` target 可为 iOS Simulator arm64 构建成功，containing app 可安装到 `KeyTao iPhone 17 Pro Clean 26.5`。
- 生成的 `.appex` 是 arm64 Mach-O，`Info.plist` 声明 `com.apple.keyboard-service`、`KeyTaoKeyboardPrincipalViewController`、`RequestsOpenAccess=true`、`PrimaryLanguage=zh-Hans`。
- `.appex` 已链接 `_keytao_session_process_key_json` 等 C FFI 符号，并复制根目录 `keytao_ios_ime.json` 与 `rime-data/default.yaml`。
- 安装后的 `KeyTao.app` entitlements 是空字典，`KeyTaoKeyboard.appex` 无 entitlement 输出；`codesign --verify --deep --strict` 通过。
- 安装后的 `KeyTao.app` 和 `KeyTaoKeyboard.appex` 均包含 `CFBundleIcons`、`Assets.car` 和 AppIcon PNG 资源。
- simulator 全局 `AppleKeyboards` 包含 `ink.rea.keytao-app.keyboard`。
- UI test 成功从 Emoji 键盘切到 “KeyTao 输入法 - KeyTao”，出现 `keytao-key-q`，点击 `q`、`e`、`y` 后宿主输入框 echo 为 `qey`。
- `cargo build -p keytao-app --target aarch64-apple-ios-sim --features custom-protocol` 可在 simulator smoke runtime 下完成，生成 `libkeytao_app_lib.a`。

此前切换失败的根因是 simulator `.appex` 在 ad-hoc 签名下仍包含 restricted entitlement。系统日志中的关键错误为：

```text
The file is adhoc signed but contains restricted entitlements
proc ... load code signature error 4 for file "KeyTaoKeyboard"
```

修复后 simulator 构建不再带 App Group 或 `application-identifier`，AMFI 不再拒载，键盘可以实际打开并输入。
