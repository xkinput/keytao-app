# KeyTao 系统输入法实现审查与改进报告

审查日期：2026-06-19

## 本轮修复状态

修复日期：2026-06-19

### 修复与验证统计

统计口径：

- 问题总数计入本报告已列出的 Mac 主 App `librime` 版本显示 `unknown` 问题，以及 P1-1 到 P3-17 的平台输入法 bug、错误设计和工程化缺口，共 18 条。
- 真实系统验收项单独统计，不和“代码是否已修复”混算。

| 项目 | 数量 | 当前状态 |
| --- | ---: | --- |
| 审查问题总数 | 18 | Mac `librime` 版本问题 + P1-1 到 P3-17 |
| 已修复 | 18 | 已完成代码或文档落地 |
| 待修复 | 0 | 当前报告内无仍停留在待修复状态的问题 |
| 本机 macOS 自动验证通过 | 14 | 编译、测试、前端构建、macOS pkg 构建/校验、版本注入、Info.plist 和 host target 检查已通过 |
| 待人工或目标系统验证 | 19 | macOS 5 条、Windows 7 条、Linux 7 条 |

本机 macOS 自动验证通过：

- `cargo fmt --check`
- `source vendor/librime/macos-universal/env.sh && cargo check -p keytao-app`
- `source vendor/librime/macos-universal/env.sh && cargo check -p keytao-app --release`
- `source vendor/librime/macos-universal/env.sh && DYLD_FALLBACK_LIBRARY_PATH="$RIME_LIB_DIR:${DYLD_FALLBACK_LIBRARY_PATH:-}" cargo test -p keytao-core`
- `source vendor/librime/macos-universal/env.sh && DYLD_FALLBACK_LIBRARY_PATH="$RIME_LIB_DIR:${DYLD_FALLBACK_LIBRARY_PATH:-}" cargo test -p keytao-app`
- `pnpm build`
- `scripts/build-macos.sh`
- `scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg`
- `bash -n scripts/build-macos.sh scripts/verify-macos-pkg.sh`
- `target/release/build/keytao-app-*/output` 已包含 `cargo:rustc-env=RIME_VERSION=1.17.0`
- `target/release/bundle/macos/KeyTao.app/Contents/MacOS/keytao-app` 已包含 `1.17.0` / `librime_version` 相关字符串
- `crates/keytao-macos-ime/Resources/Info.plist` 已确认不再包含 `TICapsLockLanguageSwitchCapable`
- `cargo check -p keytao-windows-ime`
- `cargo test -p keytao-linux-ime`

说明：`cargo check -p keytao-windows-ime` 和 `cargo test -p keytao-linux-ime` 是在本机 macOS host target 下完成的可编译性检查，不能替代 Windows TSF 与 Linux IBus/Wayland/XIM 的真实系统验收。

macOS pkg 校验结果：

- 已生成 `target/keytao-macos-pkg/KeyTao.pkg`。
- `scripts/verify-macos-pkg.sh` 已确认主 App 与 IME bundle 都包含 `librime`、`rime-plugins/librime-lua.dylib` 和 `rime-data/default.yaml`。
- 主 App、IME app、Core FFI 都是 `arm64`；随包 `librime` 同时包含 `x86_64 arm64`。
- 主 App 与 IME app 的 codesign 校验均通过。
- 当前 macOS 环境仍会把受保护的 `com.apple.provenance` 扩展属性写进 pkg payload，并表现为 AppleDouble metadata warning；这不影响本次 verify 脚本通过，但后续可单独决定是否调整打包环境或校验策略。

本轮已经按报告优先级修复以下问题：

