# macOS IME 实现说明

本文只记录 `crates/keytao-macos-ime` 里的 macOS 系统输入法实现，并按当前代码同步。

## 代码地图

- `Sources/KeyTaoIME/main.swift`：Swift 可执行入口、输入源管理命令、`IMKServer` 创建、accessory app run loop。
- `Sources/KeyTaoIME/IMKSetup.swift`：保留的 C ABI setup helper，目前不是 `build.sh` 产物的主入口。
- `Sources/KeyTaoIME/InputSourceInstaller.swift`：TIS 注册、启用、选择、禁用旧输入源、列出 KeyTao 输入源。
- `Sources/KeyTaoIME/EngineInit.swift`：librime 初始化、用户目录/共享目录解析、reload stamp 检测。
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
- `TICapsLockLanguageSwitchCapable=true`，表示输入源声明支持 CapsLock 语言切换；当前代码没有单独处理 CapsLock flagsChanged。
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
2. 准备主 App runtime，把 `rime-data` 放进 Tauri 资源目录。
3. 构建 Tauri 主 App。
4. 确认主 App bundle id 是 `ink.rea.keytao-app`，IME bundle id 是 `ink.rea.inputmethod.keytao`。
5. 重签主 App 及 dylib。
6. 打包 `/Applications/KeyTao.app` 和 `/Library/Input Methods/KeyTao.app`。
7. `postinstall` 运行 `lsregister`、清理 quarantine/provenance xattr、注册/启用/选择输入源。

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
4. 调用 `initializeEngine()` 重新部署/初始化 librime。
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

`commitComposition(_:)` 当前通过 `keytao_session_process_key(session, UInt32(kVK_Return), 0)` 尝试提交，然后隐藏候选窗。注意这里传的是 Carbon keycode，不是普通 keyDown 路径里的 `XK_Return`/`0xff0d`；这属于后续需要修正的兼容点。

`cancelComposition()` 调用 `keytao_session_reset()` 并隐藏候选窗。

鼠标点击 marked text 时，如果有 composition，会先 `commitComposition(sender)`，然后把事件放回客户端。

## 候选窗

`CandidatePanel` 是一个 borderless nonactivating `NSPanel`：

- 使用 `NSStackView` 横向排列候选。
- 每个候选是 `NSButton`，点击后调用 `keytao_session_select_candidate(session, index)`。
- 如果有上一页/下一页，显示 SF Symbols `chevron.left` / `chevron.right`，点击后调用 `keytao_session_change_page(session, backward)`。
- 候选 label 使用 librime `select_keys`，为空时 fallback 到 `1234567890`。
- comment 用更小字号和 secondary label color。
- 位置来自 `cursorRect(for:)`，无法取得可用光标 rect 时 fallback 到鼠标位置。
- 会限制在当前屏幕 visible frame 内。

## 光标定位

`cursorRect(for:)` 的优先级：

1. `client.attributes(forCharacterIndex:lineHeightRectangle:)`
2. `client.firstRect(forCharacterRange:actualRange:)`
3. fallback 到 `.zero`，候选窗/模式提示再 fallback 到鼠标位置

`normalizeTextInputRect` 会判断 rect 是否已经落在屏幕内；如果不像全局坐标，会尝试用当前前台窗口 frame 转换一次 bottom-left 坐标和 top-left 坐标。

前台窗口 frame 来自 `CGWindowListCopyWindowInfo`，只取当前 frontmost app、layer 0、on-screen window。

## Shift 与中英模式

`handleFlagsChanged()` 只关心 Shift：

1. Shift 按下时设置 `shiftPressedWithoutKey=true` 并放行。
2. 如果期间有 keyDown 且仍按着 Shift，会清掉 `shiftPressedWithoutKey`。
3. Shift 松开时，只有“没有 Command/Control/Option 混入且期间没有其它 keyDown”的 solo Shift 才继续处理。
4. 左/右 Shift 分别传 `XK_Shift_L`/`0xffe1` 和 `XK_Shift_R`/`0xffe2`。
5. modifiers 传 `rimeReleaseMask`。
6. 如果 librime accepted，则应用状态并显示 `ModeIndicatorPanel`。
7. 如果 librime 不接受，则 fallback 到 `keytao_session_set_ascii_mode(session, !asciiMode)`。

