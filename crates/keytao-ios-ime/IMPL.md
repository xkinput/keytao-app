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
- `Sources/KeyTaoIOSIME/KeyTaoIOSFloatingLayout.swift`：浮动/单手键盘的缩放契约（按方向 clamp）与持久化状态类型；刻意不引 UIKit / CKeytaoCore，`Tests/FloatingLayoutTests` 可以在 host 工具链上直接编译运行它。
- `test-floating-layout.sh`：编译并运行 `Tests/FloatingLayoutTests`，校验 iOS 侧解码与 `keytao-theme::mobile_layout` 的 clamp 一致。
- `test-touch-rollover.sh`：编译并运行 `Tests/TouchRolloverTests`，校验每根手指独立完成 `begin/move/finish` 并按抬起顺序产生命令。
- `test-interaction-policy.sh`：编译并运行 `Tests/InteractionPolicyTests`，校验长按/连删时序、光标手势、退格分段与备选默认选择策略。
- `Sources/KeyTaoIOSIME/KeyTaoIOSState.swift`：解析 FFI 返回的 Android-compatible state JSON，包括 `CandidatePanelModel` 和 `ModeHintModel`。
- `Sources/KeyTaoIOSIME/KeyTaoIOSTheme.swift`：解析 `keytao-theme` resolved JSON，并映射到 UIKit 颜色、字号和圆角。
- `Sources/KeyTaoIOSIME/Resources/keytao_ios_ime.json`：内置 iOS 移动端键盘布局，来源与 Android 默认配置同构。

## Apple 官方契约对齐点

iOS 系统键盘必须作为 containing app 内的 custom keyboard extension 发布：

- extension 主类继承 `UIInputViewController`，键盘 UI 添加到 controller 的 primary view。
- extension target 的 `Info.plist` 使用 `NSExtensionPointIdentifier = com.apple.keyboard-service`。
- 必须在需要时提供切换到下一个键盘的入口。生效布局来自用户可编辑的 `keyboard.yaml`，因此这个保证不放在布局文件里，而由 `KeyTaoIOSKeyboardView.applyInputModeSwitchKey()` 在渲染前强制注入：`needsInputModeSwitchKey` 为 true 且当前层没有 `keyboardPicker` 键时，在最后一行首位插入 🌐 键并从该行最宽的键（各布局里都是空格）扣掉同等 weight；为 false 时把已有的 `keyboardPicker` 键剔除。
- 🌐 键上覆盖一个透明 `UIButton`，按 Apple 文档把 `handleInputModeList(from:with:)` 绑定到 `.allTouchEvents`：轻点切到下一个键盘，长按弹出系统键盘选择器。覆盖层不可用时（候选面板展开等）仍回落到 `advanceToNextInputMode()`。
- 文本只能通过 `textDocumentProxy` 的 `insertText()` / `deleteBackward()` 等接口进入宿主输入框。
- iOS 会在 secure text input、phone pad / name phone pad 等场景临时替换为系统键盘；宿主 App 也可以拒绝第三方键盘。
- extension 只能在自己的主 view 内绘制，不能像 macOS/Windows/Linux 那样在光标附近显示独立候选窗；但从 iOS 13 起 `UITextDocumentProxy` 提供 `setMarkedText(_:selectedRange:)` / `unmarkText()`，宿主输入框内的 preedit 是可用的。
- 默认没有网络、App Group 或 containing app shared container 权限；当前模板设置 `RequestsOpenAccess=true`，用户仍必须在系统设置里显式允许“完全访问”，KeyTao 才能读取 App Group 里的方案、主题和 reload stamp。
- iOS 16+ 会对 extension 主动读取 `UIPasteboard` 弹出系统粘贴提示，因此 iOS 不在键盘显示时读取剪贴板或自动展示粘贴建议。

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
- `easy_en.schema.yaml`、`easy_en.dict.yaml`、`easy_en.custom.yaml`、`lua/easy_en.lua`：可选 Easy English 附加方案。资源由 Tauri 放进 containing app 的 `assets/addon-schemas/easy_en`，主 App 的 `addon_schema_install` 再复制到 App Group；键盘扩展只读取已部署的 App Group 数据，不直接修改 bundle。
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
  - `keytao_session_process_enter_json`（D5 唯一的 Enter 实现）
  - `keytao_session_commit_composition_json`（Rime 拒收按键时先把已转换内容上屏）
  - `keytao_session_set_input_policy_json`（D9 敏感输入）
  - `keytao_text_to_keysym`（D10 text→keysym 唯一实现）
  - `keytao_utf16_offset_from_chars`（D8 偏移换算）
  - `keytao_reload_if_stamp_changed`（reload stamp 检测收敛到通用层）
  - `keytao_engine_capabilities`（D4 能力探测，返回 `KEYTAO_CAP_*` 掩码；Swift 侧包成 `KeyTaoEngineCapabilities`）
  - `keytao_session_capabilities`（同一套掩码的 per-session 版本，session 为 null / 已退役时返回 0）