- 新增 Mac 主 App 的 `librime` 版本探测：构建期优先从 `RIME_VERSION` 或 vendored `RIME_LIB_DIR/pkgconfig/rime.pc` 写入版本，再用系统 `pkg-config` 兜底；运行期再从环境变量、随包 `librime*.dylib`、pkg-config 和 Nix store 兜底，避免发布包中显示 `unknown`。
- P1-1：Windows 部署成功后也写 `keytao-ime.reload`，让 TSF 监听到 App 重新部署。
- P1-2：macOS `commitComposition(_:)` 改为和普通 Return 路径共用 `XK_Return`/`0xff0d`。
- P1-3：Tauri capability 增加 macOS `~/Library/keytao` 和 Windows `%APPDATA%/keytao` 打开权限。
- P1-4：Linux IBus shim 空格选词改为必须存在 candidates，并补候选索引单元测试。
- P1-5：Windows TSF 对象计数绑定 `TextService` COM 对象生命周期，而不是绑定 `Activate`/`Deactivate`。
- P2-6：Linux 单实例逻辑改为已有 owner 时退出，不再主动杀旧 daemon 或继续双开。
- P2-7：macOS install/uninstall Tauri 命令在 release build 中拒绝执行，仅保留 debug 开发入口。
- P2-8：删除旧 `src-tauri/src/ime` Linux 内嵌 IME 路径，并同步平台文档。
- P2-9：Windows 注册 DLL 路径 buffer 从 260 扩到 32768，并检测截断/失败。
- P2-10：IBus shim 的 session bus fallback 改为优先 `XDG_RUNTIME_DIR`，最后使用当前 uid，不再硬编码 uid 1000。
- P2-11：Windows IME runtime 打包路径改为稳定的 `keytao-windows-ime-runtime/x64`，旧 `_up_/target/...` 仅保留为兼容查找路径。
- P2-12：macOS cursor rect 改回 Apple 官方坐标契约：`firstRectForCharacterRange` / `lineHeightRectangle` 返回 screen/global rect，KeyTao 不再用前台窗口 frame 做二次转换；仅过滤无效 rect 和屏幕角落哨兵值。
- P2-13：移除 macOS `TICapsLockLanguageSwitchCapable` 声明，避免声明当前未实现的 CapsLock 切换能力。
- P3-14：新增 `keytao-core::key_policy`，收敛 Enter、空 composition bypass、候选选择、Ctrl+grave 转发等共享规则；Linux Wayland/KDE/XIM/GNOME IBus/IBus shim 与 Windows key map 已改为复用该规则。
- P3-15：在 `keytao-core` 增加 key policy golden tests，覆盖空 composition bypass、有 preedit 无 candidates 时 Space 不选词、候选高亮 clamp、`select_keys` 映射和 Ctrl+grave 转发。
- P3-16：Linux/Windows/macOS status 增加用户目录、共享数据目录、共享数据来源、reload stamp 路径与签名等诊断字段；macOS debug logs 额外返回 `~/Library/keytao/log` 下的 librime 日志。
- P3-17：status 中明确 `shared_data_source`，能区分包内 runtime 和 `default_fallback`，为排查误用 Squirrel/Homebrew/系统 Rime 数据提供依据。

仍建议后续继续推进：

- macOS 自动构建、包内容、版本注入和 CapsLock 声明已在本机验证；About 页面实际显示、IMK 输入行为、候选窗定位、reload 和日志诊断仍需要人工在真实输入场景里检查。
- Linux 与 Windows 的真实系统输入链路必须在对应系统中验证；本机 macOS 无法完成 IBus/Wayland/XIM 和 Windows TSF 的运行时验收。

## 审查范围

本次审查覆盖当前仓库内和系统输入法相关的实现：

- 通用 runtime 与 FFI：`crates/keytao-core`、`crates/keytao-core-ffi`
- App 部署与状态对接：`src-tauri/src/lib.rs`、`src/App.tsx`、`src-tauri/capabilities/default.json`
- Linux 系统输入法 daemon：`crates/keytao-linux-ime`
- macOS IMKit 输入法：`crates/keytao-macos-ime`
- Windows TSF TIP：`crates/keytao-windows-ime`
- Android `InputMethodService` 系统输入法：`src-tauri/gen/android/app`

