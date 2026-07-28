# macOS IME 实现说明

本文只记录 `crates/keytao-macos-ime` 里的 macOS 系统输入法实现，并按当前代码同步。

跨平台通用契约见 [输入法通用层实现规范](../../docs/ime-common-layer.md)；本文只补充 macOS IMK/TIS 的协议、打包和 AppKit UI 差异。

## 代码地图

- `Sources/KeyTaoIME/main.swift`：Swift 可执行入口、输入源管理命令、`IMKServer` 创建、accessory app run loop。
- `Sources/KeyTaoIME/IMKSetup.swift`：保留的 C ABI setup helper，目前不是 `build.sh` 产物的主入口。
- `Sources/KeyTaoIME/InputSourceInstaller.swift`：TIS 注册、启用、选择、禁用旧输入源、列出 KeyTao 输入源。
- `Sources/KeyTaoIME/EngineInit.swift`：用户目录/共享目录解析，以及 `KeyTaoRuntime` —— 进程级一次性初始化、UI 能力声明、reload stamp 文件监听。
- `Sources/KeyTaoIME/ImeState.swift`：FFI JSON 状态的 Swift DTO（`KeyTaoImeState` / `KeyTaoPanelModel` / `KeyTaoModeHintModel`）与 Unicode 标量偏移 → UTF-16 换算。
- `Sources/KeyTaoIME/InputController.swift`：`IMKInputController` 子类，处理按键、composition、候选窗、模式切换。
- `Sources/KeyTaoIME/CandidatePanel.swift`：候选 NSPanel，只把通用层的 `CandidatePanelModel` 映射到 AppKit。
- `Sources/KeyTaoIME/ModeIndicatorPanel.swift`：中英模式提示 NSPanel，文案来自通用层的 `ModeHintModel`。
- `Resources/Info.plist`：bundle 元信息、输入源 id、IMK controller/delegate class、图标和 TIS 输入模式声明。
- `build.sh`：构建 IME-only bundle/pkg。
- `install.sh`：开发用本机安装脚本。
- `Smoke/main.swift` + `smoke.sh`：手动冒烟工具，用真实 librime 校验 macOS 依赖的 keytao-core-ffi 契约（state JSON 形状、候选面板模型、key policy、偏移换算），不进任何构建或 CI 路径。
- `scripts/build-macos.sh`：构建主 App + IME bundle 的完整 pkg。

## 位置与标识

- 输入法 bundle：`/Library/Input Methods/KeyTao.app`
- bundle id：`ink.rea.inputmethod.keytao`
- 主输入源 id：`ink.rea.inputmethod.keytao.Hans`
- IMK connection name：`KeyTao_Connection`
- controller/delegate class：`KeyTaoIME.KeyTaoInputController`
- 用户数据目录：默认 `~/Library/keytao`

macOS IME 不默认读取 `~/Library/Rime`。那个目录属于鼠须管，KeyTao IME 默认只读 KeyTao App 自己安装和部署的 `~/Library/keytao`。

`Info.plist` 当前声明：

- `LSUIElement=true`，输入法进程不显示 Dock 图标。
- `LSBackgroundOnly=false`，因为候选窗和模式提示需要 AppKit UI。
- 输入源菜单/调色板图标使用 `keytao-menu-icon.pdf`。

## 构建与安装

`crates/keytao-macos-ime/build.sh` 做 IME-only 构建：

1. 查找或下载 librime 开发文件。
2. 生成输入源图标。
3. 构建 `keytao-core-ffi` 动态库。
4. 创建 `KeyTao.app` bundle skeleton。
5. 复制 `Info.plist`、图标、本地化 `InfoPlist.strings`，并用 `PlistBuddy` 把 workspace 版本写进 `CFBundleShortVersionString` / `CFBundleVersion`。
6. 复制 `libkeytao_core_ffi.dylib`、`librime*.dylib` 和 rime plugins。
7. 生成 `Sources/CKeytaoCore/module.modulemap`。
8. 用 `swiftc` 编译所有 Swift 文件为 `Contents/MacOS/KeyTaoIME`。
9. 对 dylib 和 bundle 签名。
10. 默认生成只安装 IME bundle 的 pkg（`pkgbuild --version` 同样用 workspace 版本）；`--skip-pkg` 可跳过。

版本来源优先级：`KEYTAO_VERSION` 环境变量 → `package.json`（node 可用时）→ `Cargo.toml` 的 `[workspace.package].version`。`CFBundleShortVersionString` 用完整版本串（如 `1.2.1-alpha.42`）；`CFBundleVersion` 必须能被 Installer 比较，所以剥掉非数字字符变成 `1.2.1.42`。pkg component plist 的 `BundleIsVersionChecked=true` 依赖这个键判断是否替换已安装 bundle，写死 `1` 会让升级留下旧 IME bundle 与旧 FFI dylib。

`scripts/build-macos.sh` 做完整发行 pkg：

