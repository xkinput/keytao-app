# macOS IME 实现说明

本文只记录 `crates/keytao-macos-ime` 里的 macOS 系统输入法实现，并按当前代码同步。

跨平台通用契约见 [输入法通用层实现规范](../../docs/ime-common-layer.md)；本文只补充 macOS IMK/TIS 的协议、打包和 AppKit UI 差异。

## 代码地图

- `Sources/KeyTaoIME/main.swift`：Swift 可执行入口、输入源管理命令、`IMKServer` 创建、accessory app run loop。
- `Sources/KeyTaoIME/IMKSetup.swift`：保留的 C ABI setup helper，目前不是 `build.sh` 产物的主入口。
- `Sources/KeyTaoIME/InputSourceInstaller.swift`：TIS 注册、启用、选择、禁用旧输入源、列出 KeyTao 输入源。
- `Sources/KeyTaoIME/EngineInit.swift`：用户目录/共享目录解析、调用 FFI 初始化通用 runtime、reload stamp 检测。
- `Sources/KeyTaoIME/InputController.swift`：`IMKInputController` 子类，处理按键、composition、候选窗、模式切换。
- `Sources/KeyTaoIME/CandidatePanel.swift`：候选 NSPanel。
- `Sources/KeyTaoIME/ModeIndicatorPanel.swift`：中英模式提示 NSPanel。
- `Resources/Info.plist`：bundle 元信息、输入源 id、IMK controller/delegate class、图标和 TIS 输入模式声明。
- `build.sh`：构建 IME-only bundle/pkg。
- `install.sh`：开发用本机安装脚本。
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
5. 复制 `Info.plist`、图标、本地化 `InfoPlist.strings`。
6. 复制 `libkeytao_core_ffi.dylib`、`librime*.dylib` 和 rime plugins。
7. 生成 `Sources/CKeytaoCore/module.modulemap`。
8. 用 `swiftc` 编译所有 Swift 文件为 `Contents/MacOS/KeyTaoIME`。
9. 对 dylib 和 bundle 签名。
10. 默认生成只安装 IME bundle 的 pkg；`--skip-pkg` 可跳过。

`scripts/build-macos.sh` 做完整发行 pkg：

1. 构建 IME runtime。
2. 准备主 App runtime，把 `rime-data`、`librime.1.dylib`、OpenCC 数据和 `rime-plugins` 放进 Tauri 资源/Frameworks 目录。
3. 构建 Tauri 主 App。
4. 确认主 App bundle id 是 `ink.rea.keytao-app`，IME bundle id 是 `ink.rea.inputmethod.keytao`。
5. 在签名前把 `rime-plugins` 和插件依赖补进主 App `Contents/Frameworks`，保证主 App 部署 Lua 方案和 IME 运行时使用同等能力。
6. 重签主 App 及 dylib。
7. 打包 `/Applications/KeyTao.app` 和 `/Library/Input Methods/KeyTao.app`。
8. `postinstall` 运行 `lsregister`、清理 quarantine/provenance xattr、注册/启用/选择输入源。

本地完整打包和离线验证命令：

```sh
pnpm install
pnpm build:macos
scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg
```

测试安装命令：