这些 JSON 与 Android JNI state JSON 同形，包含：

- 原始 `ImeState` 字段：`committed`、`preedit`、`cursor`、`selStart`、`selEnd`、`candidates`、`highlightedCandidateIndex`、`page`、`isLastPage`、`selectKeys`、`asciiMode`、`schemaName`。`allCandidates` 已从通用层删除，完整候选改用 `keytao_session_all_candidates_json` 按需拉取。
- `cursor` / `selStart` / `selEnd` 的单位是 Unicode 标量偏移，Swift 侧用 `keytao_utf16_offset_from_chars` 换算成 UTF-16 再交给 UIKit。
- `KeyTaoImeState` 逐字段解码（缺字段回退到默认值），避免通用层加一个键就让整份 state 解析失败、键盘彻底失去状态。
- `accepted`：本次按键是否被 librime 接受。
- `candidatePanel`：由 `keytao-theme::ResolvedImeTheme::candidate_panel_model()` 生成。
- `modeHint`：由 `keytao-theme::ResolvedImeTheme::mode_hint_model()` 生成。

因此 Swift 层不直接读取 librime context/menu/status，也不自行决定候选 label、候选高亮、翻页能力或中英文案。

## Composition 与提交

`UITextDocumentProxy` 自 iOS 13 起提供 `setMarkedText(_:selectedRange:)` 与 `unmarkText()`，这是 CJK 输入法在宿主输入框内展示未上屏编码的标准通道（宿主据此抑制自动更正、分组撤销）。当前应用顺序：

1. FFI 返回 `committed` 非空时，先清掉 marked text，再 `textDocumentProxy.insertText(committed)`。
2. `preedit` 非空时调 `setMarkedText(preedit, selectedRange:)`；选中段用 `selStart` / `selEnd`（换算成 UTF-16）表达，没有选中段时退化为 `cursor` 处的零长插入点。
3. `preedit` 为空时清 marked text。
4. `preedit` 和候选同时仍显示在键盘顶部候选栏，作为宿主 proxy 对 marked text 支持不佳时的降级显示。
5. `preedit` 为空且无候选时，候选栏恢复为空闲 toolbar。
6. 光标移动、宿主文本变化、键盘收起时统一走 `resetInputContextCaches()`：清 marked text、清 composition、清退格恢复栈与剪贴板快照。

清 marked text 必须是「先 `setMarkedText("")` 再 `unmarkText()`」：`unmarkText()` 只是把 marked text 定稿成普通文本，不会删掉它，直接 unmark 会让 preedit 和 Rime 提交的文本同时留在输入框里。

少数宿主（尤其 WKWebView 内的输入框）对 proxy marked text 支持不佳。`ios_ime.json` 的 `hostMarkedText: false` 可以关掉宿主内 preedit，只保留候选栏 preedit。

所有对宿主的写入都经 `withHostTextMutation`，在当前 runloop 内屏蔽由我们自己触发的 `textWillChange` / `selectionWillChange`，否则每次 `setMarkedText` 都会被当成用户切到了另一个输入框而清掉组字。

Enter 走通用层唯一实现 `keytao_session_process_enter_json`（把 `XK_Return` 交给 Rime，未接受时由 core fallback 到 `commit_raw_input`），平台不再自己提交 preedit 字面量。