1. 构建 IME runtime。
2. 准备主 App runtime，把 `rime-data`、`librime.1.dylib`、OpenCC 数据和 `rime-plugins` 放进 Tauri 资源/Frameworks 目录。
3. 构建 Tauri 主 App。
4. 确认主 App bundle id 是 `ink.rea.keytao-app`，IME bundle id 是 `ink.rea.inputmethod.keytao`。
5. 在签名前把 `rime-plugins` 和插件依赖补进主 App `Contents/Frameworks`，保证主 App 部署 Lua 方案和 IME 运行时使用同等能力。
6. 重签主 App 及 dylib。
7. 打包 `/Applications/KeyTao.app` 和 `/Library/Input Methods/KeyTao.app`。
8. `postinstall` 结束 `KeyTaoIME` / `imklaunchagent` / `TextInputMenuAgent`，运行 `lsregister`、清理 quarantine/provenance xattr、注册/启用/选择输入源。
9. PackageInfo 使用 `postinstall-action="logout"`，让 Installer 在完成页要求注销当前用户会话。

安装脚本**不得**执行 `killall cfprefsd`。`cfprefsd` 是系统级 CFPreferences 守护进程，以 root 结束它会连带终止其它用户/系统实例并丢弃在途偏好写入；输入源注册走 TIS API，本来就通过它正常落盘，加上 `postinstall-action="logout"` 已经保证配置生效。`scripts/verify-macos-pkg.sh` 会检查 postinstall 里没有这行，并校验 IME bundle 版本与 pkg 版本一致。

本地完整打包和离线验证命令：

```sh
pnpm install
pnpm build:macos
scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg
```

测试安装命令：

```sh
sudo installer -pkg target/keytao-macos-pkg/KeyTao.pkg -target /
```

改动按键/候选/主题路径后，建议先跑一次 FFI 契约冒烟（会组字并提交，请指向用户目录的副本）：

```sh
rsync -a --exclude log ~/Library/keytao/ /tmp/keytao-smoke-user/
crates/keytao-macos-ime/build.sh --release --skip-pkg
crates/keytao-macos-ime/smoke.sh /tmp/keytao-smoke-user
```

安装后必须先注销并重新登录 macOS，再执行方案安装、手动部署和输入验证：

```sh
test -d "/Applications/KeyTao.app"
test -x "/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME"
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --list-input-sources
open -a KeyTao
```

Release CI 的 macOS 分支必须走 `pnpm build:macos`，然后执行 `scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg`，最后上传 `keytao-app-<version>-macos-<arch>.pkg`。当前脚本按 runner 架构构建，例如 `macos-arm64` 或 `macos-x86_64`；不要用 Tauri 的 dmg bundle 作为 macOS 发行产物。

macOS 发行包只构建 pkg，不构建 dmg。原因是 KeyTao 同时包含普通 App 和系统输入法 bundle，必须把输入法稳定安装到 `/Library/Input Methods` 并执行 TIS/LaunchServices 注册；dmg 拖拽安装无法可靠表达这个系统输入法安装流程。

`install.sh` 是开发安装脚本，会构建 IME pkg、sudo 安装到系统输入法目录、刷新 LaunchServices/TIS 相关进程，并执行注册命令。

## 输入源注册命令

`main.swift` 在正常启动 IMK server 前会先解析命令行：

- `--register-input-source`：调用 `TISRegisterInputSource`。
- `--enable-input-source`：启用 `ink.rea.inputmethod.keytao.Hans`。
- `--select-input-source`：启用并选择主输入源。
- `--disable-legacy-input-sources`：禁用旧 bundle id/input source id。
- `--list-input-sources`：打印包含 KeyTao/keytao 的输入源。

这些命令由 pkg `postinstall` 和开发 `install.sh` 使用。

## 进程启动

正常输入法进程启动流程：

1. `main.swift` 创建 `IMKServer(name:bundleIdentifier:)`。
2. `NSApplication.shared` 设置为 `.accessory`，调用 `KeyTaoRuntime.shared.start()`，然后进入 run loop。
3. `KeyTaoRuntime.start()` 同步做两件轻量的事：`keytao_set_ui_capabilities(...)` 声明 macOS 候选窗能力（可自定义颜色、可竖排、可 hover、可阴影、可分隔线），以及 `ImeThemeManager.installThemeSources()` 把 default/user theme 路径与系统配色注入通用层；重活全部丢到后台队列。
4. 后台队列上 `initializeEngine()` 解析 user/shared dir 并调用 `keytao_init(userDir, sharedDir)`；该入口只加载已部署产物，未安装或未部署时会失败。失败后按 2 秒退避重试，**不会**在每次按键上重试。
5. macOS 通过 `imklaunchagent` 按需连接该 IMK server。
6. 每个输入上下文创建一个 `KeyTaoInputController`，构造时创建独立 `keytao_create_session()`。
7. runtime 尚未就绪时 session 为 nil，controller 直接放行按键，并通过 `KeyTaoRuntime.requestInitialization()` 触发一次后台重试；runtime 起来后广播 `KeyTaoRuntime.didReloadNotification`，controller 在主线程补建 session 并刷新状态。
8. controller deinit 时调用 `keytao_destroy_session()` 并注销通知观察者。

`handleEvent:client:` 是客户端进程的同步调用，输入法在其中阻塞会拖住前台应用的事件循环。因此 librime 初始化、部署检测、reload stamp 读取一律不在 IMK 回调线程上做。

`IMKSetup.swift` 中的 `keytao_imk_setup()` 也会创建 `IMKServer` 并保存在全局，但当前 `build.sh` 直接编译 Swift 可执行文件，主路径是 `main.swift`。

