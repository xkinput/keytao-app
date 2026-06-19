# 输入法通用层实现规范

本文定义 KeyTao 系统输入法前端的通用层边界。后续新增或重构平台输入法时，应先对齐本文，再进入各平台 `IMPL.md`。

目标是让平台实现只处理平台差异，把稳定行为收敛到同一套 runtime、core、state、key event、UI model 和 reload 规则里。

## 当前结论

现在直接操作 librime 的代码在 `crates/keytao-core/src/lib.rs`：

- `deploy()` 负责 `setup()`、`initialize()`、`full_deploy_and_wait()`。
- `Engine::process_key_result()` 负责调用 `session.process_key(KeyEvent::new(...))`。
- `extract_state()` 负责把 librime context/menu/status 转成统一 `ImeState`。

平台层不应该直接操作 librime。Linux 之前有一份自己的 `CoreEngine/ImeSession` 调度，现在已经收敛为 `keytao-core::ImeRuntime` / `ImeRuntimeSession`；Linux 只在 `crates/keytao-linux-ime/src/engine.rs` re-export。macOS 通过 `keytao-core-ffi` 创建 per-session 时，也走同一套 `ImeRuntimeSession`。Windows TSF 当前也已改为持有 `ImeRuntimeSession`，不再直接持有 `Engine`。

还没有完全收敛的是系统协议、按键转换、UI 应用和部分 reload 触发时机。这些可以继续留在平台层，但 reload generation、session refresh、modifier mask、candidate/session API 应逐步都放在通用层。

## 架构优点

这套架构的核心价值是把“输入法业务状态”和“平台系统协议”拆开：

1. librime 只由 `keytao-core` 操作，避免 Linux、macOS、Windows 各自再实现 deploy、session、candidate、reset、ascii mode 等细节。
2. reload 入口统一到 `ImeRuntime::reload()`，部署后只递增 generation，已有 session 在下一次访问时懒刷新，减少“词库已部署但 IME 还在旧状态”的问题。
3. 平台层变薄：新增平台只要实现 key event 归一化、commit/preedit/candidate adapter 和 UI renderer，不需要理解 librime context/menu/status。
4. `ImeState` 成为唯一 UI/提交输入，顶功、commit + new preedit、候选 label、分页和中英状态可以按同一顺序处理。
5. `theme.yaml` 已落在共享主题语义和 UI model 上，平台 renderer 只负责把 model 映射到 AppKit、Wayland SHM、X11 overlay、TSF candidate window 或系统 lookup table。
6. 测试面更稳定：key map、modifier mask、reload generation、candidate actions 可以在通用层或薄 adapter 层分别测试，不需要每个平台重复造一套业务状态。

这也意味着平台层不要把“为了某个平台方便”而新增的字段直接塞进 `ImeState` 或 `theme.yaml`。平台差异应落在 adapter 能力声明和 renderer fallback 里。

## librime 通信模型

librime 是进程内 native library，不是独立服务。通用层和 librime 的通信发生在 `keytao-core` 内部：

```text
platform key event
  -> X11 keysym + Rime modifier mask
  -> ImeRuntimeSession::process_key_result()
  -> Engine::process_key_result()
  -> rime_api::Session::process_key(KeyEvent)
  -> KeyStatus + Rime context/menu/status
  -> extract_state()
  -> KeyProcessResult { accepted, state: ImeState }
  -> platform commit/preedit/candidate adapter
```

部署通信：

```text
App installs schema/dict/lua/opencc files
  -> keytao_core::deploy(user_data_dir, shared_data_dir)
  -> rime_api setup/initialize on first call
  -> full_deploy_and_wait()
  -> App writes keytao-ime.reload
  -> running IME calls ImeRuntime::reload()
  -> generation += 1
  -> ImeRuntimeSession refreshes internal Engine lazily
```

按键通信里只有两种输入从平台进入通用层：

- `keycode`：统一使用 X11 keysym，例如 Return 是 `0xff0d`，Escape 是 `0xff1b`。
- `mask`：统一使用 Rime modifier mask，只保留 Shift、Control、Alt 和 Release，CapsLock/NumLock/鼠标状态等噪声在通用层过滤。

通用层只向平台返回两类输出：

- `accepted`：librime 是否接受该按键，平台据此决定是否吞掉原生事件。
- `ImeState`：提交文本、preedit、cursor、候选、highlight、page、select keys、ascii mode。