按键的 text→keysym 转换走 `keytao_text_to_keysym`，与 Android/桌面同一份实现：单个可打印字符映射为 keysym（Latin-1 直通，其余 `0x01000000|cp`），返回 0 表示不要送 Rime。Rime 未接受该键时先 `commit_composition()` 把已转换内容上屏，再把原字符直接插入宿主，避免丢字。

按 D6，**任何模式下按键都先进 Rime**：英文模式是 librime 的 `ascii_mode` 选项而不是键盘侧的捷径，iOS 不再按 `currentState.asciiMode` 直接提交字符，schema 的标点、`ascii_composer`、`key_binder` 规则在英文模式下同样生效；librime 拒绝该键时才走上面的宿主直插路径。唯一的旁路仍是 D9 的敏感/直通宿主（`hostTraits.bypassesRime`），那里 session policy 本来就不组字。这条契约的前提是 schema 的 processors 里有 `ascii_composer`（Rime 默认预设都有）：没有它时 `ascii_mode` 不会让 librime 拒绝字母键，英文模式会继续组字——这一点五个前端一致，不是 iOS 独有。

移动端 `englishMode` 默认 `ascii`，因此上述 `ascii_mode` 契约仍是默认及无 English 方案时的降级路径。仅当 `englishMode == "schema"` 且已安装 English 方案时，中/En 才会先清掉未上屏 composition，再选择 English schema；返回中文会恢复进入 English 前快照的中文 schema 选项（包括临时 `ascii_punct`）。敏感/直通宿主的性能旁路始终只使用 `ascii_mode`，不切换 schema；桌面契约不受此移动端设置影响。

多字符 `rimeInput`（键位的 `rimeValue`）按序**逐次**送 Rime 并逐次应用结果：中途触发的自动上屏必须先落到宿主，否则会被后一次 state 覆盖而丢字。回退分两种，避免把已经进过 Rime 的前缀重复打进宿主：

- 送出任何一键**之前**：先把整串逐字符映射成 keysym，只要有一个映射不出来，整个键回退成它声明的字面量（`fallbackValue`），Rime 完全不参与。
- 送出过前缀**之后**被拒：先应用该次结果（保住它带回的 commit），再 `commit_composition()` 上屏已转换内容，最后只把 Rime 没消费掉的**剩余子串**插入宿主。

空格键同样遵守 D6：不再按「有没有组字」在本地分流，一律先送 `XK_Space` 给 Rime（`full_shape` 会把它变成 U+3000，schema 也可能在 `key_binder` 里绑它），Rime 拒绝时才按上面的宿主直插路径补一个半角空格。

engine 因 App Group/schema/runtime 不可用而无法进入正式 Rime session 时，`input` / `rimeInput` 会 fallback 为直接提交对应字符；这个路径只用于 simulator smoke 和首次安装诊断。运行库还在后台初始化时，按键改为进入重放队列（上限 32 个），初始化结束后按序回放，不会把编码原样打进宿主。

## 宿主 traits（UITextInputTraits）

`textDocumentProxy` 继承 `UITextInputTraits`，是 iOS 上等价于 Android `EditorInfo` 的唯一协议通道。`KeyTaoHostTraits` 在 `viewWillAppear` / `textDidChange` 读取并下发：

| trait | 行为 |
| --- | --- |
| `returnKeyType` | Enter 键标签改为 前往 / 搜索 / 加入 / 下一项 / 路线 / 发送 / 完成 / 继续 / 紧急呼叫 |
| `keyboardType` = numberPad、decimalPad、phonePad、asciiCapableNumberPad | 强制切到 numbers 层，并按 D9 把 session policy 置为不组字 |
| `keyboardType` = namePhonePad、numbersAndPunctuation、emailAddress、URL、webSearch | 不旁路；这些 presentation hint 仍允许中文组字 |
| `keyboardType` = asciiCapable | **不**旁路：该值只表示键盘可以显示 ASCII，很多宿主在仍需中文的输入框上也会设它 |
| `autocapitalizationType` | 仅在英文模式生效：`allCharacters` 锁定 Shift，`sentences` / `words` 在新输入上下文里预置一次性 Shift；中文模式下永不预置（大写字母不在任何 Rime speller 字母表里） |
| `keyboardAppearance` | 优先级高于设备外观：`.dark` / `.alert` → 深色，`.light` → 浅色，`.default` 才回落到 traitCollection |
| `isSecureTextEntry` | 见下节 |