## 用户目录

`resolveUserDataDir(home:)` 的规则：

1. 如果设置了 `KEYTAO_RIME_USER_DATA_DIR`，并且该目录含有 `keytao.schema.yaml` 或 `build/keytao.schema.yaml`，使用它。
2. 否则使用 `~/Library/keytao`。

这里刻意不探测 `~/Library/Rime`，避免和鼠须管的用户配置混用。

## 共享数据目录

IME 进程的 `resolveSharedDataDir()` 规则：

1. 依次读取 `KEYTAO_RIME_SHARED_DATA_DIR`、`RIME_SHARED_DATA_DIR`、`RIME_DATA_DIR`。
2. 只接受包含 `default.yaml` 的目录。
3. 再尝试：
   - `/Applications/KeyTao.app/Contents/Resources/rime-data`
   - `/Applications/KeyTao.app/Contents/SharedSupport`
   - `/Library/Input Methods/KeyTao.app/Contents/Resources/rime-data`
   - `/Library/Input Methods/KeyTao.app/Contents/SharedSupport`
   - `/Library/Input Methods/Squirrel.app/Contents/SharedSupport`
   - `/opt/homebrew/share/rime-data`
   - `/usr/local/share/rime-data`
4. 找不到时返回空字符串，`keytao_init()` 会失败并写 `NSLog`。

主 App 的 `macos_app_shared_data_dir()` 还会优先查找 Tauri resource 中的 `rime-data`/`SharedSupport`，然后才 fallback 到输入法 bundle。

## App 部署后的重载

主 App 完成 `rime_deploy_default` 后，会写入：

```text
~/Library/keytao/keytao-ime.reload
```

stamp 的路径、格式与签名算法只有一份实现，在 `keytao-core` 里；macOS 侧不再自己读文件内容做字符串比较。检测时机：

- `KeyTaoRuntime` 在后台队列上用 `DispatchSource.makeFileSystemObjectSource` 监听用户数据目录（stamp 会被重写甚至重建，所以监听目录而不是文件；目录被删除/改名时自动重建监听，打不开时 5 秒后重试）。
- `activateServer(_:)` 也触发一次检查，但同样异步执行。
- **按键路径不做任何 stamp 检查**，`handle(_:client:)` 里没有文件 I/O。

检测与执行都走通用层：

1. `keytao_reload_stamp_path_at(userDir)` 给出监听目标。
2. `keytao_reload_if_stamp_changed()` 一步完成"签名变化 → `keytao_reload()`"；只有 reload 真跑成功才把该次变更记为已见，失败会在下一次事件重试。
3. reload 成功后主线程广播 `KeyTaoRuntime.didReloadNotification`。
4. 当前激活的 controller 收到通知后清空 marked text、隐藏候选窗和模式提示、确保 session 存在，再读一次 session state 刷新 UI。
5. 非激活的 controller 只清内部标志，不碰客户端，也不弹面板。

session 本身不会被销毁重建：`keytao_reload()` 会丢弃 runtime 内全部存活 session 的 Engine，`ImeRuntimeSession` 在下一次访问时懒重建并迁移重建前的 ascii_mode。

因此用户在 App 里重新部署后，不需要手动重启输入法；文件事件到达时就会刷新。

## 按键处理

热路径在 `InputController.swift`。

1. `recognizedEvents` 只声明 `keyDown` 和 `flagsChanged`。
2. `flagsChanged` 交给 `handleFlagsChanged()`。
3. 任何 keyDown 都先无条件清掉 `shiftPressedWithoutKey`——包括 Command 组合键。否则 Cmd+Shift+X 之后松开 Shift 会被误判成 solo Shift 并切换中英。
4. Shift/Control/Option/Command 转成 Rime modifier mask；Command 映射为 **Super**（`1 << 26`），与 X11/Rime 对窗口系统修饰键的约定一致。
5. mask 含 Super 时提前放行：Cmd 组合是窗口系统保留组合，librime 在 macOS 上不绑定它们。
6. Carbon 特殊键转换为 X11 keysym：Return、**小键盘 Enter（`XK_KP_Enter`/`0xff8d`）**、Backspace、Delete、Escape、Space、方向键、Home/End、PageUp/PageDown、Tab、F4。
7. 其余按键的文本→keysym 转换调用 `keytao_text_to_keysym()`，即 `keytao-core::key_policy::keysym_for_text` —— Latin-1 直通、其余 `0x01000000 | codepoint`。macOS 不再自己限制 ASCII 范围。无修饰键时用 `event.characters` 保留布局实际产生的字符（Shift+a 得到 `0x41`），带 Control/Option/Command 时用 `charactersIgnoringModifiers` 取基键。
8. keysym 为 0（这个键没法讲给 librime，例如媒体键、死键）时：**有 composition 就先 `commitComposition(sender)` 再放行**，不能让应用在 marked text 还在的情况下收到这个键。
9. bypass 判定调用 `keytao_key_policy_should_bypass(session, keyval, modifiers)`，不在 Swift 侧重写键集。当前通用层规则：有 composition 一律不放行；无 composition 时放行 Super/Hyper/Meta 组合与导航/编辑类 nonstarter 键（Space/Return/Backspace/Delete/Tab/Escape/Home..Begin/方向/翻页）。**Ctrl/Option 组合不再提前放行**，它们要先进 Rime，由 `accepted` 决定吞否（Rime 的 key_binder / ascii_composer 需要收到它们）。
10. Enter（`keytao_key_policy_is_enter`，含小键盘 Enter）且没有 Ctrl/Alt 时走 `keytao_session_process_enter_json()`，这是五端唯一的 Enter 实现：先把 `XK_Return` 交给 Rime，Rime 不接受且确实在组字时由通用层 fallback 到 `commit_raw_input()`。macOS 不再自造"提交 preedit 字面量"。
11. 其余按键调用 `keytao_session_process_key_json(session, keyval, modifiers)`。
12. `accepted` 原样作为 `handle` 返回值。