候选选择、翻页、reset 和 ascii mode 也走 session API。平台 UI 不直接读写 librime 状态，只把用户动作转成 `select_candidate()`、`change_page()`、`reset()` 或 `set_ascii_mode()`。

## 代码分层

当前系统输入法由四层组成：

| 层级 | 当前位置 | 职责 | 不应承担 |
| --- | --- | --- | --- |
| IME runtime + Rime wrapper | `crates/keytao-core` | librime setup/deploy/session、`ImeRuntime`、reload generation、modifier mask、`ImeState` 抽取、用户目录/共享目录、配置合并工具 | 平台协议、窗口绘制、系统安装 |
| C FFI | `crates/keytao-core-ffi` | 给 Swift/C/其它语言提供 per-session C ABI，并复用 `ImeRuntimeSession` | 平台策略、UI 样式、按键猜测 |
| Platform frontend | `crates/keytao-linux-ime`、`crates/keytao-macos-ime`、`crates/keytao-windows-ime` | 系统输入法协议、按键转换、提交文本、更新 preedit、候选窗/候选服务、日志诊断 | Rime 业务状态、配置合并、跨平台视觉语义 |
| App integration | `src-tauri/src/lib.rs` 和 React UI | 下载安装方案、触发部署、状态展示、写 reload stamp；Linux 可在启动时拉起 fallback daemon | 直接接管系统按键热路径、在正式 UI 暴露系统输入法安装/卸载/重启按钮 |

平台前端之间可以使用完全不同的系统协议，但必须共享同一套输入输出语义：

```text
native key event
  -> X11 keysym + Rime modifier mask
  -> keytao-core ImeRuntimeSession
  -> ImeState + accepted
  -> platform commit/preedit/candidate adapter
```

## Runtime/Core 契约

`keytao-core` 是平台无关核心。所有桌面平台都应通过它进入 librime，并通过 `ImeRuntime` 管理 IME session 调度。

### `deploy(user_data_dir, shared_data_dir)`

- 必须在创建 session 前成功执行。
- 进程内 `setup()` + `initialize()` 只在第一次 deploy 时执行；后续 deploy 只重新 `full_deploy_and_wait()`。
- `user_data_dir` 是 KeyTao 自有用户目录，不默认复用其它输入法的用户目录。
- `shared_data_dir` 必须包含基础 Rime 数据，至少要有 `default.yaml`。
- 调用方应把 deploy 放在后台线程或平台允许阻塞的位置。

### `Engine`

- 一个 `Engine` 对应一个 librime session。
- 每个输入上下文必须独立创建 session，不能多个客户端共享一个 session。
- `Engine` 的公开操作应被视为唯一的 session 状态入口：
  - `process_key_result(keycode, mask)`
  - `state()`
  - `select_candidate(index)`
  - `change_page(backward)`
  - `reset()`
  - `is_ascii_mode()`
  - `set_ascii_mode(enabled)`
  - `current_schema_name()`

平台前端不应直接访问 librime context/menu/status，也不应绕过 core 自己解析候选。

### `ImeRuntime`

`ImeRuntime` 是输入法通用运行时，负责把“什么时候部署、什么时候重载、session 什么时候刷新”从平台层收回来。

当前职责：

- `ImeRuntime::new()`：使用平台默认用户目录和共享数据目录。
- `ImeRuntime::with_dirs(user_data_dir, shared_data_dir)`：用于 macOS/Windows/测试等明确指定目录的场景。
- `init()`：首次部署并初始化 librime。
- `reload()`：重新部署词库并递增 generation。
- `create_session()`：为输入上下文创建 `ImeRuntimeSession`。

`ImeRuntimeSession` 当前职责：

- 在 `state()`、`process_key_result()`、`select_candidate()`、`change_page()`、`reset()`、`set_ascii_mode()` 前检查 generation。
- 发现 generation 变化时自动重建内部 `Engine`，让新词库实时生效。
- 在 `process_key_result()` 中统一过滤 modifier mask。

平台层只需要持有 runtime/session，不需要自己维护 generation、reload 后重建 session、modifier mask 过滤。

## FFI 契约

非 Rust 平台前端应优先使用 `keytao-core-ffi` 的 per-session API。FFI per-session 已经复用 `ImeRuntimeSession`，所以 Swift/C 层不需要直接管理 librime session。