## 总体结论

当前大方向是对的：`keytao-core` 已经把 librime 的部署、session、reload generation 和 `ImeState` 抽取收敛到一处；Linux/macOS/Windows/Android 都在朝“平台 adapter 只处理系统协议和 UI”的方向走。平台实现文档也基本贴近代码，尤其是 Linux/macOS/Windows/Android 的 `IMPL.md` 已经把实现边界写清楚。

主要问题不在 core 架构，而在平台对接的细节还没有完全闭环：

1. Windows TSF 已经实现 reload stamp 监听，但 App 部署后没有写 Windows stamp，导致重新部署后 Windows 输入法不会自动刷新。
2. macOS `commitComposition` 使用 Carbon 虚拟键码 `kVK_Return`，而不是 librime 统一 keysym `0xff0d`。
3. App 的文件打开权限还停在旧 Rime 目录，和当前默认用户目录 `~/Library/keytao` / `%APPDATA%/keytao` 不一致。
4. Linux IBus shim 的空格选词逻辑和其它后端不一致，在“有 preedit 但无 candidates”的状态下仍会尝试选第 0 个候选。
5. Windows TSF 的 COM/DLL 生命周期计数不是跟对象生命周期绑定，`DllCanUnloadNow` 可能给出不安全的结果。
6. Linux/macOS/Windows 之间仍有大量重复 key handling 规则，未来容易继续出现“某个平台修了，另一个平台没修”的漂移。

未发现会直接破坏用户词库或跨平台 core 数据结构的 P0 问题；但上面的 P1 问题会造成真实输入行为错误或部署后状态不刷新，应优先修。

## 做得好的部分

- `ImeRuntime` + `ImeRuntimeSession` 的 generation 懒刷新设计合理，能避免平台层各自重建 librime session。
- `ImeState` 的字段和应用顺序已经形成统一契约，顶功场景所需的 “commit + new preedit” 顺序在文档中也明确写出。
- macOS 打包脚本已经把 `rime-plugins/librime-lua.dylib` 作为 runtime 一部分处理，解决了“配置存在但 Lua 组件创建失败”的关键路径。
- Linux daemon 对 GNOME、KDE、wlroots Wayland、XIM、IBus shim 的分流相对清楚，没有把 KDE KWin 私有 `WAYLAND_SOCKET` 和普通 daemon 混在一起。
- Windows TSF 已经走 `ImeRuntimeSession`，没有再直接持有低层 `Engine`。
- Android 侧 `InputMethodService`、SAF 安装和 Rime 文件合并已有实现与测试覆盖，并已作为正式 Android 系统输入法入口维护。

## P1：应优先修复的问题

### 1. Windows 部署后不会触发 TSF reload

证据：

- App 的 `rime_deploy_default` 只在 Linux/macOS 写 reload stamp：`src-tauri/src/lib.rs:2310`
- Windows TSF 初始化和监听的是 `%APPDATA%/keytao/keytao-ime.reload`：`crates/keytao-windows-ime/src/state.rs:58`
- 文档已把 Windows reload 作为当前能力描述：`crates/keytao-windows-ime/IMPL.md`

影响：

- 用户在 Windows App 内安装/部署新方案后，TSF 进程不会收到 stamp 变化。
- 现有 TSF session 可能继续使用旧词库，直到重启输入法进程、重新注册、注销登录或手动写 stamp。
- 这会让“部署成功”的 UI 反馈和真实输入行为不一致。

建议：

- 把 `write_keytao_ime_reload_stamp()` 的 cfg 扩展到 Windows。
- 部署成功后 Linux/macOS/Windows 都写同一个默认用户目录下的 `keytao-ime.reload`。
- Windows 端增加一个轻量集成检查：部署后 status/debug 至少能显示 stamp 路径和当前签名。

### 2. macOS `commitComposition` 使用了错误 keycode

证据：