`ascii_mode` 不是 bypass 判据。英文模式下按键同样送进 Rime，由 `ascii_composer` 决定放行，这样 Rime 的方案自定义绑定、`ascii_punct` 开关在 macOS 上和 Linux/Windows 行为一致。

### 与通用层的一处偏离

通用层规则是"任何模式下按键都先进 Rime"；macOS 对 **Command（Super）组合** 保留提前放行。

- 偏离原因：D7 允许系统保留组合提前放行；把 Cmd+V/Cmd+A 送进 librime 存在被 Speller 吃掉的风险（旧版 librime 的 Speller 只挡 Ctrl/Alt），代价是剪贴板类快捷键在组字时失效。
- 影响范围：Rime 侧无法用 Command 做方案绑定（macOS 上本来也没有这种惯例）。
- 收敛方式：等 `key_policy` 明确定义"系统保留修饰键在有 composition 时如何处置"后，把这一句换成对通用层的调用。

## Composition 与提交

`apply(state, to:)` 的顺序：

1. 如果 `committed` 非空且当前有 composition，先用空 `setMarkedText` 清掉旧 marked range。
2. 调用 `insertText(committed)` 提交文本。
3. 调用 `setMarkedText` 写入新的 `preedit`。
4. 用 `preedit` 或候选是否为空更新 `hasComposition`。
5. 同步 `asciiMode`。
6. 候选为空则隐藏候选窗，否则显示/更新候选窗。

### marked text 的偏移单位与分段

`ImeState` 的 `cursor` / `selStart` / `selEnd` 单位是 **Unicode 标量偏移**（通用层已从 librime 的 UTF-8 字节偏移换算过），而 IMKit 的 `NSRange` 单位是 UTF-16。macOS 侧用 `String.keytaoUtf16Offset(fromCharacterOffset:)`（内部调 `keytao_utf16_offset_from_chars`）换算，不再直接把 librime 的偏移当 UTF-16 用。

marked text 按 IMK 惯例分三段标注：

- `[0, selStart)`：`kTSMHiliteConvertedText`（Rime 已转换的部分）
- `[selStart, selEnd)`：`kTSMHiliteSelectedRawText`（当前正在转换的部分）
- `[selEnd, end)`：`kTSMHiliteRawText`（剩余原始编码）

`selStart == selEnd`（没有选中段）时退化为整段 `kTSMHiliteSelectedRawText`，也就是改动前的单一下划线外观。

### 会话结束

三个入口共用 `endComposition(commit:client:)`：

- `commitComposition(_:)`（IMK 要求立即结束组字）：没有 composition 时只隐藏面板返回；否则 `keytao_session_commit_composition_json()` 提交、应用状态、隐藏面板。若 librime 仍留着 composition，再补一次 `clear_composition` 与空 `setMarkedText`，保证"该会话已结束"在两侧一致。
- `deactivateServer(_:)`（失焦、切换输入源、切换文本框）：按 `sender` 取 client 后走同一条**提交**路径，再重置 `lastModifierFlags` / `shiftPressedWithoutKey`。改动前这里只 reset librime、不碰 client，客户端会残留带下划线的预编辑。
- `cancelComposition()`：走**丢弃**路径（`keytao_session_clear_composition_json()`）并清 client 的 marked text。不调用 `super`，因为 KeyTao 未实现 `originalString`，默认实现会把空串写回客户端。

失焦处置矩阵按通用层约定：需要提交 → `commit_composition()`；需要丢弃 → `clear_composition()`。不再伪造 `XK_Return` / `XK_Escape`。

`hidePalettes()` 已实现：系统要求收起输入法 UI 时隐藏候选窗与模式提示，再调 `super`。两个面板都是 `hidesOnDeactivate=false` 的高层 NSPanel，不实现这个方法它们不会自己消失。

鼠标点击提交目前**没有**接入：`recognizedEvents` 只声明 `keyDown | flagsChanged`，因此 IMK 内建的"点击组字区外自动 commitComposition:"不生效，`IMKMouseHandling` 的回调也不会被派发。之前那份 `mouseDown(onCharacterIndex:)` 实现是死代码，已删除。若要恢复该能力，需要在 `recognizedEvents` 里加 `leftMouseDown` 并在真实客户端上实测回调确实到达（Rime 官方前端 Squirrel 同样只声明 `keyDown | flagsChanged`，这是业界通行取舍）。

## 候选窗

`CandidatePanel` 是一个 borderless nonactivating `NSPanel`：