当前 C ABI：

- `keytao_init(user_dir, shared_dir) -> bool`
- `keytao_is_initialized() -> bool`
- `keytao_reload() -> bool`
- `keytao_create_session() -> session`
- `keytao_destroy_session(session)`
- `keytao_session_state(session) -> KeytaoState*`
- `keytao_session_process_key(session, keyval, modifiers) -> KeytaoState*`
- `keytao_session_select_candidate(session, index) -> KeytaoState*`
- `keytao_session_change_page(session, backward) -> KeytaoState*`
- `keytao_session_reset(session) -> KeytaoState*`
- `keytao_session_get_ascii_mode(session) -> bool`
- `keytao_session_set_ascii_mode(session, enabled) -> KeytaoState*`
- `keytao_free_state(state)`

FFI 调用规则：

1. 每个返回的 `KeytaoState*` 必须调用 `keytao_free_state()`。
2. `KeytaoState` 内所有字符串都是 UTF-8 C string。
3. `committed` 和 `select_keys` 在 C ABI 中用空字符串表示无值。
4. FFI 里保留了旧的 module-level singleton API，例如 `keytao_process_key()`；这些旧入口内部也复用一个 `ImeRuntimeSession`。新平台前端不要使用这些旧入口，除非是单上下文工具。
5. 平台前端持有 session handle 时，应在输入上下文销毁、失焦彻底结束或进程退出时销毁 session。

## 统一状态模型

`ImeState` 是所有平台前端的唯一 UI/提交输入：

| 字段 | 含义 | 平台应用规则 |
| --- | --- | --- |
| `committed` | 本次按键产生的提交文本 | 非空时先清旧 preedit，再通过平台原生提交接口写入客户端 |
| `preedit` | 当前预编辑文本 | 用平台 composition/marked text/preedit API 更新；为空时清除当前 preedit |
| `cursor` | preedit 光标位置 | 映射到平台 preedit cursor/selection |
| `candidates` | 当前页候选 | 显示候选文本和 comment；空则隐藏候选窗或 lookup table |
| `highlighted_candidate_index` | 当前高亮候选 | 映射为高亮、lookup table cursor 或默认空格选择目标 |
| `page` | 当前候选页 | 用于上一页按钮/状态 |
| `is_last_page` | 是否末页 | 用于下一页按钮/状态 |
| `select_keys` | Rime 候选选择键 | 用于候选 label；为空时 fallback 到 `1234567890` |
| `ascii_mode` | 当前中英模式 | 只驱动状态显示和模式提示，不替代 Rime 状态 |

状态应用顺序必须固定：

1. 如果 `committed` 非空，并且平台有旧 composition/preedit，先清掉旧 preedit。
2. 提交 `committed`。
3. 设置新的 `preedit` 和 cursor。
4. 更新 `ascii_mode`。
5. 根据 `candidates` 显示或隐藏候选 UI。
6. 返回或记录 `accepted`，决定原生按键是否继续传给客户端。

这个顺序对顶功很重要：一次按键可能同时返回 `committed` 和新的 `preedit`。平台实现不能先设置新 preedit 再提交旧文本。

## 按键事件规范

所有平台前端应尽量向 librime 发送同一形状的事件：

```text
keycode = X11 keysym
mask = Rime modifier mask
```

当前 modifier mask：

| 名称 | 值 | 含义 |
| --- | --- | --- |
| `Shift` | `0x0001` | Shift pressed |
| `Control` | `0x0004` | Control pressed |
| `Alt` / `Mod1` | `0x0008` | Alt/Option pressed |
| `Release` | `1 << 30` | key release event，主要用于 Shift release |

特殊键应使用 X11 keysym：

| 键 | keysym |
| --- | --- |
| Return | `0xff0d` |
| Backspace | `0xff08` |
| Delete | `0xffff` |
| Escape | `0xff1b` |
| Space | `0x0020` |
| Tab | `0xff09` |
| Left / Up / Right / Down | `0xff51` / `0xff52` / `0xff53` / `0xff54` |
| Home / End | `0xff50` / `0xff57` |
| PageUp / PageDown | `0xff55` / `0xff56` |
| Shift_L / Shift_R | `0xffe1` / `0xffe2` |

### printable key 规则