- 普通 keyDown 的 Return 被转换为 `0xff0d`。
- `commitComposition(_:)` 直接传 `UInt32(kVK_Return)`：`crates/keytao-macos-ime/Sources/KeyTaoIME/InputController.swift:115`
- `kVK_Return` 是 Carbon 虚拟键码，不是 librime 统一使用的 X11 keysym。

影响：

- 通过 IMKit 主动提交 composition、鼠标点击 marked text 触发提交等路径，可能向 Rime 发送错误按键。
- 表现可能是无法提交、错误字符进入编码、候选状态异常，且普通按回车路径看起来正常，排查会很绕。

建议：

- 将该处改成 `0xff0d` 或复用 `rimeKeyValue` 中的 Return 常量。
- 增加一个 Swift 侧小测试或至少在 `IMPL.md` 的修复记录中标注已消除该兼容点。

### 3. `openPath` 权限仍指向旧目录

证据：

- 当前默认用户目录是 macOS `~/Library/keytao`、Windows `%APPDATA%/keytao`：`crates/keytao-core/src/lib.rs:839`
- capability 仅允许 Linux KeyTao 目录，以及 macOS/Windows 的旧 Rime 目录：`src-tauri/capabilities/default.json:15`
- macOS `~/Library/Rime` 和 Windows `$APPDATA/Rime` 仍在白名单中：`src-tauri/capabilities/default.json:33`

影响：

- App 里的“打开目录”按钮在 macOS/Windows 默认目录下可能失败。
- UI 展示的是 KeyTao 自己的数据目录，但权限模型还在暗示用户去旧 Rime 目录。

建议：

- 增加 `$HOME/Library/keytao`、`$HOME/Library/keytao/**`。
- 增加 `$APPDATA/keytao`、`$APPDATA/keytao/**`。
- 旧 Rime 目录是否保留应作为“扩展/迁移目录”单独说明，不应和默认目录混在一起。

### 4. Linux IBus shim 空格选词缺少 candidates 守卫

证据：

- `ibus_backend.rs` 中只要 key 是空格，就构造 `Some(highlighted.min(len.saturating_sub(1)))`：`crates/keytao-linux-ime/src/ibus_backend.rs:630`
- 当 `preedit` 非空但 `candidates` 为空时，`should_bypass_empty_composition` 不会放行，后续会尝试 `select_candidate_at(0)`。
- GNOME IBus engine、Wayland、KDE、XIM 路径都显式要求 candidates 非空才用空格选候选。

影响：

- 少数 Rime 状态下，空格可能不是提交/处理当前 preedit，而是被错误翻译成“选第 0 个候选”动作。
- 这是 Linux 后端之间的行为漂移，会造成“同样是 IBus，在 GNOME 和 shim 下行为不一致”。

建议：

- 将空格选词条件改为 `is_candidate_select_key(keyval) && !before_state.candidates.is_empty()`。
- 给 `candidate_select_index` 加单元测试，覆盖 `preedit != "" && candidates.is_empty()`。

### 5. Windows TSF COM/DLL 生命周期计数不准确

证据：

- `DLL_OBJ_COUNT` 注释说是 active COM object count：`crates/keytao-windows-ime/src/globals.rs:9`
- 实际 `obj_add()` 在 `TextService::Activate` 里调用：`crates/keytao-windows-ime/src/text_service.rs:95`
- `ClassFactory::CreateInstance` 创建 `TextService` 时没有增加对象计数。

影响：

- COM object 已创建但还没 Activate 时，`DllCanUnloadNow()` 可能返回可卸载。
- 如果 Activate/Deactivate 序列异常或多次进入，计数也不一定代表真实对象生命周期。
- TSF TIP 是 in-proc DLL，这类生命周期问题很难稳定复现，但一旦出现就是随机崩溃或输入法被系统卸载。

建议：

- 将对象计数绑定到 COM 对象构造/Drop，而不是 `Activate`/`Deactivate`。
- `Activate` 只管理 thread manager sink、session 和 runtime 状态。
- 给 `ClassFactory`、`TextService`、`KeyEventSink` 的生命周期补最小日志。