- 只消费通用层的 `CandidatePanelModel`（经 `keytao_session_*_json`）：label 文本、高亮候选、comment 过滤、翻页可用性、面板方向全部由 `keytao-theme` 算好，AppKit 层只做 model → NSView 映射。label 兜底（`select_keys` 用尽时用序号）和高亮 clamp 在 Swift 里已经没有第二份实现。
- 使用 `NSStackView` 按 model 的 `orientation` 横向或纵向排列候选；macOS 通过 `keytao_set_ui_capabilities` 声明支持竖排，所以拿到的是主题里配置的方向。
- 每个候选是自定义 `NSControl`，样式由 `ImeTheme` 驱动，点击后按 model 里的 `index` 调用 `keytao_session_select_candidate_json(session, index)`。
- `navigation.canGoPrevious` / `canGoNext` 决定是否显示自绘翻页按钮，点击后调用 `keytao_session_change_page_json(session, backward)`。
- comment 字号、颜色和选中态颜色由主题控制。
- 位置来自 `cursorRect(for:)`，无法取得可用光标 rect 时 fallback 到鼠标位置。
- 会限制在当前屏幕 visible frame 内。
- 窗口层级取 `client.windowLevel() + 1`（`IMKTextInput` 头文件对自绘候选窗的要求），并且不低于 `.popUpMenu`；客户端返回非正值时退回 `.popUpMenu`。`ModeIndicatorPanel` 用同一规则。

## 光标定位

`resolveCursorRect(for:)` 的优先级（`lastPreeditCursor` 已是 UTF-16 偏移）：

1. `client.attributes(forCharacterIndex: lastPreeditCursor, lineHeightRectangle:)`
2. 当前 marked range 加 `lastPreeditCursor` 的 `client.firstRect(forCharacterRange:actualRange:)`
3. 当前 selected range 的 `client.firstRect(forCharacterRange:actualRange:)`
4. range 0 的 `client.firstRect(forCharacterRange:actualRange:)`
5. 全部不可用时返回 nil，`cursorRect(for:)` 复用最近一次有效 rect，再不行返回 `.zero`，候选窗/模式提示最后 fallback 到鼠标位置

Apple 官方 SDK 头文件对这两个接口的坐标契约很明确：

- `NSTextInputClient.firstRectForCharacterRange(_:actualRange:)` 返回 screen coordinate。
- `IMKTextInput.firstRectForCharacterRange(_:actualRange:)` 返回 global coordinate。
- `IMKTextInput.attributesForCharacterIndex(_:lineHeightRectangle:)` 的 line rect 供输入法放置 candidate window。

因此 KeyTao 不再使用前台窗口 frame 做任何自定义坐标转换。`cursorRect(for:)` 只接受客户端返回的 screen/global rect；宽度为 0、高度有效的插入光标 rect 会用 1px lookup rect 查找屏幕，明显落在屏幕角落的缺省 rect 会被拒绝。客户端短暂返回无效 rect 时，候选窗会复用最近一次有效插入点。

官方依据在本机 SDK：

- `AppKit.framework/Headers/NSTextInputClient.h`
- `Carbon.framework/Frameworks/HIToolbox.framework/Headers/IMKInputSession.h`

## Shift 与中英模式

`handleFlagsChanged()` 只关心 Shift：

1. 只要新的 modifier 集合里出现 Command/Control/Option，立即清掉 `shiftPressedWithoutKey`。这条不能只靠松开时读 `lastModifierFlags`：像 Cmd+Shift+4 这类系统热键，其 keyDown 根本不会派发给输入法。
2. Shift 按下时，只有当时没有其它修饰键才设置 `shiftPressedWithoutKey=true`，然后放行。
3. 任何 keyDown 都会清掉 `shiftPressedWithoutKey`（见「按键处理」第 3 条）。
4. Shift 松开时，只有“前后都没有 Command/Control/Option 混入且期间没有其它 keyDown”的 solo Shift 才继续处理。
5. 左/右 Shift 分别传 `XK_Shift_L`/`0xffe1` 和 `XK_Shift_R`/`0xffe2`。
6. modifiers 传 `rimeReleaseMask`。
7. 如果 librime accepted，则应用状态并按 `modeHint` 显示 `ModeIndicatorPanel`。
8. 如果 librime 不接受，则 fallback 到 `keytao_session_set_ascii_mode_json(session, !asciiMode)`。

`ModeIndicatorPanel` 是主题驱动的 nonactivating `NSPanel`，默认 72x48，文案取自通用层 `ModeHintModel.text`（默认 `中` / `英`），默认 0.75 秒后自动隐藏。

## 输入法菜单

`InputController.menu()` 当前提供：

- `Redeploy KeyTao`：调用 `keytao_reload()`，隐藏候选，刷新 state，并播放 Glass 音效。
- `Open KeyTao App`：优先打开 bundle id `ink.rea.keytao-app`，否则打开 `/Applications/KeyTao.app`。

Rime 自身的 schema / options 菜单不由 `InputController.menu()` 重新实现。按键路径会把 `F4` 映射为 `XK_F4` / `0xffc1` 送入 librime，候选窗按通用候选模型显示 Rime 生成的菜单候选。

## App 对接点