- 平台能拿到布局后的 printable 字符时，应优先传实际字符的 keysym。
- Shift+a 在 macOS/Linux 当前目标行为是传 `0x41` 加 Shift mask，让 librime 的 ASCII composer 自己决定行为。
- Windows TSF 也应对 Shift+字母传大写 ASCII keysym，同时保留 Shift modifier mask；后续如果接入完整键盘布局转换，应继续保持“keysym 表示实际字符、mask 表示修饰键”的语义。
- CapsLock/NumLock 不应进入 Rime modifier mask；如果平台布局已产出大写字符，keycode 本身可体现大写。

### bypass 规则

没有 active composition 时，平台前端应放行这些按键，避免拦截应用行为：

- Space、Return、Backspace、Delete、Tab、Escape。
- Home/End/PageUp/PageDown/方向键。
- Ctrl/Alt/Option 组合键。
- 平台无法转换或无业务意义的功能键、媒体键。

有 active composition 或 candidates 时，以上按键可交给 Rime 或平台候选交互处理。

### Shift release

当前中英切换基线：

1. Shift key down 不切换模式。
2. 只有没有混入其它 keyDown 的 solo Shift release 才送入 Rime。
3. 送入 `Shift_L` 或 `Shift_R` keysym，mask 为 `Release`。
4. 如果 Rime 不接受，而平台需要兜底，可以调用 `set_ascii_mode(!ascii_mode)`。

平台前端必须区分 solo Shift 和 Shift+letter/number/symbol。Shift+其它按键必须走普通按键路径。

## 候选交互规范

候选行为应尽量由 core 统一：

- 空格：有 candidates 时优先选择 `highlighted_candidate_index`。
- 数字/选择键：如果平台协议能直接处理，应根据 `select_keys` 映射到候选 index，再调用 `select_candidate(index)`。
- 上一页/下一页：优先调用 `change_page(backward)`，不要在平台层伪造 page state。
- Enter：有 preedit 时可提交当前 preedit 并 reset session；如果交给 Rime，也必须使用 `XK_Return`。
- Escape/cancel：调用 `reset()`，清 preedit，隐藏候选。
- 鼠标点击候选：调用 `select_candidate(index)`，再按 `ImeState` 应用结果。

如果平台候选服务只支持结构化 lookup table，不支持自绘样式，也仍应保持 label、candidate、highlight 和 page 语义一致。

## 数据目录与部署

桌面平台使用独立 KeyTao 用户目录：

| 平台 | 用户目录 |
| --- | --- |
| macOS | `~/Library/keytao` |
| Linux | `$XDG_DATA_HOME/keytao`，通常是 `~/.local/share/keytao` |
| Windows | `%APPDATA%/keytao` |

共享数据目录由平台查找：

- 优先显式环境变量：`KEYTAO_RIME_SHARED_DATA_DIR`、`RIME_SHARED_DATA_DIR`、`RIME_DATA_DIR`。
- 再查 App 或 IME bundle/runtime 内的 `rime-data` / `SharedSupport` / `share/rime-data`。
- 最后 fallback 到系统 Rime 数据目录，例如 Linux `/usr/share/rime-data`、macOS Squirrel/Homebrew、Windows Weasel。

App 的方案安装只写文件；部署才调用 `keytao_core::deploy()`。任何平台前端都不应自行合并 `default.custom.yaml` 或 `rime.lua`。

## 打包规范

桌面发行包必须把“能部署”和“能输入”需要的 runtime 一起打包，不能让主 App 和系统 IME 使用不同能力集：

- `librime` native library。
- OpenCC 数据和运行时依赖。
- `rime-plugins`，尤其是 Lua 插件。
- 基础 `rime-data`，至少包含 `default.yaml`、`key_bindings.yaml`、`punctuation.yaml`、`symbols.yaml`、`essay.txt` 和 OpenCC 数据。

平台约束：

- macOS 只构建 pkg。pkg 同时安装主 App 和 `/Library/Input Methods/KeyTao.app`，不构建 dmg，因为 dmg 拖拽安装无法可靠完成系统输入法注册。
- Linux 只构建 deb、rpm 和 tar.gz，不构建 AppImage。deb/rpm 通过 Tauri resource 放入 `runtime/`，tar.gz 使用可执行文件同级 `runtime/`。
- Windows 继续使用 installer 方式，并应保持 `resources/rime-data` 和 runtime DLL 闭包完整。