## P2：中期修复与设计债

### 6. Linux 单实例逻辑会主动杀掉已有 owner

证据：

- 申请 D-Bus name 失败后，代码会查 owner pid 并发送 `SIGTERM`：`crates/keytao-linux-ime/src/main.rs:514`
- 如果重试仍失败，会 “Continuing anyway”：`crates/keytao-linux-ime/src/main.rs:541`

影响：

- 可能杀掉正在工作的旧 daemon，造成输入状态丢失。
- 如果 name owner 不是预期实例，行为更危险。
- 继续运行会制造多 daemon 竞争，而不是明确失败。

建议：

- 默认行为改成“已有 owner 则退出”。
- App 的 restart 命令才显式终止旧进程。
- 终止前校验 `/proc/<pid>/exe` 或 command line 必须是当前预期 `keytao-ime`。

### 7. macOS install/uninstall 命令仍暴露在 Tauri invoke handler

证据：

- `macos_install_ime` 仍会运行仓库 install script：`src-tauri/src/lib.rs:2077`
- `macos_uninstall_ime` 仍会执行 `sudo rm -rf /Library/Input Methods/KeyTao.app`：`src-tauri/src/lib.rs:2127`
- React UI 当前没有按钮，但命令仍被注册到 invoke handler。

影响：

- 和“输入法随 App 包安装，不在正式 UI 暴露安装/卸载”的产品方向不完全一致。
- 如果未来有 WebView 注入或误调用，仍存在系统组件操作入口。

建议：

- release build 中移除这些命令，或用 debug/dev feature gate 包起来。
- 保留 CLI/script 作为开发入口即可。

### 8. Linux 旧 Tauri 内嵌 IME 路径仍在仓库中

证据：

- `src-tauri/src/ime/mod.rs` 仍 re-export 旧 `linux::{spawn, TrayArc, TrayShared}`。
- 当前 `src-tauri/src/lib.rs` 没有调用该 `spawn`，但仍保留 Linux overlay/global shortcut 和 `rime::RimeEngine` 初始化：`src-tauri/src/lib.rs:2566`
- 当前文档也已经说明不要继续在旧文件里扩展 Linux 系统输入法能力。

影响：

- 新维护者容易修错路径。
- 旧 overlay/global shortcut 与系统 daemon 的边界不够清晰。
- 测试输入框、调试 overlay、系统 IME 三者混在 App 进程里，会增加行为解释成本。

建议：

- 删除旧 `src-tauri/src/ime/linux.rs`，或改名到明确的 `legacy/experimental` 并默认不编译。
- 如果 `rime::RimeEngine` 只是测试输入框用途，命名上与系统 IME 分开。

### 9. Windows 注册 DLL 路径缓冲区仍是 260

证据：

- `registration.rs::dll_path()` 使用 `vec![0u16; 260]`：`crates/keytao-windows-ime/src/registration.rs:107`
- `state.rs::dll_related_dirs()` 已经使用 32768 长度缓冲区：`crates/keytao-windows-ime/src/state.rs:163`

影响：

- 安装路径较长时，注册表可能写入截断 DLL 路径。
- 后续 TSF status 可能查不到注册路径，或者 Windows 无法加载 TIP。

建议：

- 统一用 32768 buffer，或循环扩容直到 `GetModuleFileNameW` 没有截断。

### 10. IBus shim 地址文件写入仍有桌面环境假设

证据：

- 生成固定 `wayland-0`、`wayland-1` 文件名：`crates/keytao-linux-ime/src/ibus_backend.rs:837`
- `DBUS_SESSION_BUS_ADDRESS` 缺失时 fallback 到 `unix:path=/run/user/1000/bus`：`crates/keytao-linux-ime/src/ibus_backend.rs:987`

影响：

- 非 uid 1000 用户、远程会话、容器会话、多 Wayland display 下容易写错地址。
- 旧地址文件可能误导 GTK/Chromium 客户端连接不存在的 daemon。