正常用户路径里，macOS 输入法 bundle 应随主 App 的 pkg 一起安装、升级和移除；用户不应该在 App 内再看到“安装输入法 / 卸载输入法”这类系统组件管理按钮。App 只应该承担状态展示、方案安装/部署、reload 通知和必要诊断，降低用户理解负担。

Tauri 主 App 当前已有 macOS 相关命令：

- `macos_ime_status`：只检查 `/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME` 是否存在。
- `macos_install_ime`：仅 debug build 可运行仓库内 `crates/keytao-macos-ime/install.sh --release`，release build 会拒绝该开发接口。
- `macos_uninstall_ime`：仅 debug build 可执行 `/Library/Input Methods/KeyTao.app` 移除，release build 会拒绝该开发接口。
- `rime_deploy_default`：部署后写 `~/Library/keytao/keytao-ime.reload`。

React 页面只在初始化时调用 `macos_ime_status` 做状态展示；正式 UI 不提供刷新、安装或卸载 macOS 输入法 bundle 的按钮，避免形成 App 内重复安装入口。

## 跨平台前端契约

macOS 前端要和 Linux/Windows 等系统输入法前端共享同一套实现边界。稳定边界应是：

1. `keytao-core` 只负责 librime 初始化、部署、session、按键处理和 `ImeState` 抽取。
2. 平台前端只负责系统协议接入、原生按键事件转换、文本提交、预编辑更新、候选 UI、模式提示和诊断。
3. 每个输入上下文必须通过 `keytao-core-ffi` 创建独立 `ImeRuntimeSession`；全局 librime 初始化可以按进程复用。
4. 原生按键必须先转换成 librime 期望的 X11 keysym + Rime modifier mask，再送入 core。
5. `accepted=false` 的按键应尽量放行给客户端；没有 composition 时，导航键、删除键、空格、回车、Tab、Escape 不应被输入法误截获。Ctrl/Option 组合键要先送 Rime，由 `accepted` 决定是否放行。
6. 应用 `ImeState` 的顺序必须固定：需要提交文本时先清旧 preedit，再提交 `committed`，再设置新的 `preedit`，最后更新候选窗和 `ascii_mode`。
7. reload 只通过用户数据目录下的 `keytao-ime.reload` 通知；前端收到变化后重建 session 并刷新 UI，不把部署逻辑散到候选窗或菜单里。
8. UI 不应直接读取或修改 Rime session 内部状态；候选选择、翻页、reset、ascii mode 切换必须通过 core/FFI 提供的 session API。

这套契约是接入 `theme.yaml`、Windows TSF、以及更多 Linux 后端时的稳定基线。平台文档和代码都应围绕这个边界演进，避免把引擎、系统协议、UI 样式混在同一层里。

## 统一 `theme.yaml` 接入方式

macOS 候选窗和模式提示接入了通用 `keytao-theme` 主题层：

- 默认主题源文件在 `crates/keytao-theme/default-theme.yaml`，构建时复制到 `KeyTao.app/Contents/Resources/default-theme.yaml`。
- 用户覆盖主题路径是 `~/Library/keytao/theme.yaml`。
- 开发覆盖路径可用环境变量 `KEYTAO_IME_THEME_PATH`。
- `ImeThemeManager` 不解析 YAML；它调用 `keytao_resolve_theme_json_with_system_scheme(defaultPath, userPath, scheme)` 获取 Rust 通用层合并、校验后的 normalized JSON。
- `ImeThemeManager.installThemeSources()` 在进程启动时把同一组路径通过 `keytao_set_theme_paths()` 注入通用层，这样 `keytao_session_*_json` 产出的 `candidatePanel` / `modeHint` 用的是用户主题而不是内置默认值。
- 系统明暗由 macOS 侧读 `NSApp.effectiveAppearance` 后用 `keytao_set_system_color_scheme()` 推给通用层（配色变化时才推一次），避免 `keytao-theme` 在输入法进程里 fork `defaults` 探测。
- macOS 通过 `keytao_set_ui_capabilities(true, true, true, true, true, false)` 声明自绘面板能力；不声明时通用层默认按软键盘候选条形状产出模型（横排）。
- `CandidatePanel.swift` 和 `ModeIndicatorPanel.swift` 只消费 `ResolvedImeTheme` 与 `CandidatePanelModel` / `ModeHintModel` 对应的 Swift DTO，负责映射到 AppKit。

当前 macOS 层落成三层：

1. `keytao-theme`：读取共享配置，合并默认值，校验类型和范围，输出平台无关的 `ResolvedImeTheme` 与 `CandidatePanelModel` / `ModeHintModel`。
2. FFI adapter：`keytao-core-ffi` 暴露 `keytao_resolve_theme_json*()` 与带 UI model 的 `keytao_session_*_json()`，Swift 通过 `Codable` 解成 DTO。
3. AppKit renderer：把 `ImeTheme + KeyTaoImeState` 映射到 `NSColor`、`NSFont`、spacing、padding、corner radius、shadow、highlight、comment、mode hint。

`theme.yaml` v2 应只表达跨平台可落地的语义，不把 AppKit 或 Linux SHM 细节写进配置：