## 敏感输入（D9）

系统会在 secure text entry 场景把键盘临时换成系统键盘，但本扩展仍做防御性处理：`isSecureTextEntry` 或上表中的直通类 `keyboardType` 出现时，调用 `keytao_session_set_input_policy(session, composing: false, learning: false)`，按键完全不进 librime，不产生 preedit/候选，也不可能发生用户词学习；同时清空剪贴板历史与退格恢复栈。

剪贴板读取需要完全访问：`currentClipboardText()` 先 `guard hasFullAccess`，再用 `UIPasteboard.general.hasStrings` 预检（metadata 访问，不会触发 iOS 16 起的系统粘贴授权弹窗），未授权时给出明确提示而不是误报「剪贴板为空」。剪贴板历史绑定当前输入上下文，`textWillChange` / `selectionWillChange` / `viewWillDisappear` 都会清空。

## 冷启动与内存

- `keytao_init` + `keytao_create_session` + App Group 目录种子写入在专用串行队列上执行（`KeyTaoIOSEngine.ensureReadyAsync`），只有 session 指针回到主队列安装，`viewDidLoad` 不再同步阻塞首帧；加载期间候选栏显示「正在加载键道方案…」。
- `keyboard.yaml` 只在文件缺失或为空时种子写入，不再按字符串启发式覆盖用户已经编辑过的布局。
- `didReceiveMemoryWarning` 释放可重建缓存：剪贴板历史、按键重放队列、展开候选、logo 图片、theme/config 缓存；session 与已部署方案不动，打字不受影响。
- App Group 共享文件可能正被主 App 写入，`resolveTheme` / `loadConfig` 解析结果为空时保留上一次成功的 theme/config，而不是回退到内置默认值。

## 键盘反馈

`KeyTaoInputView`（`inputView`）与 `KeyTaoIOSKeyboardView` 都遵循 `UIInputViewAudioFeedback` 并返回 `enableInputClicksWhenVisible = true`，每次确认按键调用 `UIDevice.current.playInputClick()`，是否发声由用户在「设置 > 声音」决定。触觉反馈额外要求完全访问，未授权时不再空转调用 `UIImpactFeedbackGenerator`。

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
- `edit`：`paste` / `pasteText` / `tab` / `lineStart` / `lineEnd` 已实现；`copy` / `cut` / `selectAll` / 选区扩展受 `UITextDocumentProxy` 能力限制，仍只给提示。
- `panel`：功能面板（Rime 开关、选择、剪贴板、Emoji）。

软键盘 Shift 与 Android 一致，是 adapter 本地状态：

1. `off`
2. `once`
3. `locked`

模式键直接调用 `setAsciiMode()`，不模拟硬件 Shift release。

浮动/单手键盘的 `floating.portrait.scale` / `floating.landscape.scale` 按方向 clamp，与共享层 `keytao-theme::mobile_layout` 的 `sanitize()` 一致：竖屏最低 `0.70`、横屏最低 `0.45`、上限 `1.0`，缺省值同为竖屏 `0.88` / 横屏 `0.62`；`scale > 1.5` 视为百分比。以前 iOS 对两个方向统一 clamp 到 `0.70`，同一份 `keyboard.yaml` 的横屏键盘在 iOS 上会被悄悄放大。用户拖拽调整单手键盘宽度时另有一个 iOS 独有的上限 `KeyTaoIOSKeyboardLayoutState.maximumScale = 0.94`——再宽的话拖拽边缘会压到屏幕边上、之后就再也缩不回来了；这是交互限制，不是布局契约。这套规则由 `test-floating-layout.sh` 覆盖（含共享层默认 JSON 的解码断言）。

## 候选栏和主题

主题调度与 Android 保持一致：

1. `keytao-theme` 解析 `theme.yaml`，合并默认主题并校验范围。
2. FFI `keytao_resolve_theme_json()` 返回 resolved theme JSON。
3. FFI state JSON 附带 `CandidatePanelModel` 和 `ModeHintModel`。
4. `KeyTaoIOSKeyboardView` 只把 model 映射到 UIKit 控件，不重新计算 label、页码和 mode hint。