macOS release CI 必须执行 `pnpm build:macos` 和 `scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg`，再上传 `keytao-app-<version>-macos-<arch>.pkg`。当前脚本按 runner 架构构建，例如 `macos-arm64` 或 `macos-x86_64`；不要让 Tauri 的 `dmg` bundle 重新进入 macOS 发行流程。

`keytao-core` 不关心打包格式；它只要求传入可靠的 shared data dir。平台 App/IME 启动代码必须优先选择包内 runtime，再 fallback 到环境变量或系统目录。

## Reload 规范

桌面输入法通过用户目录下的 reload stamp 感知 App 部署：

```text
<user_data_dir>/keytao-ime.reload
```

当前平台行为：

- Linux daemon 每秒检查 stamp mtime，变化后调用 `ImeRuntime::reload()`，已有 `ImeRuntimeSession` 通过 generation 懒刷新。
- macOS IMK 在 `activateServer` 和每次 `handle` 前比较 stamp 内容，变化后调用 `keytao_reload()`；已有 `ImeRuntimeSession` 通过 generation 懒刷新。后续可继续把 stamp 检测也收敛到通用层。
- Windows TSF 在 focus 和 key event 热路径比较 `%APPDATA%/keytao/keytao-ime.reload` 的 mtime/size 签名，变化后调用 `ImeRuntime::reload()`，并在 TSF edit session 中清理旧 composition 和候选窗。

reload 时必须：

1. 清除旧 preedit 和候选 UI。
2. 销毁或重建当前 session。
3. 重新读取状态。
4. 避免在 UI renderer 或候选点击回调里执行 deploy。

## UI 和 `theme.yaml` 边界

主题系统由 `crates/keytao-theme` 提供。平台前端共享主题语义、默认值、校验和 UI model，不共享绘制实现。

当前结构：

```text
theme.yaml
  -> UI color scheme + accent color + mode variant
  -> keytao-theme::ResolvedImeTheme
ImeState-like input + backend capabilities
  -> CandidatePanelModel / ModeHintModel
ResolvedImeTheme + Model
  -> platform renderer
```

`theme.yaml` v2 只表达跨平台可落地语义：

- `ui.colorScheme`：`auto`、`light` 或 `dark`；`auto` 会跟随系统主题，resolved theme 会带上最终 `effectiveColorScheme`。
- `ui.accentColor`：主题强调色，用于派生候选高亮、hover 和模式提示强调色。
- `dark:` / `light:` 模式变体，根级字段仍作为通用配置。
- font family、font size、font weight。
- panel padding、gap、radius、border、shadow、max width、orientation。
- background、foreground、comment、label、highlight、hover、separator、preedit color。
- mode hint size、radius、duration、label、color。

平台映射规则：

- macOS 通过 `keytao-core-ffi` 获取 normalized JSON，再由 AppKit adapter 映射为 `NSColor`、`NSFont`、`NSControl`。
- Linux Wayland/X11/KDE/IBus fallback overlay 通过 `ThemeResolver + CandidatePanelModel / ModeHintModel` 渲染 BGRA buffer。
- Linux IBus/Kimpanel/GNOME 系统候选服务只能映射结构，视觉由桌面环境决定。
- Windows candidate window 和 mode hint window 通过 `ThemeResolver + CandidatePanelModel / ModeHintModel` 渲染 layered window BGRA buffer，但必须尊重 TSF focus/composition 生命周期。

主题不能控制：

- Rime session 状态。
- 候选选择逻辑。
- 候选数量、分页规则或 select key 来源。
- 光标定位、屏幕边界和输入法窗口生命周期。
- 平台按键转发策略。
- reload/deploy。
- 光标 rect 可信度判断和屏幕边界修正。

## 简化后的目标架构

目标是把输入法拆成两类代码：

```text
通用层 keytao-core / keytao-core-ffi
  - librime setup / deploy / reload
  - ImeRuntime / ImeRuntimeSession
  - key event mask normalization
  - ImeState extraction
  - candidate/page/reset/ascii mode operations
  - theme.yaml loader and UI model

平台 IME 层
  - 系统输入法注册和生命周期
  - 原生 key event -> X11 keysym
  - commit / preedit / candidate UI adapter
  - cursor rect and screen bounds
  - platform diagnostics
```