- UI 模式：`ui.colorScheme: auto | light | dark`；`auto` 跟随系统主题并在 resolved JSON 中给出最终 `effectiveColorScheme`。
- 主题强调色：`ui.accentColor`，用于派生候选高亮、hover 和模式提示强调色。
- 模式变体：`light:` / `dark:` 下的字体、面板、候选、导航和模式提示覆盖项。
- 字体族、字号、字体粗细。
- 候选窗方向、padding、gap、圆角、边框、阴影、最大宽度。
- 背景、前景、注释、label、highlight、hover、separator 颜色。
- 模式提示尺寸、圆角、持续时间、中/英文字和颜色。
- 候选 label 规则由通用层的 `CandidatePanelModel` 决定：`select_keys` 为空时 label 兜底到 `1234567890`，用尽时用序号。**该兜底只用于 label，不得用于按键拦截**（macOS 本来就不拦截数字键，数字一律送 Rime，由方案的 `select_keys` / `key_binder` 决定）。

macOS renderer 要保留系统适配能力：当字体或颜色缺失时使用系统字体和动态颜色；当光标 rect 不可信时仍 fallback 到鼠标位置；当屏幕空间不足时仍限制在 visible frame。也就是说，`theme.yaml` 控制视觉，不接管系统可靠性策略。

## 与 Linux 实现的关键差异

| 维度 | macOS IMK | Linux daemon |
| --- | --- | --- |
| 进程模型 | 系统按需启动 `/Library/Input Methods/KeyTao.app`，每个 `IMKInputController` 一个 session | App 启动/重启独立 `keytao-ime` daemon，daemon 内按后端/上下文创建 session |
| 系统协议 | `InputMethodKit` + TIS 输入源 | Wayland input-method-v2、KDE input-method-v1、GNOME IBus engine、IBus D-Bus shim、X11 XIM |
| 文本提交 | `IMKTextInput.insertText`、`setMarkedText` | 各后端原生提交：Wayland `commit_string`、KDE context、IBus signals、XIM commit |
| 候选 UI | 自有 AppKit `NSPanel`，可完整主题化 | 自绘 SHM/X11 overlay 可完整主题化；IBus/Kimpanel 系统候选服务只能表达有限结构 |
| 光标定位 | IMK client rect + 前台窗口转换 + 鼠标 fallback | 协议提供 text rectangle / spot location / compositor popup surface，能力按后端不同 |
| 重载 | 后台队列上的 `DispatchSource` 监听用户目录 + `activateServer` 兜底，走 `keytao_reload_if_stamp_changed()`；按键路径无文件 I/O | daemon watcher 轮询 reload stamp，session 按 generation 懒刷新 |
| 日志 | 当前主要 `NSLog` 到系统日志 | `/tmp/keytao-ime.log` 滚动日志，App 可读取 |
| 模式提示 | AppKit HUD，Shift release 或 fallback 切换后显示 | input-method-v2 有自绘 hint；KDE 目前只记日志；IBus 系统通道未统一 hint |

因此统一主题时，macOS 和 Linux 不能共享“渲染实现”，但应共享“输入模型、主题语义和 fallback 规则”。Mac 端负责把主题语义映射到 AppKit；Linux 自绘通道负责映射到像素 buffer；系统候选服务只能尽量映射文字、label、highlight/page 信息。

## 后续补齐顺序

建议按风险从低到高补齐：

1. ~~先抽出平台无关的候选/模式提示模型字段~~ 已完成：macOS 现在消费 `CandidatePanelModel` / `ModeHintModel`。
2. 增强 `macos_ime_status`：检查 bundle、TIS 注册、主输入源 enabled/selectable/current、旧输入源残留。
3. 如需支持 CapsLock 切换，再补 `flagsChanged` 状态同步并重新声明 `TICapsLockLanguageSwitchCapable`。
4. 再补 schema 切换、选项开关、周边文本、鼠标 hover/滚轮等体验能力。

## librime 按键兼容基线

macOS 前端要尽量模拟 Linux/X11 传给 librime 的事件形状：

- keyval 表达“实际字符”的 X11 keysym，modifier mask 表达“同时按住的修饰键”。
- Shift+a 在中文状态下应传 `0x41` 加 Shift mask，让 librime 的 ASCII composer 处理大写首字母或符号输入。
- Ctrl/Option 组合键应保留为基键加 Control/Alt mask，先送 Rime，由 `accepted` 决定是否放行给应用（Rime 的 `key_binder`、`Ctrl+grave` 之类的绑定要能收到）。
- Command 映射为 Super mask（`1 << 26`）；作为窗口系统保留组合，macOS 仍提前放行，见「按键处理」的偏离登记。
- solo Shift release 才用于中英模式切换；Shift+字母、Shift+数字符号必须清掉 `shiftPressedWithoutKey`，走普通 key press 路径。
- CapsLock 不进入 Rime modifier mask；如果系统布局已经产出大写字符，keyval 本身可以是大写 keysym。
- 非 Latin-1 字符按 X11 约定编码为 `0x01000000 | codepoint`，由 `keytao_text_to_keysym()` 统一产出；Rime 不接受时 `handle` 返回 false，字符由 macOS 原生路径上屏，不会丢字。

## 当前已接入能力