iOS 由于不能在键盘 view 外绘制候选窗，候选栏固定在键盘顶部。没有候选时显示空闲 toolbar：功能面板、中英模式切换、选择/剪贴板/Emoji 面板和单手/分栏布局开关；系统键盘切换不在 toolbar 上，而是主键盘区强制注入的 🌐 键。

## 能力位驱动的 UI 降级（D4）

翻页、点击选词、候选高亮这三类控件都依赖 librime 自己的入口函数。入口缺失时通用层不会报错，而是**降级成合成按键**——翻页变成 `-`/`=`，点击选词变成 schema 的 select key。schema 没导入默认 `paging_with_minus_equal`、也没配 `menu/select_keys` 时，这些字符会直接进编码，用户看到的是「点一下候选，编码里多了个 `-`」。所以 iOS 侧的规则是：**入口不存在的控件必须先在 UI 上关掉，而不是画出来再让它污染编码。**

### 能力从哪来

- `keytao_engine_capabilities()`：ABI 级掩码，只查 `RimeApi` 的 `data_size` 和函数指针，`keytao_init()` 之前就能回答。
- `keytao_session_capabilities(session)`：同一套掩码的 per-session 版本；null / 已退役 handle 返回 0，即「全部禁用」，这是安全方向。

`KeyTaoIOSEngine.capabilities` 把两者合成一个缓存值：

- 还没有 session 时用 `KeyTaoEngineCapabilities.current`（ABI 级）。**不能**直接用 session 版本，否则运行库在后台启动的那几帧里掩码是 0，布局会先把翻页键丢掉、session 就绪后再长回来，出现闪烁。
- `installPreparedSession()` / `initializeRuntime()` 装上 session 后立刻 `refreshCapabilities()` 改用 session 掩码；`close()` 和 reload（`reload()` / `reloadIfNeeded()`）同样刷新，因此 session 重建、runtime 换代都不会留下属于旧 handle 的缓存。

`KeyTaoKeyboardViewController.refreshEngineCapabilities()` 在 `refreshInputAvailability()`（冷启动、每次 `viewWillAppear`）和 `reloadIfNeeded()` 里读这个值，变化时 `KeyTaoIOSKeyboardView.update(engineCapabilities:)` 推给 view 并重排。

### 各能力位对应的降级

| 能力位 | 缺失时的通用层行为 | iOS UI 降级 |
| --- | --- | --- |
| `KEYTAO_CAP_NATIVE_PAGING` | 合成 `-`/`=` | `applyEngineCapabilities(to:)` 在布局解析阶段丢掉 `nextCandidatePage` / `previousCandidatePage` 键（整行被丢空就连行去掉；整套布局只剩翻页键时保留原布局，否则会把强制注入的 🌐 键一起弄没）；滑动、长按、键位栈里的同名 command 由 `changeCandidatePage(backward:)` 兜住，提示「当前 RIME 运行库不支持候选翻页」 |
| `KEYTAO_CAP_CANDIDATE_SELECTION` / `KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION` | 合成 select key | `KeyTaoIOSKeyboardView.isSelectable(_:)` 按位判断（面板功能行自带 command，不过 librime，不受影响）：不可选时候选不再是 accessibility button 而是 `.staticText`，点击不收起展开面板、不触发触感，只交给 `keyboardView(_:didSelectCandidate:global:)` 提示「当前 RIME 运行库不支持点击选词」 |
| `KEYTAO_CAP_CANDIDATE_HIGHLIGHT` | 高亮移动降级为 no-op，**不伪造按键** | 候选上画的高亮是 librime 自己的 `highlighted_candidate_index`（即 Space 会上屏的那一条），缺这个位也依然真实，所以**不撤**；这个位真正管的是「前端主动移动高亮」（hover、方向键），iOS 软键盘没有这类手势，也不画 hover 态，因此没有可关的东西。日后如果加了会调 `keytao_session_highlight_candidate` 的交互，必须先查这个位——`selectedGlobalCandidateIndex()` 上的注释锁了这条约束 |
| `KEYTAO_CAP_CANDIDATE_DELETION` | — | iOS 目前没有「忘掉这个词」手势，未接入 |