`ModeIndicatorPanel` 是 72x48 的 HUD 风格 nonactivating `NSPanel`，显示 `英` 或 `中`，0.75 秒后自动隐藏。

## 输入法菜单

`InputController.menu()` 当前提供：

- `Redeploy KeyTao`：调用 `initializeEngine()`，销毁当前 session，隐藏候选，刷新 state，并播放 Glass 音效。
- `Open KeyTao App`：优先打开 bundle id `ink.rea.keytao-app`，否则打开 `/Applications/KeyTao.app`。

## App 对接点

正常用户路径里，macOS 输入法 bundle 应随主 App 的 pkg 一起安装、升级和移除；用户不应该在 App 内再看到“安装输入法 / 卸载输入法”这类系统组件管理按钮。App 只应该承担状态展示、方案安装/部署、reload 通知和必要诊断，降低用户理解负担。

Tauri 主 App 当前已有 macOS 相关命令：

- `macos_ime_status`：只检查 `/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME` 是否存在。
- `macos_install_ime`：运行仓库内 `crates/keytao-macos-ime/install.sh --release`，属于开发/过渡接口，不应接入正式用户 UI。
- `macos_uninstall_ime`：`sudo rm -rf /Library/Input Methods/KeyTao.app`，属于开发/过渡接口，不应接入正式用户 UI。
- `rime_deploy_default`：部署后写 `~/Library/keytao/keytao-ime.reload`。

React 页面目前只调用 `macos_ime_status` 刷新状态，这是正确方向。后续应移除或隐藏安装/卸载命令，避免形成 App 内重复安装入口。

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

1. 清理安装/卸载过渡接口  
   后端仍有 `macos_install_ime` 和 `macos_uninstall_ime`，但正式产品不应在 App UI 暴露这些操作。输入法应跟随应用 pkg 安装、升级、卸载；后续应移除、隐藏或限定为开发模式命令，简化用户路径。

2. 没有 Mac IME 专用日志采集  
   IME 目前主要用 `NSLog` 写系统日志；App 的 `read_debug_logs` 读取的是 `/tmp/keytao-ime.log` 和 `/tmp/keytao-app.log`，更偏 Linux daemon。Mac 输入法排查还缺统一日志入口。

3. `commitComposition` 的 Return keyval 不一致  
   普通按键路径把 Return 转成 `0xff0d`，但 `commitComposition` 传 `UInt32(kVK_Return)`。应改成同一套 X11 keysym，避免某些客户端调用 commit composition 时 librime 不按预期提交。

4. CapsLock 只声明，未在代码路径里处理  
   `Info.plist` 有 `TICapsLockLanguageSwitchCapable=true`，但 `handleFlagsChanged()` 只处理 Shift。若要支持 CapsLock 切换或和 Rime switch_key 对齐，需要补 flagsChanged/状态同步逻辑。

5. 输入源状态检测较浅  
   `macos_ime_status` 只看可执行文件是否存在；没有检查 TIS 是否已注册、主输入源是否 enabled/selectable、当前是否选中，也没有展示旧输入源残留。

6. 菜单缺少 Rime 常用操作  
   目前只有 Redeploy 和 Open App。尚未接 schema 切换、选项开关、同步用户数据、重置用户词典、显示当前 schema/status 等功能。

7. 候选键盘操作未做 Mac 侧增强  
   数字选词、方向键、PageUp/PageDown 主要依赖 librime 处理。候选窗按钮可点击，但没有实现 `candidateClicked` 以外的 Mac 原生候选交互、鼠标 hover 高亮、滚轮翻页等体验。

8. 周边文本能力未接  
   当前没有读取或传递 surrounding text。若以后接入需要上下文的 Lua/filter/translator、智能删除、跨段落联想等能力，需要补 IMKTextInput 周边文本读取策略。

9. 共享数据目录打包链路仍需收敛  
   完整 `scripts/build-macos.sh` 会把 `rime-data` 放入主 App 资源；IME-only `build.sh` 主要打包 dylib/plugins，不直接复制 `rime-data`。IME runtime 目前靠 App 资源、输入法 bundle fallback 目录、Squirrel/Homebrew fallback。后续应明确发行包里 IME 进程的首选 shared data 来源。

10. 缺少轻量健康检查  
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