```sh
sudo installer -pkg target/keytao-macos-pkg/KeyTao.pkg -target /
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
2. `NSApplication.shared` 设置为 `.accessory` 并进入 run loop。
3. macOS 通过 `imklaunchagent` 按需连接该 IMK server。
4. 每个输入上下文创建一个 `KeyTaoInputController`。
5. controller 初始化时调用 `ensureEngineReady()`。
6. `ensureEngineReady()` 触发 lazy 全局，只在进程生命周期内首次调用 `initializeEngine()`。
7. `initializeEngine()` 解析 user/shared dir 后调用 `keytao_init(userDir, sharedDir)`。
8. 每个 controller 创建独立 `keytao_create_session()`。
9. controller deinit 时调用 `keytao_destroy_session()`。

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

macOS IME 进程不会用文件 watcher，而是在这些时机检查 stamp 内容：

- `activateServer(_:)`
- 每次 `handle(_:client:)` 进入时

内容变化且非空时：

1. 如果有 composition，先清空 marked text。
2. 隐藏候选窗和模式提示。
3. 销毁当前 Rime session。
4. 调用 `keytao_reload()`，由通用 runtime 重新部署并递增 generation。
5. 创建新 session。
6. 读取新 session state 并刷新候选/状态。

因此用户在 App 里重新部署后，不需要手动重启输入法；下一次激活或按键会触发刷新。

## 按键处理

热路径在 `InputController.swift`。

1. `recognizedEvents` 只声明 `keyDown` 和 `flagsChanged`。
2. `handle(_:client:)` 每次先检查 reload stamp。
3. `flagsChanged` 交给 `handleFlagsChanged()`。
4. Command 组合键直接放行。
5. 如果 keyDown 时 Shift 仍按住，清掉 `shiftPressedWithoutKey`，避免 Shift+字母被当作 solo Shift。
6. `asciiMode && !hasComposition` 时直接放行，让系统输入英文。
7. Carbon 特殊键转换为 X11 keysym：Return、Backspace、Delete、Escape、Space、方向键、Home/End、PageUp/PageDown、Tab。
8. 可打印 ASCII 先尝试 `event.characters`，保留当前布局实际产生的字符；例如 Shift+a 传 `XK_A`/`0x41`。
9. Command、Control、Option 组合键使用 `charactersIgnoringModifiers` 取得基键，再用 modifier mask 表达修饰键。
10. Shift、Control、Option 转成 Rime modifier mask。
11. 没有 composition 时，空格、删除、Tab、回车、Escape、导航键和 Ctrl/Option 组合键放行。
12. 调用 `keytao_session_process_key(session, keyval, modifiers)`。
13. `accepted` 原样作为 `handle` 返回值。

## Composition 与提交

`apply(state, to:)` 的顺序：

1. 如果 `committed` 非空且当前有 composition，先用空 `setMarkedText` 清掉旧 marked range。
2. 调用 `insertText(committed)` 提交文本。
3. 调用 `setMarkedText` 写入新的 `preedit`，selection 使用 librime 返回的 `cursor`。
4. 用 `preedit` 或 candidates 是否为空更新 `hasComposition`。
5. 同步 `asciiMode`。
6. candidates 为空则隐藏候选窗，否则显示/更新候选窗。

`commitComposition(_:)` 通过 `keytao_session_process_key(session, rimeKeyReturn, 0)` 尝试提交，然后隐藏候选窗；这里和普通 keyDown 路径一样使用 `XK_Return`/`0xff0d`，不再传 Carbon 虚拟键码。

`cancelComposition()` 调用 `keytao_session_reset()` 并隐藏候选窗。

鼠标点击 marked text 时，如果有 composition，会先 `commitComposition(sender)`，然后把事件放回客户端。

## 候选窗

`CandidatePanel` 是一个 borderless nonactivating `NSPanel`：

- 使用 `NSStackView` 按主题配置横向或纵向排列候选。
- 每个候选是自定义 `NSControl`，样式由 `ImeTheme` 驱动，点击后调用 `keytao_session_select_candidate(session, index)`。
- 如果有上一页/下一页，显示自绘翻页按钮，点击后调用 `keytao_session_change_page(session, backward)`。
- 候选 label 使用 librime `select_keys`，为空时 fallback 到 `1234567890`。
- comment 字号、颜色和选中态颜色由主题控制。
- 位置来自 `cursorRect(for:)`，无法取得可用光标 rect 时 fallback 到鼠标位置。
- 会限制在当前屏幕 visible frame 内。

## 光标定位

`cursorRect(for:)` 的优先级：

1. 当前 marked range 加 `lastPreeditCursor` 的 `client.firstRect(forCharacterRange:actualRange:)`
2. 当前 selected range 的 `client.firstRect(forCharacterRange:actualRange:)`
3. range 0 的 `client.firstRect(forCharacterRange:actualRange:)`
4. `client.attributes(forCharacterIndex:lineHeightRectangle:)`
5. fallback 到 `.zero`，候选窗/模式提示再 fallback 到鼠标位置

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

1. Shift 按下时设置 `shiftPressedWithoutKey=true` 并放行。
2. 如果期间有 keyDown 且仍按着 Shift，会清掉 `shiftPressedWithoutKey`。
3. Shift 松开时，只有“没有 Command/Control/Option 混入且期间没有其它 keyDown”的 solo Shift 才继续处理。
4. 左/右 Shift 分别传 `XK_Shift_L`/`0xffe1` 和 `XK_Shift_R`/`0xffe2`。
5. modifiers 传 `rimeReleaseMask`。
6. 如果 librime accepted，则应用状态并显示 `ModeIndicatorPanel`。
7. 如果 librime 不接受，则 fallback 到 `keytao_session_set_ascii_mode(session, !asciiMode)`。

`ModeIndicatorPanel` 是主题驱动的 nonactivating `NSPanel`，默认 72x48，显示 `英` 或 `中`，默认 0.75 秒后自动隐藏。

## 输入法菜单

`InputController.menu()` 当前提供：

- `Redeploy KeyTao`：调用 `keytao_reload()`，隐藏候选，刷新 state，并播放 Glass 音效。
- `Open KeyTao App`：优先打开 bundle id `ink.rea.keytao-app`，否则打开 `/Applications/KeyTao.app`。

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
5. `accepted=false` 的按键应尽量放行给客户端；没有 composition 时，导航键、删除键、空格、回车、Tab、Escape、Ctrl/Option 组合键不应被输入法误截获。
6. 应用 `ImeState` 的顺序必须固定：需要提交文本时先清旧 preedit，再提交 `committed`，再设置新的 `preedit`，最后更新候选窗和 `ascii_mode`。
7. reload 只通过用户数据目录下的 `keytao-ime.reload` 通知；前端收到变化后重建 session 并刷新 UI，不把部署逻辑散到候选窗或菜单里。
8. UI 不应直接读取或修改 Rime session 内部状态；候选选择、翻页、reset、ascii mode 切换必须通过 core/FFI 提供的 session API。

这套契约是接入 `theme.yaml`、Windows TSF、以及更多 Linux 后端时的稳定基线。平台文档和代码都应围绕这个边界演进，避免把引擎、系统协议、UI 样式混在同一层里。

## 统一 `theme.yaml` 接入方式

macOS 候选窗和模式提示接入了通用 `keytao-theme` 主题层：

- 默认主题源文件在 `crates/keytao-theme/default-theme.yaml`，构建时复制到 `KeyTao.app/Contents/Resources/default-theme.yaml`。
- 用户覆盖主题路径是 `~/Library/keytao/theme.yaml`。
- 开发覆盖路径可用环境变量 `KEYTAO_IME_THEME_PATH`。
- `ImeThemeManager` 不解析 YAML；它调用 `keytao_resolve_theme_json(defaultPath, userPath)` 获取 Rust 通用层合并、校验后的 normalized JSON。
- `CandidatePanel.swift` 和 `ModeIndicatorPanel.swift` 只消费 `ResolvedImeTheme` 对应的 Swift DTO，负责映射到 AppKit。

当前 macOS 层落成三层：

1. `keytao-theme`：读取共享配置，合并默认值，校验类型和范围，输出平台无关的 `ResolvedImeTheme`。
2. FFI adapter：`keytao-core-ffi` 暴露 `keytao_resolve_theme_json()`，Swift 通过 `Codable` 解成 DTO。
3. AppKit renderer：把 `ResolvedImeTheme + KeyTaoStateView` 映射到 `NSColor`、`NSFont`、spacing、padding、corner radius、shadow、highlight、comment、mode hint。

`theme.yaml` 第一版应只表达跨平台可落地的语义，不把 AppKit 或 Linux SHM 细节写进配置：

- 字体族、字号、字体粗细。
- 候选窗方向、padding、gap、圆角、边框、阴影、最大宽度。
- 背景、前景、注释、label、highlight、hover、separator 颜色。
- 模式提示尺寸、圆角、持续时间、中/英文字和颜色。
- 候选 label 规则仍来自 Rime `select_keys`，为空时统一 fallback 到 `1234567890`。

macOS renderer 要保留系统适配能力：当字体或颜色缺失时使用系统字体和动态颜色；当光标 rect 不可信时仍 fallback 到鼠标位置；当屏幕空间不足时仍限制在 visible frame。也就是说，`theme.yaml` 控制视觉，不接管系统可靠性策略。

## 与 Linux 实现的关键差异

| 维度 | macOS IMK | Linux daemon |
| --- | --- | --- |
| 进程模型 | 系统按需启动 `/Library/Input Methods/KeyTao.app`，每个 `IMKInputController` 一个 session | App 启动/重启独立 `keytao-ime` daemon，daemon 内按后端/上下文创建 session |
| 系统协议 | `InputMethodKit` + TIS 输入源 | Wayland input-method-v2、KDE input-method-v1、GNOME IBus engine、IBus D-Bus shim、X11 XIM |
| 文本提交 | `IMKTextInput.insertText`、`setMarkedText` | 各后端原生提交：Wayland `commit_string`、KDE context、IBus signals、XIM commit |
| 候选 UI | 自有 AppKit `NSPanel`，可完整主题化 | 自绘 SHM/X11 overlay 可完整主题化；IBus/Kimpanel 系统候选服务只能表达有限结构 |
| 光标定位 | IMK client rect + 前台窗口转换 + 鼠标 fallback | 协议提供 text rectangle / spot location / compositor popup surface，能力按后端不同 |
| 重载 | 激活或按键时比较 reload stamp 内容，调用 `keytao_reload()` 后由 runtime generation 懒刷新 session | daemon watcher 每秒看 reload stamp mtime，session 按 generation 懒刷新 |
| 日志 | 当前主要 `NSLog` 到系统日志 | `/tmp/keytao-ime.log` 滚动日志，App 可读取 |
| 模式提示 | AppKit HUD，Shift release 或 fallback 切换后显示 | input-method-v2 有自绘 hint；KDE 目前只记日志；IBus 系统通道未统一 hint |

因此统一主题时，macOS 和 Linux 不能共享“渲染实现”，但应共享“输入模型、主题语义和 fallback 规则”。Mac 端负责把主题语义映射到 AppKit；Linux 自绘通道负责映射到像素 buffer；系统候选服务只能尽量映射文字、label、highlight/page 信息。

## 后续补齐顺序

建议按风险从低到高补齐：

1. 先抽出平台无关的候选/模式提示模型字段，保证 macOS、Linux 文档和代码都围绕同一个 UI 输入结构命名。
2. 增强 `macos_ime_status`：检查 bundle、TIS 注册、主输入源 enabled/selectable/current、旧输入源残留。
3. 如需支持 CapsLock 切换，再补 `flagsChanged` 状态同步并重新声明 `TICapsLockLanguageSwitchCapable`。
4. 再补 schema 切换、选项开关、周边文本、鼠标 hover/滚轮等体验能力。

## librime 按键兼容基线

macOS 前端要尽量模拟 Linux/X11 传给 librime 的事件形状：

- keyval 表达“实际字符”的 X11 keysym，modifier mask 表达“同时按住的修饰键”。
- Shift+a 在中文状态下应传 `0x41` 加 Shift mask，让 librime 的 ASCII composer 处理大写首字母或符号输入。
- Ctrl/Option 组合键应保留为基键加 Control/Alt mask；没有 composition 时直接放行，避免截获应用快捷键。
- solo Shift release 才用于中英模式切换；Shift+字母、Shift+数字符号必须清掉 `shiftPressedWithoutKey`，走普通 key press 路径。
- CapsLock 不进入 Rime modifier mask；如果系统布局已经产出大写字符，keyval 本身可以是大写 keysym。

## 当前已接入能力

- 系统输入法 bundle 打包、注册、启用、选择。
- 每个输入上下文独立 Rime session。
- App 部署后的 reload stamp 刷新。
- 中文 composition 的 marked text 更新。
- commit、preedit、candidate、highlight、page、select_keys、ascii_mode 状态读取。
- 候选点击选择。
- 候选翻页按钮。
- Shift release 中英切换与 fallback 手动切换。
- Shift+字母大写输入兼容。
- 输入法菜单里的 redeploy 和打开主 App。

## Mac 端仍未对接或待补齐

1. Mac IME 日志采集仍需补系统日志  
   App 的 `read_debug_logs` 已能返回 `~/Library/keytao/log` 下的 librime 日志，但 IMK 层 `NSLog` 仍主要在系统日志中，尚未接入统一诊断入口。

2. 输入源状态检测仍偏浅  
   `macos_ime_status` 已展示 user/shared/reload/log 目录信息，但输入源安装状态仍主要看可执行文件是否存在；没有检查 TIS 是否已注册、主输入源是否 enabled/selectable、当前是否选中，也没有展示旧输入源残留。

3. CapsLock 切换尚未实现  
   当前未声明 `TICapsLockLanguageSwitchCapable`。如果后续要支持 CapsLock 中英切换，需要补 `flagsChanged` 同步、Rime `ascii_mode` 状态规则，并避免和 solo Shift 切换冲突。

4. 菜单缺少 Rime 常用操作  
   目前只有 Redeploy 和 Open App。尚未接 schema 切换、选项开关、同步用户数据、重置用户词典、显示当前 schema/status 等功能。

5. 候选键盘操作未做 Mac 侧增强  
   数字选词、方向键、PageUp/PageDown 主要依赖 librime 处理。候选窗按钮可点击，但没有实现 `candidateClicked` 以外的 Mac 原生候选交互、鼠标 hover 高亮、滚轮翻页等体验。

6. 周边文本能力未接  
   当前没有读取或传递 surrounding text。若以后接入需要上下文的 Lua/filter/translator、智能删除、跨段落联想等能力，需要补 IMKTextInput 周边文本读取策略。

7. IME-only 构建不是正式发行路径
   `crates/keytao-macos-ime/build.sh` 主要用于开发和单独调试输入法 bundle；正式发行必须走 `scripts/build-macos.sh`，确保主 App、IME bundle、`rime-data`、OpenCC 和 `rime-plugins` 一起进入 pkg。

8. 缺少轻量健康检查  
    reload stamp 能刷新 session，但 App 还没有对当前 IME 进程、TIS 注册状态和 reload 成功状态做健康检查。即使后续需要修复动作，也应优先设计为自动恢复或诊断提示，而不是暴露安装/卸载类按钮。

## 顶功排查点

顶功可能在一次按键结果里同时返回 `committed` 和新的 `preedit`。macOS IMKit 对 marked range 很敏感，旧 marked text 没先清理时，`insertText` 可能替换旧 range，表现为前一个字被顶上去或提交位置异常。

排查优先看：

- `InputController.apply(_:, to:)`
- `clearMarkedText(client:)`
- `updateMarkedText(_:cursor:client:)`
- `hasComposition` 更新时机
- 同一次状态里的 `committed` 和 `preedit`

当前实现要求：

- 有 `committed` 时，先清空旧 marked text。
- 再 `insertText(committed)`。
- 最后设置新的 `preedit`。
- 分别测试 commit-only、preedit-only、commit + new preedit 三种状态。