### 判断方式

UI 一律**按位判断**，不写「iOS 就是没有」。iOS 当前的实测结果是 selection / global selection / deletion 为 1，paging / highlight 为 0（`keytao-core::engine_capabilities()` 对 iOS target 编译期判定后两项不可用，因为 1.8.5 的 `RimeApi` 根本没有这两个字段），但只要换上 librime ≥ 1.9 的 runtime，掩码变了 UI 就自动恢复，不需要改 Swift。

## Reload 与部署

reload stamp 路径：

```text
group.ink.rea.keytao-app/keytao/keytao-ime.reload
```

iOS extension 在 `viewWillAppear()` / `textDidChange()` 等轻量生命周期里调用 `reloadIfNeeded()`：

1. 调 `keytao_reload_if_stamp_changed()`。stamp 路径、签名格式（`<len>:<mtime_nanos>:<fnv1a64>`）与变更检测全部由 `keytao_core::ReloadStamp` 提供，Swift 侧不再自己算 `size:mtime` —— 主 App 报出的签名和键盘比较的签名从此是同一个函数算的。
2. 通用 runtime 执行 `ImeRuntime::reload_without_deploy()`：丢弃本 runtime 全部存活 session 的 Engine，再 finalize/initialize，并递增 generation。
3. 已有 session 下一次操作时懒刷新内部 `Engine`，并迁移重建前的 ascii_mode。
4. reload 真正跑成功才把这次 stamp 变更标记为已处理，失败会在下一次检查时重试。
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
| preedit | `setComposingText()` 写宿主输入框 | `setMarkedText()` 写宿主输入框 |
| UI | Android `Canvas` 自绘 | UIKit view/button/scroll view |
| next keyboard | `InputMethodManager.showInputMethodPicker()` | `advanceToNextInputMode()` |
| open access | Android 存储权限 | `RequestsOpenAccess` + 用户允许完全访问 |
| reload | `onStartInputView()` 检查 stamp | `viewWillAppear()` / `textDidChange()` 检查 stamp |

## 当前已接入能力

- iOS custom keyboard extension 源码包和 `Info.plist` 模板。
- UIKit 软键盘、候选栏、字母/数字/符号层。
- 移动端配置 `ios_ime.json`，字段与 Android 默认配置保持同形。
- 点击、长按、上滑、下滑动作。
- 中英模式键、结构化 Rime 方案/选项页、候选选择、reset；Rime 选项从当前 schema 的 `switches` 动态读取，布尔项使用 schema 状态文案，单选组循环切换，无 switches 时显示空态；翻页与点击选词按 `KEYTAO_CAP_*` 掩码开关，见「能力位驱动的 UI 降级（D4）」。
- 强制注入的 🌐 切换键、`advanceToNextInputMode()` 与长按 `handleInputModeList(from:with:)` 键盘选择器。
- C FFI per-session runtime：init、reload、create/destroy session、process key、select candidate、global select、all candidates、change page、reset、schema list/current/select 和 Rime options。
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

  该脚本只做源码级检查，**不编译 Swift**。在 macOS 上真正编译 iOS 代码要指定模拟器 SDK 与 target，
  否则 `swift build` 会按 macOS 目标编译并直接失败于 `no such module 'UIKit'`：

  ```bash
  cd crates/keytao-ios-ime
  swift build --sdk "$(xcrun --sdk iphonesimulator --show-sdk-path)" \
    -Xswiftc -target -Xswiftc arm64-apple-ios15.0-simulator
  ```


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

3. 宿主 marked text 兼容性实测
   `setMarkedText` / `unmarkText` 已接入，但各类宿主（原生 UITextField/UITextView、WKWebView 输入框、Flutter/RN 文本框）对 proxy marked text 的实现质量不一，需要在真机上按宿主类型实测；出问题的宿主可用 `ios_ime.json` 的 `hostMarkedText: false` 降级为「仅候选栏 preedit」。