建议：

- fallback 应基于 `XDG_RUNTIME_DIR` 或当前 uid，而不是硬编码 1000。
- 写入前清理 KeyTao 生成的过期 bus 文件，或记录创建时间/进程有效性。

### 11. Windows 打包路径带着开发目录形状

证据：

- Tauri Windows 资源把 IME runtime 打到 `_up_/target/keytao-windows-ime-runtime/x64`：`src-tauri/tauri.windows.conf.json:7`
- runtime 查找也要兼容多种 `_up_/target` 路径：`src-tauri/src/lib.rs:552`

影响：

- 发布包结构不直观，status/debug 展示的 runtime path 不像产品路径。
- 后续 NSIS 自定义安装、升级、清理会更难。

建议：

- 将 runtime 放到稳定资源目录，例如 `keytao-windows-ime-runtime/x64`。
- 只保留一到两个兼容旧包的 fallback，后续版本再删。

### 12. macOS cursor rect 计算偏启发式

证据：

- `cursorRect(for:)` 优先请求 character index 0 的 line rect，再请求 range 0 的 first rect。
- 如果坐标不在屏幕内，再用 frontmost window frame 做 bottom-left/top-left 转换。

影响：

- 多窗口、多屏、浏览器 WebView、Electron、远程桌面等场景下，候选窗可能定位到窗口左上角、鼠标处或上一焦点附近。

建议：

- 优先记录/查询当前 marked range 或 selected range 的 rect。
- 对 fallback 到鼠标位置的情况打 debug 日志，并在 App 诊断页显示最近一次 cursor rect 来源。

### 13. CapsLock 声明与实现不一致

证据：

- macOS `Info.plist` 声明 `TICapsLockLanguageSwitchCapable`。
- 当前 `InputController` 只处理 Shift 的 `flagsChanged`，没有 CapsLock 专门逻辑。

影响：

- 系统 UI 可能认为输入源支持 CapsLock 切换，但实际行为依赖系统或无效。

建议：

- 如果暂不支持，移除该声明。
- 如果要支持，应明确 CapsLock 与 Rime `ascii_mode` 的同步规则，并避免和 solo Shift 切换冲突。

## P3：长期改进方向

### 14. 收敛重复 key handling 规则

当前这些逻辑在多个平台重复：

- `should_bypass_empty_composition`
- Enter 提交 preedit
- 空格选择高亮候选
- `select_keys` 到候选 index 的映射
- solo Shift release
- Ctrl/Alt 快捷键放行

建议在 Rust 侧增加一个小的共享 adapter 模块，至少提供：

- `KeyActionPolicy`
- `candidate_index_for_select_key`
- `should_bypass_empty_composition`
- `should_forward_consumed_shortcut`
- 特殊 keysym 常量

Swift 侧可以先不直接依赖 Rust policy，但应把常量和行为写成同名结构，配套 golden case。

### 15. 增加跨平台 golden tests

建议建立一组平台无关测试用例：

- 空 composition + Space/Backspace/Return/Arrow 应放行。
- 有 preedit + Return 应提交 preedit。
- 有 candidates + Space 应选择 highlighted candidate。
- 有 preedit 无 candidates + Space 不应强行 select candidate。
- solo Shift release 应切换中英；Shift+a 不应触发 solo Shift。
- deploy 后 reload generation 应让旧 session 下次访问重建 engine。

这些测试可以先落在 `keytao-core` 或 Linux adapter，再逐步映射到 macOS/Windows。

### 16. 诊断能力需要按平台补齐

当前 App 可读 Linux `/tmp/keytao-ime.log`，但 macOS/Windows 的系统输入法日志没有同等入口。

建议：

- macOS：收集 `~/Library/keytao/log/` librime 日志和最近的 `NSLog` 指引。
- Windows：status 返回 user dir、shared dir、stamp signature、runtime dir、registered path、TSF active 状态。
- Linux：status 区分 daemon owner pid、fallback daemon、KWin private instance，并展示是否成功获得 D-Bus name。