- 系统输入法 bundle 打包、注册、启用、选择；bundle 版本随 workspace 版本写入。
- 每个输入上下文独立 Rime session。
- App 部署后的 reload stamp 刷新（文件事件驱动，不在按键路径）。
- 中文 composition 的 marked text 更新，含 UTF-16 偏移换算与已转换/正在转换/原始编码三段标注。
- commit、preedit、candidate、highlight、page、ascii_mode 状态读取（走 FFI JSON，候选面板与模式提示直接消费通用层 UI model）。
- 失焦提交、`commitComposition:` 提交、`cancelComposition` 丢弃、`hidePalettes` 收起 UI。
- 候选点击选择。
- 候选翻页按钮。
- 候选窗/模式提示按 `client.windowLevel() + 1` 抬升层级。
- Shift release 中英切换与 fallback 手动切换。
- Shift+字母大写输入兼容。
- 小键盘 Enter 映射为 `XK_KP_Enter`；无法映射的按键在有 composition 时先提交再放行。
- 输入法菜单里的 redeploy 和打开主 App。

## Mac 端仍未对接或待补齐

1. Mac IME 日志采集仍需补系统日志  
   App 的 `read_debug_logs` 已能返回 `~/Library/keytao/log` 下的 librime 日志，但 IMK 层 `NSLog` 仍主要在系统日志中，尚未接入统一诊断入口。

2. 输入源状态检测仍偏浅  
   `macos_ime_status` 已展示 user/shared/reload/log 目录信息，但输入源安装状态仍主要看可执行文件是否存在；没有检查 TIS 是否已注册、主输入源是否 enabled/selectable、当前是否选中，也没有展示旧输入源残留。

3. CapsLock 切换尚未实现  
   当前未声明 `TICapsLockLanguageSwitchCapable`。如果后续要支持 CapsLock 中英切换，需要补 `flagsChanged` 同步、Rime `ascii_mode` 状态规则，并避免和 solo Shift 切换冲突。

4. 原生菜单缺少 Rime 常用操作
   `F4` 已通过 librime 打开 Rime schema / options 菜单；macOS 输入源菜单本身目前仍只有 Redeploy 和 Open App，尚未把同步用户数据、重置用户词典、显示当前 schema/status 等做成原生菜单项。

5. 候选键盘操作未做 Mac 侧增强  
   数字选词、方向键、PageUp/PageDown 主要依赖 librime 处理。候选窗按钮可点击，但没有实现 `candidateClicked` 以外的 Mac 原生候选交互、鼠标 hover 高亮（`keytao_session_highlight_candidate_json` 已可用但未接）、滚轮翻页等体验。

6. 周边文本能力未接  
   当前没有读取或传递 surrounding text。若以后接入需要上下文的 Lua/filter/translator、智能删除、跨段落联想等能力，需要补 IMKTextInput 周边文本读取策略。

7. IME-only 构建不是正式发行路径
   `crates/keytao-macos-ime/build.sh` 主要用于开发和单独调试输入法 bundle；正式发行必须走 `scripts/build-macos.sh`，确保主 App、IME bundle、`rime-data`、OpenCC 和 `rime-plugins` 一起进入 pkg。

8. 缺少轻量健康检查  
    reload stamp 能刷新 session，但 App 还没有对当前 IME 进程、TIS 注册状态和 reload 成功状态做健康检查。即使后续需要修复动作，也应优先设计为自动恢复或诊断提示，而不是暴露安装/卸载类按钮。

9. 鼠标点击组字区外自动提交未接  
   见「Composition 与提交」：需要在 `recognizedEvents` 加 `leftMouseDown` 并在真实客户端上实测 `IMKMouseHandling` 回调确实到达，才谈得上恢复。

10. 面板的 `collectionBehavior` 未设置  
    候选窗/模式提示没有声明 `canJoinAllSpaces` / `fullScreenAuxiliary`。Squirrel 同样没有设置，Apple 也无硬性要求，需要在跨 Space、全屏场景实测确认确实失效后再加。

11. `macos_ime_status` 未返回 IME bundle 版本  
    bundle 版本现在随 workspace 版本写入 Info.plist，但 App 侧的 `macos_ime_status_inner`（`src-tauri/src/lib.rs`）仍只检查可执行文件是否存在，无法用它发现主 App 与 IME bundle 的版本偏斜。该文件不在本 crate 的所有权范围内。

12. 敏感输入策略未接  
    macOS 上系统 secure event input 会在密码框里屏蔽第三方输入法，因此 `InputContextPolicy` 没有接。已确认 IMK 层的 `NSLog` 不输出按键内容、keysym 或提交文本。

## 顶功排查点

顶功可能在一次按键结果里同时返回 `committed` 和新的 `preedit`。macOS IMKit 对 marked range 很敏感，旧 marked text 没先清理时，`insertText` 可能替换旧 range，表现为前一个字被顶上去或提交位置异常。

排查优先看：

- `InputController.apply(_:, to:)`
- `clearMarkedText(client:)`
- `updateMarkedText(_:client:)`
- `hasComposition` 更新时机
- 同一次状态里的 `committed` 和 `preedit`

当前实现要求：

- 有 `committed` 时，先清空旧 marked text。
- 再 `insertText(committed)`。
- 最后设置新的 `preedit`。
- 分别测试 commit-only、preedit-only、commit + new preedit 三种状态。