KeyTao App 的理想操作方式：

1. App 安装或更新方案文件。
2. App 调用通用 deploy API 或写入统一 reload request。
3. 正在运行的 IME runtime 收到 reload request。
4. `ImeRuntime::reload()` 重新部署词库并递增 generation。
5. 各平台 session 下一次访问时自动刷新到新词库。
6. 平台层只刷新 UI，不关心 librime 部署细节。

这样 App 可以更灵活地操作 IME：触发部署、查询状态、请求重载、读取诊断；IME 层只负责系统协议、UI 和配置接入。

## 平台接入清单

新增平台前端时按这个顺序实现：

1. 数据目录：确认 `default_user_data_dir()` 和 shared data 查找规则。
2. 初始化：在平台允许的位置创建 `ImeRuntime` 或调用 `keytao_init()`。
3. Session：为每个输入上下文创建独立 `ImeRuntimeSession`，并在上下文销毁时释放。
4. Key map：把平台原生 key event 转为 X11 keysym + Rime modifier mask。
5. Bypass：没有 composition 时放行导航键、删除键、空格、回车、Tab、Escape 和 Ctrl/Alt 组合键。
6. Process：调用 `ImeRuntimeSession::process_key_result()` 或 `keytao_session_process_key()`。
7. Apply state：按固定顺序应用 `committed`、`preedit`、`candidates`、`ascii_mode`。
8. Candidate actions：实现 select candidate、change page、reset。
9. Mode switch：实现 solo Shift release，必要时提供 ascii mode fallback。
10. Reload：接入 `keytao-ime.reload` 或平台等价刷新机制。
11. UI model：接入 `CandidatePanelModel` / `ModeHintModel`，不要让平台 UI 直接发明 state 字段。
12. Diagnostics：提供状态检查和日志入口，至少能定位 shared data、user data、session init、key event、commit/preedit。
13. Tests：覆盖 key map、bypass、commit+new-preedit、candidate select、reload 后 session refresh。

## 不变量

这些规则不能因平台差异改变：

- 一个输入上下文一个 session。
- 平台传给 core 的按键必须是 X11 keysym + Rime modifier mask。
- `ImeState` 是提交、预编辑和候选显示的唯一来源。
- `committed` 必须先于新 `preedit` 应用。
- 没有 composition 时不能吞应用快捷键和导航键。
- UI/theme 不读写 Rime 状态。
- App 负责方案安装和部署；系统输入法负责按键热路径。
- reload 通过用户目录的稳定信号触发，不通过 UI 组件触发。

## 当前已知差异与收敛点

- `keytao-core::key_policy` 已收敛 Enter、空 composition bypass、Space/`select_keys` 候选选择和 Ctrl+grave 转发规则；Linux/Windows 前端应优先复用它，Swift 侧保持同名常量和行为对齐。
- macOS `commitComposition` 和普通 Return 路径都使用 `XK_Return`/`0xff0d`。
- Linux 旧的 `src-tauri/src/ime/linux.rs` 内嵌 Wayland IME 代码已清理；系统输入法维护以 `crates/keytao-linux-ime` daemon 为准。
- Linux GNOME/IBus/Kimpanel 视觉不能完整受 `theme.yaml` 控制；文档和 UI 设置页需要明确“结构生效，视觉受系统限制”。
- Windows TSF 已接入 reload stamp、solo Shift release、候选选择和 Enter direct commit；后续仍需要补更多真实 Windows 桌面回归测试。
- macOS reload stamp 检测仍在 Swift 层；后续可以把 stamp path、mtime/content 检测和 reload request 也收敛到通用 runtime。
- 平台 IME 日志入口不一致：Linux 已有 `/tmp/keytao-ime.log`，macOS 主要是 `NSLog`，Windows 主要靠注册状态和运行时目录排查。

## 文档维护规则

平台实现变化时，同步维护三处文档：

1. 本文：只写跨平台契约、通用状态、统一规范和平台接入清单。
2. 平台 `IMPL.md`：写该平台具体协议、进程、目录、构建、限制和排查。
3. `README.md`：只放用户可理解的入口链接和平台状态。

如果平台实现为了系统限制偏离本文，必须在平台 `IMPL.md` 标出“偏离原因、影响范围、后续收敛方式”。