### 17. 共享数据目录 fallback 要显式可观测

macOS/Windows/Linux 都有多级 shared data fallback。fallback 本身合理，但用户遇到 Lua 或 schema 问题时，需要知道到底用了包内 runtime、系统 Rime、Squirrel、Homebrew 还是空目录。

建议：

- status/debug 明确展示 active `shared_data_dir`。
- 如果 fallback 到 Squirrel/Homebrew/系统 Rime，给出 warning，不把它当作正常发行路径。

## 建议修复顺序

### 第一批：直接修真实 bug

1. Windows deploy 后写 reload stamp。
2. macOS `commitComposition` 改用 `0xff0d`。
3. 更新 capability 的 KeyTao 数据目录白名单。
4. Linux IBus shim 空格选词加 candidates 守卫。
5. Windows `dll_path()` 改用长路径 buffer。

### 第二批：减少误操作和平台漂移

1. release build 移除或 gate macOS install/uninstall invoke 命令。
2. Linux 单实例逻辑改为默认退出，不主动杀 owner。
3. 清理旧 Tauri Linux IME 模块和 overlay 命名边界。
4. Windows COM object lifetime 计数绑定对象生命周期。

### 第三批：工程化收敛

1. 抽共享 key policy/adaptor helper。
2. 建跨平台 golden tests。
3. 统一诊断 status schema。
4. 整理 Windows runtime 包路径。

## 最小验收清单

- macOS：普通回车、IMK `commitComposition`、鼠标点击 marked text 三条提交路径都使用 `XK_Return` 语义。
- Windows：App 部署后 `%APPDATA%/keytao/keytao-ime.reload` mtime/size 变化，TSF 下次 focus/key event 触发 reload。
- Linux：IBus shim 在 `preedit != "" && candidates.is_empty()` 时空格不调用 `select_candidate_at(0)`。
- App：三个桌面平台的“打开目录”都能打开当前 `default_user_data_dir()`。
- 打包：Windows status 中 runtime path 不再优先显示 `_up_/target/...`。
- 文档：`IMPL.md` 中已列出的“后续修正点”在修完后同步改为“已修复行为”。

## 仍需到对应系统验证的内容

本轮在 macOS 开发机上完成了可自动执行的构建、测试和包内容校验。当前待人工或目标系统验证共 19 条：macOS 5 条、Windows 7 条、Linux 7 条。下面是需要分别进入对应系统验证的项目。

### macOS 本机已自动验证通过

- 版本注入：release build 的 `target/release/build/keytao-app-*/output` 已包含 `cargo:rustc-env=RIME_VERSION=1.17.0`；release 主 App 二进制已包含 `1.17.0` 与 `librime_version` 相关字符串。
- 主 App release 包：`scripts/build-macos.sh` 已成功生成 `target/keytao-macos-pkg/KeyTao.pkg`。
- 包内容校验：`scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg` 已通过，确认主 App 与 IME bundle 都包含 `librime*.dylib`、`rime-plugins/librime-lua.dylib` 和 `rime-data/default.yaml`。
- 架构和签名：校验结果显示主 App、IME app、Core FFI 均为 `arm64`，随包 `librime` 为 `x86_64 arm64`，主 App 与 IME app 的 codesign 校验通过。
- CapsLock 声明：`crates/keytao-macos-ime/Resources/Info.plist` 已确认不再包含 `TICapsLockLanguageSwitchCapable`。

说明：当前 macOS 环境仍会把受保护的 `com.apple.provenance` 扩展属性写进 pkg payload，并表现为 AppleDouble metadata warning；这不影响本次 verify 脚本通过，但后续可单独决定是否调整打包环境或校验策略。

### 需要在 macOS 中人工验证