4. 选择/复制/剪切面板未接
   粘贴与剪贴板历史已接入（需完全访问），但选区扩展、复制、剪切依赖宿主 API，`UITextDocumentProxy` 无对应能力，当前仍只提示「当前输入框不支持此编辑操作」。

5. 扩展内存预算实测
   `didReceiveMemoryWarning` 降级钩子已就位，但键盘扩展的 jetsam 上限是未公开数值，键道大词库在扩展内的常驻内存尚无本机实测数据，需要在真机上记录一次基线后再决定是否进一步收缩 `expandedCandidateLimit` 等参数。

6. iOS librime runtime 版本落后
   `vendor/librime/ios/*` 当前是 librime 1.8.5（`scripts/build-ios-librime.sh` 固定在 LibrimeKit v0.1.0 的 ref），缺 `change_page` 与 `highlight_candidate_on_current_page` 两个 1.9 起才有的入口。keytao-core 现在按 `data_size` 做 ABI 能力探测并对 iOS target 编译期判定这两项为不可用（`EngineCapabilities { native_paging: false, candidate_highlight: false }`），`cargo check -p keytao-core-ffi --target aarch64-apple-ios-sim` 已恢复通过。仍需重建/导入 librime ≥ 1.9 的 iOS SDK（会连带涉及 Boost 版本与 C++ 标准、librime-lua 的兼容补丁）。当前实际影响：
   - 选词走官方 `select_candidate_on_current_page`（1.8.5 已有），不合成 select key，安全。
   - 高亮降级为 no-op，不伪造按键；Swift 侧本来也不调用。
   - 翻页在 1.8.5 上会退化成合成 `-`/`=` 按键（schema 没导入默认 `paging_with_minus_equal` 时这两个字符会进编码），**因此翻页 UI 现在按能力禁用**；内置默认布局本来就没有翻页键，只有用户自己在 `keyboard.yaml` 里写了这两个动作才会看到差异。
   - 点击选词同样过 `KEYTAO_CAP_CANDIDATE_SELECTION` / `KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION` 判断。1.8.5 两项都有，所以当前不会触发；这道 guard 是防止将来换到缺入口的 runtime 时退回合成 select key。
   - 具体的降级点、能力缓存与刷新时机见「能力位驱动的 UI 降级（D4）」。

7. iPad/外接键盘适配未细化
   当前重点是触摸软键盘。iPad split/floating keyboard、hardware keyboard passthrough 和多窗口需要单独验证。

## 验证记录

已通过：

```bash
source vendor/librime/macos-universal/env.sh
cargo check -p keytao-core -p keytao-core-ffi
pnpm check:ios-ime
```

Swift 源码类型检查通过。`pnpm check:ios-ime` 会校验主 App/extension plist、entitlement、Swift 源码、切换键强制注入断言和 C FFI 头文件，并运行 rollover、interaction policy、floating layout 三个 host 测试脚本；在存在 `vendor/librime/ios/<target>` 或 `KEYTAO_IOS_RIME_ROOT` 时继续检查 iOS Rust target，没有 iOS 版 librime runtime 时会跳过链接检查并明确提示导入命令。keytao-core 加上 ABI 能力探测后，最后一步 `cargo check -p keytao-core-ffi --target aarch64-apple-ios-sim` 在本机 librime 1.8.5 runtime 上也已通过（原先报 `E0609`）。

`test-touch-rollover.sh`、`test-interaction-policy.sh` 与 `test-floating-layout.sh` 均由 `scripts/verify-ios-ime.sh` 调用。

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

## 键盘内设置面板

工具栏的「设置」会打开键盘内设置面板。布局、候选字号、键角提示和反馈选项仍写入 `ios_ime.json`，配色与主题色仍写入 `theme.yaml`；没有新增配置存储。滑杆和色板在手势移动时只更新内存中的键盘配置、约束常量或主题预览，抬手时才完整应用布局，并通过 `persistSettings` 或主题写入器持久化一次。高度预览不使用动画。

App 的移动端页面继续保留长按延迟、删除速度、退格滑动模式、滑动判定阈值、双击空格句号、Flick、回车键、悬浮布局、配置路径及恢复默认等低频设置。完整主题路径、桌面主题配置和桌面页面行为不受键盘内面板影响。