- About/版本信息：打开主 App 的版本页，确认 `librime 版本` 不再是 `unknown`，应显示构建时 vendored librime 版本或随包 dylib 解析出的版本。
- IMK 提交路径：在 TextEdit、Safari、Chrome/Electron 输入框中分别验证普通 Return、系统触发 `commitComposition`、鼠标点击 marked text 后提交，三条路径都能正常提交当前 preedit。
- 候选窗定位：在多屏、浏览器 WebView、Electron、远程桌面或缩放显示环境下验证 marked range / selected range 定位是否跟随真实输入位置，不再贴到应用左下角；日志中应能看到 cursor rect 来源。
- Reload：主 App 部署方案后确认 `~/Library/keytao/keytao-ime.reload` 签名变化，IME 下一次 activation/key handling 后加载新 schema/Lua。
- 日志诊断：调用 `read_debug_logs` 或诊断入口时，确认 macOS 下能读到 `~/Library/keytao/log` 的 librime 日志；`macos_ime_status` 应返回 `user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path` 和 `log_dir`。

### 需要到 Windows 中验证

- Windows/MSVC 构建：在 Windows 开发环境中准备 `vendor\\librime\\windows-x64` 后执行 `cargo build -p keytao-windows-ime --target x86_64-pc-windows-msvc --release`，确认 TSF DLL 能完整链接。
- App 打包路径：执行 Windows 打包，确认资源目录中 IME runtime 位于 `keytao-windows-ime-runtime/x64`，status 中 `runtime_dir` 不再优先显示 `_up_/target/...`。
- 注册长路径：把 App 安装到较长路径下，执行注册，确认 `DllRegisterServer` 写入完整 DLL 路径，没有 260 字符截断。
- COM 生命周期：注册并切换 KeyTao TSF，反复 focus/blur、切换输入法和关闭应用，确认没有随机卸载、崩溃或 `DllCanUnloadNow` 过早卸载迹象。
- Deploy reload：在主 App 中部署方案，确认 `%APPDATA%\\keytao\\keytao-ime.reload` 签名变化；TSF 下一次 key/focus 后触发 reload，旧 preedit/candidates 被清空，新 schema 生效。
- Status 诊断：调用 `windows_ime_status`，确认返回 `user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`、`registered_path` 和 `runtime_dir`。
- 输入行为：验证 Return 提交 preedit、Space 选择高亮候选、数字/`select_keys` 选词、solo Shift release 中英切换、Shift+字母不触发 solo Shift。

### 需要到 Linux 中验证

- Linux 构建：在带 `dbus-1.pc` 的 Linux 构建环境中执行 `cargo test -p keytao-linux-ime --target x86_64-unknown-linux-gnu --no-run` 或正式 Linux build；本机 macOS 因缺少 `dbus-1.pc` 无法完成该 target 编译。
- IBus shim：在非 GNOME 的 IBus shim 路径验证 `preedit != "" && candidates.is_empty()` 时 Space 不调用 `select_candidate_at(0)`，并继续交给 librime/前端按预期处理。
- GNOME IBus engine：在 GNOME Wayland 下验证 Space/数字选词、Return 提交、空 composition 快捷键放行、Shift release 模式切换。
- KDE/KWin：验证 KWin `WAYLAND_SOCKET` 私有 input-method-v1 实例不参与普通 daemon 单实例抢占；普通 daemon 已存在时新 daemon 应退出，不主动 kill owner。
- wlroots Wayland 与 XIM：验证共享 `key_policy` 后，Return、Space 高亮选词、Ctrl+grave 转发、空 composition bypass 与原行为一致。
- DBus 地址：在非 uid 1000 用户、远程会话或自定义 `XDG_RUNTIME_DIR` 中验证 IBus 地址文件写入的是当前 session bus，不再硬编码 `/run/user/1000/bus`。
- Status 诊断：调用 `linux_ime_status`，确认返回 `daemon_owner_pid`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path` 和 `reload_stamp_signature`；KDE native/fallback process 计数应符合当前会话。
