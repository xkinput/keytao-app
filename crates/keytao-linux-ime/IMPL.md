# Linux IME 实现说明

本文只记录 `crates/keytao-linux-ime` 里的 Linux 系统输入法 daemon 实现，并按当前代码同步。

跨平台通用契约见 [输入法通用层实现规范](../../docs/ime-common-layer.md)；本文只补充 Linux daemon 的协议、进程和桌面环境差异。

## 代码地图

- `src/main.rs`：进程入口、日志轮转、单实例控制、reload watcher、后端选择、托盘启动。
- `src/engine.rs`：Linux 侧兼容别名，re-export `keytao_core::ImeRuntime` 为 `CoreEngine`、`ImeRuntimeSession` 为 `ImeSession`。
- `src/wayland_backend.rs`：wlroots/非 KDE Wayland，使用 `zwp_input_method_v2`。
- `src/wayland_backend_kde.rs`：KDE/KWin Wayland，使用 KWin 私有启动的 `input-method-v1`。
- `src/gnome_ibus_engine.rs`：GNOME IBus engine，连接现有 `ibus-daemon`。
- `src/ibus_backend.rs`：IBus 兼容 D-Bus shim，服务 Chromium/CEF/Electron 等 IBus 客户端。
- `src/ibus_shared.rs`：两个 IBus 前端共用的按键决策状态机（`key_phase` / `press_route` / `process_key_event`）、content-type→policy 判定（`ContentTypeState`）、模式指示器（`ModeIndicator`）、面板动作（`select_candidate` / `change_page` / `navigation_key`）与 IBusText/IBusLookupTable 构造；GNOME engine 与 IBus shim 只负责各自 D-Bus 接口上的发布。
- `src/x11_backend.rs`：X11 XIM server，注册为 `@im=keytao`。
- `src/panel.rs`：候选窗和模式提示的 BGRA 像素渲染。
- `src/kimpanel.rs`：KDE Kimpanel/impanel2 D-Bus 候选服务。
- `src/reload_bus.rs`：reload 广播，reload watcher 通知各后端清掉旧词库产出的 UI。
- `src/tray.rs`：Linux 托盘菜单，打开 `keytao-app` 或退出 daemon。
- `keytao.xml`：IBus component 描述，安装到 `/usr/share/ibus/component/keytao.xml`。
- `keytao-wayland-launcher.desktop`：KDE Virtual Keyboard 入口。

Tauri 主 App 不直接处理 Linux 系统输入法按键。它负责安装方案、部署方案、状态展示，并在部署后写 `keytao-ime.reload` 通知 daemon 重载；Linux 启动阶段可以尝试拉起 fallback `keytao-ime`，但正式 UI 不再提供系统输入法启动/重启按钮。

当前系统输入法维护边界以 `crates/keytao-linux-ime` 为准。旧的 `src-tauri/src/ime/linux.rs` 内嵌 Wayland IME/overlay 路径已清理，Tauri 主 App 不再保留 Linux 系统输入法前端实现。

## 跨平台前端契约

Linux daemon 要和 macOS IMK、Windows TSF 等前端共享同一套实现边界：

1. `keytao-core` 只负责 librime 初始化、部署、session、按键处理和 `ImeState` 抽取。
2. Linux daemon 负责后端选择、协议接入、按键转发、文本提交、预编辑、候选 UI、托盘和日志。
3. 每个输入上下文必须创建独立 `ImeSession`；上下文销毁时必须一并销毁 session（IBus 走 `org.freedesktop.IBus.Service.Destroy`）。daemon 级 `CoreEngine` 是 `keytao-core::ImeRuntime`，只管理部署、reload generation 和 session 重建。
4. 所有后端都应向 core 传 X11 keysym + modifier mask；mask 过滤与 Shift/CapsLock 归一现在由 core 的 `key_policy::normalize_key_for_modifiers` + `rime_modifier_mask` 统一处理，后端可以直接把 Lock 位传下去，不再需要本地补偿。
5. 任何模式下按键都先进 Rime，未接受再按原生路径放行；只有系统保留修饰键（Super/Meta）组合可以提前放行。空 composition 时仍放行的只有导航/编辑类按键，判定唯一来源是 `key_policy::should_bypass_empty_composition`。
6. 应用 `ImeState` 时必须按固定语义处理：`committed` 走当前协议提交，`preedit` 走当前协议预编辑，`candidates` 走候选服务或 overlay，`ascii_mode` 只驱动状态/模式提示与系统指示器。
7. reload 只通过 `~/.local/share/keytao/keytao-ime.reload` 触发；签名的路径与格式统一用 `keytao_core::ReloadStamp`（daemon 不自己算），变更检测用 peek/commit 的 `ReloadStampGate`，daemon 重载 core 后已有 session 通过 generation 懒刷新。
8. 后端不得各自发明样式、候选 label 或分页语义；这些应来自 `ImeState` 和统一主题/布局模型。
9. 检测到密码/PIN 类输入上下文时必须调用 `ImeSession::set_input_policy(InputContextPolicy::sensitive())` 并清空候选/预编辑 UI。

这套契约也是后续补 Android/Windows/macOS UI 一致性时的基础：协议不同可以分叉，key event 语义、state 应用顺序和 UI 输入模型不能分叉。

## 统一 `theme.yaml` 接入方式

Linux 自绘候选窗已经接入 `crates/keytao-theme`，通过 FreeType + tiny-skia 渲染 BGRA buffer：

- Wayland input-method-v2：`PanelRenderer::render()` / `render_mode_hint()` 输出 SHM buffer，模式提示的颜色、尺寸、圆角和时长来自 `modeHint`。
- KDE Wayland：overlay panel 使用 `PanelRenderer::render()` / `render_mode_hint()`；Kimpanel/impanel2 同时收到结构化候选。
- X11 XIM：XCB overlay 使用同一个 `PanelRenderer`。
- IBus D-Bus shim：系统 lookup table + Kimpanel + 共享 X11 overlay fallback；X11 fallback 的模式提示也走 `render_mode_hint()`。
- GNOME IBus engine：候选 UI 二选一。总线上有 `org.freedesktop.IBus.Panel`（gnome-shell / ibus-ui-gtk3）时只发结构化 lookup table；没有面板且有 `DISPLAY` 时才启动共享 X11 overlay fallback，并把 lookup table 的 `visible` 设为 `false`。

Linux 把“可完全主题化”和“受系统限制”的通道分开：

| 通道 | 可主题化范围 | 限制 |
| --- | --- | --- |
| Wayland SHM popup | 完整控制颜色、字体、间距、圆角、模式提示、尺寸 | 受 compositor popup 定位约束 |
| KDE input panel overlay | 完整控制自绘 overlay | Kimpanel/impanel2 系统候选服务只能表达 label/candidate/page/cursor |
| X11 overlay | 完整控制颜色、字体、间距、圆角、尺寸 | 位置来自 XIM spot location，窗口管理器行为可能不同 |
| IBus D-Bus shim overlay | X11 fallback overlay 可主题化 | IBus lookup table 和 Kimpanel 样式由桌面环境决定 |
| GNOME IBus engine | 无桌面 IBus panel 时的 X11 overlay fallback 可主题化；系统 lookup table 保持结构兼容 | 桌面自带 panel（GNOME 常态）时只能使用系统候选 UI，不叠加自绘 overlay |

当前落成三层：

1. `keytao-theme::ThemeResolver`：读取共享配置，合并默认值，校验类型和范围，输出平台无关 `ResolvedImeTheme`。
2. UI 模型：把 `ImeState`、scale、后端能力规整成 `CandidatePanelModel` / `ModeHintModel`。
3. Linux renderer/adapters：
   - 自绘通道把 `ResolvedImeTheme + Model` 渲染成 BGRA buffer。
   - IBus/Kimpanel 通道只映射候选结构、label、cursor、page 能力，不假装能控制系统主题。

`theme.yaml` v2 表达跨平台可落地语义：`ui.colorScheme: auto | light | dark`、`ui.accentColor`、`light:`/`dark:` 模式变体、字体族、字号、padding、gap、圆角、边框、阴影、最大宽度、横竖排、背景/前景/注释/label/highlight/separator/preedit 颜色、模式提示尺寸/持续时间/文案/颜色。`auto` 会跟随系统主题并解析出最终 `effectiveColorScheme`。缩放仍应保留平台 fallback：Linux 可以继续读取 `KEYTAO_IME_PANEL_SCALE`、`GDK_SCALE`、`QT_SCALE_FACTOR`、`QT_SCREEN_SCALE_FACTORS` 和 X11 `Xft.dpi`，但这些应只影响最终 scale，不覆盖主题语义。

`panel.rs` 不再持有业务意义的固定颜色常量；它只持有字体 fallback、缩放检测和像素渲染算法。各后端也不应直接拼接颜色、间距或模式提示尺寸。

## 运行入口

启动流程在 `main.rs`：

1. 解析参数：
   - `--version` 输出 `keytao-ime` 和 librime 版本。
   - `--ibus-engine` 只运行 GNOME/IBus engine。
   - `--backend=wayland,xim,ibus` 或 `--wayland`、`--xim`、`--ibus` 显式选择后端。
2. 初始化 `$XDG_STATE_HOME/keytao/log/keytao-ime.log`（默认 `~/.local/state/keytao/log`）日志，目录权限 0700，按天滚动并保留 3 天；旧版本留在 `/tmp` 的日志会被删除。日志打不开时退回 stderr，不再让 daemon 启动失败。
3. 创建 `CoreEngine` 并调用 `engine.init()` 部署/初始化 librime。
4. 启动 reload watcher，监听用户数据目录里的 `keytao-ime.reload`。
5. 如果不是 KWin 私有 `WAYLAND_SOCKET` 进程，通过 D-Bus 名称 `org.xkinput.keytao.ime.Daemon` 做单实例检查；若已有旧 daemon，会尝试 `SIGTERM` 后重试。
6. 根据 `WAYLAND_SOCKET`、`WAYLAND_DISPLAY`、`DISPLAY` 和 `XDG_CURRENT_DESKTOP` 选择后端。
7. XIM 和 IBus shim 以线程方式启动；GNOME IBus engine 和 Wayland 主后端在当前线程运行。
8. 非 KWin 私有进程会启动托盘。

## 数据目录与重载

用户数据目录来自 `keytao_core::default_user_data_dir()`，通常是：

```text
~/.local/share/keytao
```

共享数据目录来自 `keytao_core::default_shared_data_dir()`。

发行包里的共享数据目录优先来自包内 runtime：

- deb/rpm：Tauri resource 中的 `runtime/rime-data`。
- fallback：显式环境变量、Nix/system profile、`/usr/local/share/rime-data`、`/usr/share/rime-data`。

`CoreEngine` 是 `keytao_core::ImeRuntime` 的 Linux 侧别名，真实 runtime 行为在 `crates/keytao-core/src/lib.rs`：

1. `init()` 首次调用 `deploy(user_data_dir, shared_data_dir)`。
2. `create_session()` 为每个输入上下文创建独立 `Engine`。
3. `reload()` 重新部署 librime，并递增 `generation`。
4. 每个 `ImeSession` 在 `state()`、`process_key_result()`、`select_candidate()`、`reset()` 前检查 generation；发现变化后重建内部 `Engine`。

App 部署成功后会写：

```text
~/.local/share/keytao/keytao-ime.reload
```

daemon 每秒检查一次签名（`<len>:<mtime_nanos>:<内容哈希>`，格式与路径的唯一实现在 `keytao-core::ReloadStamp`，daemon 不再自己算），变化后调用 `CoreEngine::reload_without_deploy()`。core 会丢弃全部存活 session 的 Engine 并重新 finalize/initialize librime，因此已存在的输入上下文会在下一次访问 session 时自动刷新，并保留重建前的 ascii_mode。

签名是 **peek/commit** 语义（`main.rs` 的 `ReloadStampGate`），不用消费式的 `ReloadStampWatcher::has_changed()`：后者一问就把签名标记为已见，如果这次 reload 因为部署文件还没写完而失败，请求就永久丢了，daemon 会一直用旧词库。`pending()` 只看不消费，`commit()` 在 reload 真的成功后才推进基线，并且推进到**本次加载的那个签名**，所以 reload 期间 App 又写一次 stamp 也不会被吞掉。这与 `keytao-core-ffi` 的 `stamp_changed_peek() && reload_now()` 一致。

session 是懒刷新的，所以 reload 完成时屏幕上还留着旧词库产出的 preedit 和候选。`src/reload_bus.rs` 负责把这一步补上：watcher 在 `reload_without_deploy()` 成功后调 `reload_bus::notify()`，每个后端在启动时 `subscribe()` 拿到一个 eventfd 并把它挂进自己的 `poll()`，收到通知后执行与失焦相同的清理：

| 后端 | reload 后的动作 |
| --- | --- |
| Wayland input-method-v2 | `clear_composition()` + 清客户端 preedit + 隐藏 popup |
| KDE input-method-v1 | `clear_composition()` + 清 context preedit + 清 Kimpanel + 隐藏 overlay |
| X11 XIM | 隐藏候选窗（preedit 本来就画在这个窗口里），并递增 `reload_epoch`：每个 IC 记住自己 `last_state` 的 epoch，reload 之后的 `SetICValues`（光标移动）不会把旧词库的候选重新画回来；per-IC session 仍按 generation 懒刷新 |
| IBus shim / GNOME engine | 未接入：两者是 tokio 异步事件循环，没有 poll 循环可挂 fd，目前仍靠 generation 懒刷新 |

## 后端选择

默认选择逻辑在 `BackendSelection::for_session()` 和 `main()`：

- GNOME/Unity/Budgie/Pantheon/Cinnamon：`gnome_ibus_engine`，如果有 X11 再启动 XIM。
- KDE 普通会话：IBus shim + XIM，不抢 KWin 原生 Wayland 输入法槽位。
- KWin Virtual Keyboard 私有进程：如果存在可解析的 `WAYLAND_SOCKET`，只运行 KDE `input-method-v1` 后端。
- 其他 Wayland + X11：Wayland input-method-v2 + XIM + IBus shim。
- 纯 Wayland：Wayland input-method-v2。
- 纯 X11：XIM + IBus shim。

KDE 普通 daemon 会清理旧的：

```text
~/.config/plasma-workspace/env/keytao.sh
```

避免旧环境文件覆盖 KWin Virtual Keyboard 的路由。

## 统一按键模型

Linux 各后端尽量遵守同一套 librime 事件形状：

1. keyval 使用 librime 期望的 X11 keysym。
2. 可打印 ASCII 使用当前键盘布局实际产生的 keysym；例如 Shift+a 传 `XK_A`/`0x41`，不是降回 `XK_a`/`0x61`。
3. modifier mask 保留 Shift、Control、Alt、Super 和 release mask；core 的 `rime_modifier_mask()` 会过滤 NumLock、鼠标等噪声位，CapsLock 由 `normalize_key_for_modifiers` 折进 keysym。
4. Ctrl/Alt 组合键**不再**提前放行：先送 Rime（`Ctrl+grave`、`F4` 等 key_binder 绑定需要它），未接受再放行。只有 Super/Meta 这类系统保留组合可以提前放行。
5. 没有 composition 时，空格、回车、退格、删除、Tab、Escape、导航键等直接放行。
6. Shift 自身的按下不切换模式；Shift release 送入 Rime，用于中英模式切换。
7. Shift+字母、Shift+数字符号不是 solo Shift，必须走普通 key press 路径，让 librime 的 ASCII composer 决定提交大写或符号。
8. F4 不属于空 composition bypass；Wayland/X11/IBus 后端收到 `XK_F4` / `0xffc1` 时会送入 librime 打开 Rime schema / options 菜单。
9. 有 composition 时按 Enter 一律走 `ImeSession::process_enter()`（把 `XK_Return` 交给 Rime，Rime 不接受时由 core 退回 `commit_raw_input()`）。后端**不得**再自己提交 preedit 字面量。
10. 数字/选择键一律送 Rime，由 schema 的 `select_keys` / `key_binder` 决定；后端不得本地拦截数字键做选词。UI 点击选词走 `select_candidate_on_page(index)`。
11. 翻页走 `change_page(backward)`，清空走 `clear_composition()`，失焦需提交走 `commit_composition()`，都不再伪造按键。
12. `accepted=false` 时放行或转发按键。
13. 有 `committed` 时用当前后端原生接口提交。
14. 有 `preedit` 时用当前后端原生接口更新客户端预编辑；`ImeState::cursor` 是 Unicode 标量偏移，IBus 可直接用（`engine::ibus_cursor_pos()` 只做越界收敛），Wayland text-input（v1/v2）要的是**字节偏移**，两个 Wayland 后端各有一个 `preedit_cursor_bytes()` 做换算。
15. 有 `candidates` 时更新候选窗或候选服务。
16. 键盘被 IME grab 之后，compositor 不再替被消费的按键做重复。Wayland 两个后端记录 `repeat_info` 的 rate/delay，对**被 IME 吃掉**的键自己按 delay/interval 重放；被放行的键仍由客户端自己重复，不额外处理。是否可重复由 keymap 决定（`xkb_keymap_key_repeats`），修饰键与标了 `repeat=no` 的键不重复，和普通 `wl_keyboard` 客户端一致。

这套规则是后续实现其它平台前端时的兼容基线：keyval 表达“实际字符”，modifier mask 表达“同时按住的修饰键”。

## Wayland input-method-v2

文件：`src/wayland_backend.rs`

协议：

- `zwp_input_method_manager_v2`
- `zwp_input_method_v2`
- `zwp_input_method_keyboard_grab_v2`
- `zwp_input_popup_surface_v2`
- `zwp_virtual_keyboard_v1`
- `wl_shm`

实现要点：

- 用 keyboard grab 接管按键。
- 用 xkbcommon 从 evdev keycode 得到 keysym；`key_get_one_sym` 已经包含 Shift 后的 printable keysym。
- 用 xkb state 提取 Shift、Control、Alt。
- 未消费的物理按键优先通过 virtual keyboard 原样转发。
- 如果没有 virtual keyboard，只能用 `commit_string`/`delete_surrounding_text` 转发空格、回车、退格、删除、Tab；方向键等无法通过该 fallback 表达。
- **所有** `ImeState` 都经过同一个 `apply_state_to_input_method()`：先 `commit_string`，再按 `ImeState.preedit` + 字节游标 `set_preedit_string`，最后 `commit(serial)`。空格选词、Enter 与普通按键三条路径共用它，任何一条都不会再把客户端 preedit 清空。
- Enter 走 `ImeSession::process_enter()`，数字/空格一律送 Rime；后端不再自己提交 preedit 字面量，也不再本地拦截空格选中高亮候选。
- `accepted=false` 时也要先应用一次 `ImeState` 再转发按键：librime 拒绝按键的同时仍可能顺手 flush 一个 commit（`ascii_composer` 切模式时会确认当前 composition），直接转发会把这段文字丢掉。
- Ctrl+` 这类 librime 接受但应用也需要的快捷键会额外转发。
- 候选窗使用 SHM buffer 和 popup surface 绘制在光标附近。
- 中英模式变化时显示 `modeHint` 配置的 `英`/`中` 模式提示，颜色、尺寸、圆角和时长来自主题。
- `Deactivate` 有 180ms debounce，避免焦点切换瞬间误丢按键。
- `content_type` 事件是双缓冲的，记进 `pending_content_type`，在 `done` 时应用；`activate` 会把它重置为协议默认值（hint `none`、purpose `normal`）。purpose 为 `password`(8)/`pin`(9) 或 hint 含 `hidden_text`(0x40)/`sensitive_data`(0x80) 时切到 `InputContextPolicy::sensitive()` 并清 UI。
- `activate` 会重置 compositor 侧 preedit（协议原文如此）。debounce 内重新 activate 时 KeyTao 的 session 仍持有旧编码，因此 `done` 处理 activate 后会把当前 composition 用 `set_preedit_string` + `commit` **重发一次**，让客户端和 Rime 重新对齐。这里选"重发"而不是"reset"，是为了保住 180ms debounce 本来要保护的"焦点抖动不丢字"；代价是从输入框 A 快速点到 B 时，A 未完成的编码会显示在 B 的预编辑里（可见且可退格，不会静默混进提交）。
- `repeat_info` 的 rate/delay 用来自己重复被消费的按键（组字中的 BackSpace、方向键、选词键），主循环的 poll 超时按最近的 deadline 计算。
- debounce 到期真正失焦时会调 `zwp_input_method_keyboard_grab_v2.release()`（该请求是 destructor，wayland-client 不会在 `Drop` 时替你发），下一次 `activate` 再重新 grab；只丢弃 proxy 会在合成器侧堆积 grab 对象。
- 如果 compositor 返回 `Unavailable`，Wayland 后端退出，但 XIM/IBus shim 线程继续服务。

## KDE/KWin Wayland

文件：`src/wayland_backend_kde.rs`

KWin 6 的虚拟键盘路径使用 input-method-v1：

- `zwp_input_method_v1`
- `zwp_input_method_context_v1`
- `zwp_input_panel_v1`
- `zwp_input_panel_surface_v1`
- `wl_keyboard`
- `wl_shm`

实现要点：

- 该后端只应该由 KWin 通过私有 `WAYLAND_SOCKET` 启动。
- `Activate` 后 grab keyboard，保存 `ZwpInputMethodContextV1`。
- 未消费按键通过 `context.key(serial, time, evdev_key, state)` 转发。协议要求这四个参数原样来自 `wl_keyboard::key`，所以 serial 用 `last_key_serial`（**不是** `commit_state` 的 serial）。
- 只有按下时被放行的键才转发 release（`forwarded_keys` 集合），否则客户端会收到没有配对 press 的 release。
- modifier 通过 `context.modifiers(...)` 转发给 KWin。
- 提交使用 `context.commit_string(serial, text)`，预编辑使用 `preedit_cursor` + `preedit_string`。`preedit_string` 的第三个参数是"客户端在 reset/失焦时用来替换预编辑的提交文本"，这里传当前 preedit 本身，组字中被 reset 不再直接丢字。本进程在 debounce 到期时只 `clear_composition()` 丢弃自己这一侧的组字、不主动 `commit_string`，是否上屏完全由 KWin 依据这个字符串决定，因此两侧不会重复提交。
- Enter 走 `ImeSession::process_enter()`，数字/空格一律送 Rime；不再自己提交 preedit 字面量。
- `accepted=false` 时先 `commit_state_to_context()` 再清 UI 并转发按键，原因同 input-method-v2 一节：拒绝按键不代表没有 commit。
- `content_type` 事件按 **text-input-v1** 的编号解析：purpose `password` 是 8（v1 没有 `pin`，9 是 `date`），hint 的 `hidden_text`(0x40) / `sensitive_data`(0x80) / `password`(0xc0) 命中即切 `InputContextPolicy::sensitive()`。不要复用 v2/IBus 的常量表。
- 候选状态同时更新 Kimpanel 和 input panel overlay。
- overlay panel 设置空 input region，避免候选窗拦截鼠标点击。
- 失焦有 180ms debounce，到期后 `reset_context_state()` 会 `keyboard.release()` + `context.destroy()`：协议要求 deactivate 处理完必须销毁 context，而 wayland-client 的 proxy 在 `Drop` 时不会自动发 destructor。销毁前后 `Dispatch<ZwpInputMethodContextV1>` 会丢弃非当前 context 的事件，避免旧 context 的 `commit_state`/`reset` 覆盖新 context 的状态。
- 中英模式变化时，input panel overlay 用 `render_mode_hint()` 显示主题化模式提示，并按 `modeHint.duration` 自动隐藏；同时把新模式记进 `pending_kimpanel_mode`，由事件循环调用 `KimpanelHandle::update_mode()` 发 `UpdateProperty`，让 Plasma 指示器跟随 Rime 状态（Kimpanel 调用是 async，不能在 Wayland dispatch 里直接 await）。
- `wl_keyboard` 是 `grab_keyboard` 创建的、版本为 1 的对象，早于 `repeat_info`（v4 才有），所以 KDE 路径上收不到用户的重复设置。抓到版本 < 4 的键盘时按 xkb/Plasma 默认值（delay 600 ms、rate 25/s）兜底，否则组字中长按退格完全不重复；KWin 将来提高版本后 `repeat_info` 会直接覆盖这个兜底值。

### 已知偏离：Kimpanel 候选窗无法跟随光标

`input-method-unstable-v1` 没有任何光标矩形事件（`zwp_input_method_context_v1` 全部事件里没有 spot/rectangle，`zwp_input_panel_surface_v1` 也没有 v2 的 `TextInputRectangle`），因此 KDE 路径拿不到光标位置，**不发** kimpanel 的 `UpdateSpotLocation`，也不调 `org.kde.impanel2.SetSpotRect`。

影响与取舍：

- 主候选 UI 是 `set_overlay_panel()` 创建的 SHM overlay，由 KWin 按文本光标定位，不受此限制。
- 只有额外加载了 kimpanel/impanel 面板的用户会看到第二份候选列表，它会停在面板自己的默认位置。之所以仍然向 kimpanel 推送候选与 preedit，是因为同一通道还承载中英状态属性（`RegisterProperties` / `UpdateProperty`），KDE 用户依赖它显示模式指示器。
- 收敛条件：将来若接入能提供光标矩形的通道（`text-input-v2` 的 `TextInputRectangle` 或 KWin 扩展），再补 `UpdateSpotLocation` + impanel2 `SetSpotRect`。IBus shim 路径有 `SetCursorLocation`，所以那条通道是发 `UpdateSpotLocation` 的。

### 已知偏离：两个 Wayland 后端是进程级单 session

`docs/ime-common-layer.md` 的不变量要求“一个输入上下文一个 session”。IBus shim（每个 `CreateInputContext`）、GNOME engine（每个 `CreateEngine`）、XIM（每个 IC）都遵守，但 `wayland_backend.rs` 与 `wayland_backend_kde.rs` 在 `run()` 里只 `create_session()` 一次，整个 daemon 进程共用。

- 原因：两个 input-method 协议里，输入法进程对合成器只暴露**一个** `zwp_input_method_v2` / `zwp_input_method_v1` 对象，`activate`/`deactivate` 只表示“当前有没有文本输入”，并不给出可用来区分应用的稳定标识；v1 的 context 对象虽然每次 activate 换一个，但 KWin 在同一应用内也会换，用它当 session key 会在同一输入框里丢状态。
- 影响：`ascii_mode` 与组字状态跨应用共享——在 A 应用切成英文，切到 B 应用仍是英文。`activate` 时会把 `InputContextPolicy` 复位成默认值，所以密码框的敏感策略**不会**泄漏到下一个应用。
- 收敛方式：v2 的 `activate`/`deactivate` 与 v1 的 context 对象足以支撑“每次 activate 建 session、deactivate 销毁”，代价是每次焦点切换都丢弃组字（现在的 180ms debounce 正是为了避免这一点）。要真正做到 per-context，需要合成器提供稳定的文本输入标识。

## GNOME IBus Engine

文件：`src/gnome_ibus_engine.rs`

GNOME/mutter 不提供 `zwp_input_method_manager_v2`，所以 KeyTao 作为 IBus engine 接入 GNOME 自带 `ibus-daemon`：

1. 连接 **ibus-daemon 自己的总线**，不是 session bus。地址优先读 `IBUS_ADDRESS`，否则解析 `~/.config/ibus/bus/<machine-id>-unix[-wayland]-<n>` 里的 `IBUS_ADDRESS=` 行；该文件里的 `IBUS_DAEMON_PID` 等于本进程时会跳过（本进程的 IBus shim 也写同名文件，连上自己等于谁都没注册）。两者都没有时才退回 session bus。
2. 在 `/org/freedesktop/IBus/Factory` 暴露 `org.freedesktop.IBus.Factory` 和 `org.freedesktop.IBus.Service`，并申请总线名 `org.freedesktop.IBus.KeyTao`——ibus-daemon 就是用 component 的总线名去找它的 factory 的，这个名字必须和 `keytao.xml` 的 `<name>` 一致。
3. 构造 `IBusComponent` 和 `IBusEngineDesc` 调用 `RegisterComponent`，只作为“没装 component XML 的会话”的补充；常规安装靠 `/usr/share/ibus/component/keytao.xml`。
4. GNOME/IBus 调用 `CreateEngine("keytao")` 时创建一个独立 `ImeSession`，同一路径上同时暴露 `org.freedesktop.IBus.Engine` 与 `org.freedesktop.IBus.Service`。
5. `ProcessKeyEvent` 转给 `ImeSession`。
6. 用 IBus signals 提交文本、更新 preedit 和 lookup table。
7. `focus_out`、`reset`、`disable` 会 `clear_composition()` 并隐藏 UI；daemon 销毁引擎时调用的 `Service.Destroy` 会清 composition、从 object server 摘掉两个接口并释放 session。

协议细节（都以 ibus 1.5 的内省 XML 为准，可用 `gdbus introspect` 复核）：

- `UpdatePreeditText` 的签名是 `(v u b u)`，第 4 个参数是 `IBusPreeditFocusMode`。少一个参数时 ibus-daemon 的 `g_variant_get(parameters, "(vubu)", ...)` 会直接失败，GNOME 下预编辑完全不生效。本实现固定发 `CLEAR`(0)：引擎在 `focus_out` 里自己清 composition，不需要客户端替它提交。注意 `org.freedesktop.IBus.InputContext` 的同名信号是 3 参数，两者不能混。
- `CommitText(v)`、`UpdateLookupTable(v b)` 与上游一致。
- `ContentType` 是 `(uu)` 属性（daemon 走 `Properties.Set` 写入），不是方法；`SetContentType(uu)` 方法保留给旧 daemon。
- 中英模式通过 `RegisterProperties` / `UpdateProperty` 暴露给面板：`FocusIn` / `Enable` 时注册一次，`ascii_mode` 变化时发 `UpdateProperty`，属性 label/symbol 为 `中` / `英`。

候选 UI 二选一：`run()` 探测总线上有没有 `org.freedesktop.IBus.Panel`（gnome-shell / ibus-ui-gtk3）。有面板就只发结构化 lookup table，不启动自绘 X11 overlay，避免同屏出现两份候选列表；没有面板且有 `DISPLAY` 时才启动 overlay，并把 `UpdateLookupTable` 的 `visible` 传 `false`。`KEYTAO_IME_FORCE_OVERLAY=1` 可强制自绘。

`page_up` / `page_down` 调用 `ImeSession::change_page()`，`cursor_up` / `cursor_down` 送 `XK_Up` / `XK_Down` 给 Rime，`candidate_clicked` 调用 `select_candidate_on_page(index)` 后按统一 `ImeState` 顺序提交、清 preedit 并刷新 lookup table。

### IBus component 注册

`crates/keytao-linux-ime/keytao.xml` 描述 component 和 `keytao` engine，安装位置：

- deb/rpm：`src-tauri/tauri.linux.conf.json` 的 `bundle.linux.deb.files` / `rpm.files` → `/usr/share/ibus/component/keytao.xml`。
- Nix：`flake.nix` 的 `keytaoLinuxIme.postInstall` → `$out/share/ibus/component/keytao.xml`，并把 `<exec>` 替换成绝对路径。

只靠运行时 `RegisterComponent` 是不够的：那份注册随调用进程存活，ibus-daemon 重启或重新登录后 KeyTao 会从输入源列表里消失，daemon 也无法按 `<exec>` 拉起引擎。

验证（在真实 GNOME/IBus 会话里）：

```bash
ls /usr/share/ibus/component/keytao.xml
ibus restart
ibus list-engine | grep keytao          # 应能看到 keytao
gdbus introspect --session --dest org.freedesktop.IBus.KeyTao \
  --object-path /org/freedesktop/IBus/Factory   # 应有 Factory + Service 两个接口
```

之后在 GNOME 设置 → 键盘 → 输入源 → 添加（其他 → 中文 → KeyTao）或 `ibus-setup` 里应能看到 KeyTao。

## IBus 兼容 D-Bus 后端

文件：`src/ibus_backend.rs`

这是面向 Chromium/CEF/Electron 等应用的轻量 IBus 协议实现，不依赖真实 `ibus-daemon`：

- 自己申请 `org.freedesktop.IBus`。
- 在 `/org/freedesktop/IBus` 暴露 `CreateInputContext`、engine list、global engine 等方法。
- 每个 input context 都创建独立 `ImeSession`，并在同一路径上暴露 `org.freedesktop.IBus.Service`：GTK/Chromium 的 `IBusProxy` 释放上下文时调的是 `Service.Destroy`（`org.freedesktop.IBus.InputContext` 本身没有 Destroy），少了这个接口每个被丢弃的上下文都会漏一个 D-Bus 对象和一个 librime session。
- 实现 `ContentType` `(uu)` 属性（另保留 `SetContentType(uu)` 方法给旧客户端）：purpose 为 PASSWORD(8)/PIN(9)，或 hints 含 PRIVATE(1<<11) 时切到 `InputContextPolicy::sensitive()`，按键直通宿主并清掉候选/预编辑。
- 写入 `~/.config/ibus/bus/*` 地址文件，方便 GTK/Chromium 类客户端发现当前 D-Bus 地址。
- 同时申请/服务 `org.kde.kimpanel.inputmethod`，给 Kimpanel 发候选、预编辑和状态属性信号。启动时发一次 `RegisterProperties`，`ascii_mode` 变化时发 `UpdateProperty`，属性串形如 `/KeyTao/im:KeyTao:input-keyboard:KeyTao:menu,label=中`，Plasma 面板读 `label=` 显示中英状态。
- 另起线程显示 X11 override-redirect overlay 候选窗，作为没有桌面候选服务时的 fallback。
- 对 Chromium/CEF，提交文本前先发送空 `UpdatePreeditText`，避免旧 preedit 区域残留或提交位置错乱。
- `SetCursorLocation` 会记录光标坐标，用于 Kimpanel 和 X11 overlay 定位。

`page_up` / `page_down`、`cursor_up` / `cursor_down` 和 `candidate_clicked` 与 GNOME IBus engine 共享同一套行为：翻页走 `change_page()`，上下移动送 `XK_Up` / `XK_Down`，点击候选走 `select_candidate_on_page(index)`。property activate/show/hide 当前仍为空方法。

`ProcessKeyEvent` 的决策不再由两个后端各写一份：`src/ibus_shared.rs` 里的 `process_key_event(session, keyval, state)` 统一判定敏感直通、Shift release 模式切换、空 composition 放行、Enter 交给 Rime 与普通按键，并返回 `KeyOutcome::{Forward, ClearUi, ModeChanged, Publish}`；`ibus_backend` 与 `gnome_ibus_engine` 只把 outcome 翻译成各自 D-Bus 接口上的信号。纯决策部分（`key_phase` / `press_route`）有单测，不需要 librime。

按键之外还有三处决策同样收在 `ibus_shared`，两个前端只剩发布代码：

- `ContentTypeState`：记住客户端最后设置的 `(purpose, hints)`（IBus 把它作为可读属性暴露），映射成 `InputContextPolicy`，并回答"这一次是不是刚跨进密码框"——只有跨进去的那一次才需要拆掉屏幕上的 preedit 和候选。上一次的 policy 由上一次的 content type 推出而不是回读 session，因为这两个前端里 content type 是 session policy 的唯一写入方。
- `ModeIndicator`：中英模式指示器的发布门禁（模式没动就不发信号；有候选列表时抑制 overlay 模式提示，但属性照发）。
- `select_candidate` / `change_page` / `navigation_key`：面板手势一律走 librime 官方候选与翻页入口，不合成数字或 `-`/`=`；`navigation_key` 在 librime 未接受时不动屏幕，因为面板没有可以回退的宿主客户端。

`ContentTypeState` 与 `ModeIndicator` 都是纯状态，有不依赖 librime 的单测。

## 敏感输入上下文

密码/PIN 输入框里 KeyTao 必须完全不组字：不弹候选、不产生 preedit、也不能把密码字符送进 librime（否则会进用户词库）。core 提供 `InputContextPolicy { composing, learning }`，`composing=false` 时 `process_key_result` 根本不调用 librime。

判定在各后端本地完成，因为**三套协议的常量表互不相同，绝不能互相复用**，只有"命中即 `InputContextPolicy::sensitive()`"这个结论是共享的：

| 后端 | 来源 | 敏感判据 | 状态 |
| --- | --- | --- | --- |
| GNOME IBus engine | `org.freedesktop.IBus.Engine` 的 `ContentType` 属性 | `engine::ibus_content_type_policy()`：purpose PASSWORD=8 / PIN=9，或 hints 含 PRIVATE=1<<11 | 已接入 |
| IBus D-Bus shim | `org.freedesktop.IBus.InputContext` 的 `ContentType` 属性 | 同上 | 已接入 |
| Wayland input-method-v2 | `zwp_input_method_v2` 的 `content_type` 事件（双缓冲，`done` 时生效） | `wayland_backend::content_type_policy()`：**text-input-v3** purpose PASSWORD=8 / PIN=9，或 hint 含 `hidden_text`=0x40 / `sensitive_data`=0x80 | 已接入 |
| KDE input-method-v1 | `zwp_input_method_context_v1` 的 `content_type` 事件 | `wayland_backend_kde::content_type_policy()`：**text-input-v1** purpose PASSWORD=8（v1 无 PIN，9 是 date），hint 同 0x40/0x80/0xc0 | 已接入 |
| X11 XIM | 协议无 content-type 概念 | — | **无法检测**，见下 |

切进敏感上下文时后端会立刻清掉候选和预编辑 UI；按键路径的第一件事就是检查 `input_policy().composing`，为 false 时按键直通宿主，连 UI 信号都不发。

已知局限：XIM 协议不表达输入用途，XIM 后端无法识别密码框。X11 应用通常自己会在密码框上禁用输入法（`XSetICFocus` 不调用或直接不建 IC），但这属于应用侧责任，KeyTao 在 XIM 路径上没有兜底手段。

## X11 XIM 后端

文件：`src/x11_backend.rs`

实现要点：

- 注册 XIM server 名称 `keytao`。
- 需要会话环境 `XMODIFIERS=@im=keytao`。
- 如果 XWayland 懒启动导致 `DISPLAY` 暂时不可连，会每秒重试。
- 初始化时读取 X11 keycode 到 keysym 的 mapping。
- `ForwardEvent` 里根据 Shift bit 在 unshifted/shifted keysym slot 中选择 keysym（`keysym_at_level()`）。X11 会把用不到的 level 填成 `NoSymbol`，协议规定这种 group 视为重复第一个 keysym，所以 shifted slot 为 0 时必须回退到 unshifted——否则带着 `ShiftMask` 的 **Shift 抬起**会解析成 `NoSymbol`，solo Shift 中英切换直接失效。
- 使用 XIM commit 提交文本。Enter 走 `ImeSession::process_enter()`，数字/空格一律送 Rime。
- 声明的 input style 依次是 `PREEDIT_POSITION | STATUS_NOTHING`（over-the-spot，首选）、`PREEDIT_NOTHING | STATUS_NOTHING`、`PREEDIT_NONE | STATUS_NONE`。**不提供** on-the-spot（`PREEDIT_CALLBACKS`）：那条路径会让 Electron/Chromium X11 客户端的 preedit 卡住。over-the-spot 下预编辑仍由 KeyTao 自绘，客户端只负责用 `XNSpotLocation` 上报光标，所以 `draw_client_preedit()` 只对 `PREEDIT_CALLBACKS` 生效（当前恒不生效，保留给将来）。
- 候选窗口用 XCB override-redirect window 绘制。位置来自 `preedit_spot()` 翻译到 root 坐标，并对屏幕边界做钳制；`SetICValues` 会立刻按新 spot 重画（over-the-spot 客户端组字时会持续更新光标）。没有上报 spot 的 root-window 客户端，spot 停在焦点窗原点，那是 XIM 在该风格下能提供的最好锚点。
- 因为预编辑画在这个窗口里，只要 `preedit` 或 `candidates` 非空就显示面板（不再要求必须有候选）。
- `filter_events()` / `SetEventMask` 都是 `KeyPress | KeyRelease`（掩码 3）。KeyRelease 只用于让 librime 的 `ascii_composer` 看到 solo Shift 做中英切换，处理完一律返回 `false` 让事件回到客户端；模式变化时用 `render_mode_hint()` 在同一个窗口显示 `中` / `英` 提示，到期由主循环隐藏。
- 主循环不再阻塞在 `wait_for_event()`：先把 XCB 已缓冲的事件 drain 干净，再 `poll()` X11 socket 与 reload eventfd，超时按模式提示的 deadline 计算。

## 候选窗与字体

文件：`src/panel.rs`

- 使用 FreeType + tiny-skia 渲染 BGRA 像素 buffer。
- Wayland 通过 `wl_shm` 上传，X11 通过 XCB `put_image` 上传。
- 默认视觉来自 `keytao-theme/default-theme.yaml`；用户覆盖路径为 `~/.local/share/keytao/theme.yaml`，开发覆盖可用 `KEYTAO_IME_THEME_PATH`。
- `PanelRenderer` 每次渲染通过 `ThemeResolver` 获取按 mtime/size 缓存后的主题，因此修改 `theme.yaml` 后下一次候选窗刷新即可生效。
- 正文优先使用 `KEYTAO_IME_FONT`，否则通过 fontconfig 查找中文字体，再尝试常见 CJK 字体路径。
- 符号/emoji 优先使用 `KEYTAO_IME_SYMBOL_FONT`，否则查找 Noto Symbols/Emoji。
- 缩放读取 `KEYTAO_IME_PANEL_SCALE`、`GDK_SCALE`、`QT_SCALE_FACTOR`、`QT_SCREEN_SCALE_FACTORS`；X11 还会读取 `xrdb -query` 的 `Xft.dpi`。
- `render_mode_hint()` 渲染 `英`/`中` 模式提示，目前由 input-method-v2、KDE Wayland overlay、IBus X11 fallback 和 GNOME IBus X11 fallback 使用。

## 与 macOS 实现的关键差异

| 维度 | Linux daemon | macOS IMK |
| --- | --- | --- |
| 进程模型 | App 启动/重启独立 `keytao-ime` daemon；KDE 原生 Wayland 另有 KWin 私有进程 | 系统按需启动 `/Library/Input Methods/KeyTao.app` |
| 后端数量 | 同一 daemon 编排 Wayland、KDE、GNOME IBus、IBus shim、XIM | 单一 IMK/TIS 输入源 |
| UI 通道 | 自绘 SHM/X11 overlay + IBus/Kimpanel 系统候选服务 | 自有 AppKit `NSPanel` |
| 主题能力 | 自绘通道可完整主题化，系统候选服务受桌面环境限制 | 候选窗/模式提示都可由 AppKit renderer 完整映射主题 |
| 文本提交 | 每个后端使用自己的协议提交/预编辑接口 | `IMKTextInput.insertText` 和 `setMarkedText` |
| 重载 | watcher 用 `ReloadStampGate` peek/commit 比较签名（失败可重试），session generation 懒刷新，再经 `reload_bus` 广播清 UI | 激活或定时器比较 reload stamp |
| 日志 | `~/.local/state/keytao/log/keytao-ime.log` 滚动日志（0700），App 可读取 | 主要 `NSLog`，尚未纳入 App 日志采集 |
| 模式提示 | input-method-v2、KDE overlay、IBus X11 fallback 自绘；GNOME/系统候选服务受桌面环境限制 | AppKit HUD |

Linux 的复杂度主要来自桌面协议分裂，不应该让这些差异泄漏到 core 或主题配置。统一规范应把“后端能力”作为 adapter 层能力声明，而不是把 GNOME/KDE/X11 的细节写进 `theme.yaml`。

## 后续补齐顺序

建议按风险从低到高推进：

1. 把 IBus/Kimpanel 结构通道也显式标注 `UiCapabilities::system_lookup_table()`，避免误认为视觉会完全生效。
2. IBus shim 与 GNOME engine 接入 `reload_bus`（两者是 tokio 异步循环，需要用 `tokio::io::unix::AsyncFd` 或换成 `tokio::sync::watch`）。
3. KDE 路径若接入能提供光标矩形的通道，补 kimpanel `UpdateSpotLocation` + impanel2 `SetSpotRect`。
4. 继续补真实 Linux 桌面 golden 回归：目前的 `cargo test -p keytao-linux-ime` 只覆盖纯逻辑，协议层靠 `gdbus introspect` 手工复核。

## App 对接点

Tauri 主 App 的 Linux 相关命令在 `src-tauri/src/lib.rs`：

- `linux_ime_status`
- `linux_start_ime`
- `linux_restart_ime`
- `linux_enable_kde_support`

正式 App UI 只展示 `linux_ime_status` 的结果，不再提供启动、重启或 KDE 配置按钮。`linux_start_ime`、`linux_restart_ime`、`linux_enable_kde_support` 保留为开发/诊断和迁移接口，避免普通用户在 App 内直接操作系统输入法组件。

App 启动时会尝试启动 fallback `keytao-ime`。KDE 原生 Wayland 配置仍由系统包、桌面配置或开发接口写入：

```text
~/.local/share/applications/keytao-wayland-launcher.desktop
kwinrc [Wayland] InputMethod=keytao-wayland-launcher.desktop
```

普通 fallback daemon 和 KWin 私有进程是两个角色：前者服务 XIM/IBus，后者服务 KDE 原生 Wayland。

## 构建

- `scripts/build-linux.sh` 通过 Docker builder 生成 Linux 包。
- `scripts/container-build.sh` 在容器里构建 Tauri 包和 `keytao-ime`。
- Linux 发行目标只包含 `deb` 和 `rpm`，不构建 AppImage 或 tarball。
- deb/rpm 通过 Tauri resource 打入 `target/keytao-linux-runtime`，并同时包含 `keytao-app`、`keytao-ime` 和 runtime。
- deb/rpm 还通过 `bundle.linux.deb.files` / `rpm.files` 安装 `/usr/share/ibus/component/keytao.xml`；`scripts/verify-linux-bundles.sh` 会校验它在两种包里都存在。
- runtime 必须包含 `librime.so.*`、OpenCC 数据、`rime-plugins`、基础 `rime-data`，以及 librime/OpenCC 需要的非系统依赖。
- `keytao-app` 和 `keytao-ime` 构建时写入 RUNPATH，覆盖 `$ORIGIN/runtime/lib`、Tauri resource runtime、deb/rpm 的 `/usr/lib/keytao-app/...` 布局。
- 构建镜像安装 `librime-dev` 只作为编译来源；打包阶段会把构建镜像里的 librime runtime 闭包复制进 KeyTao runtime。用户安装 deb/rpm 后不应再依赖系统预装 `librime` 或 `opencc` 才能运行 KeyTao 输入法。
- 开发时也可以直接运行 `cargo build -p keytao-linux-ime --release`。

## 测试

`src/main.rs` 里除 `panel` 外的模块全部带 `#[cfg(target_os = "linux")]`，所以在 macOS/Windows host 上 `cargo test -p keytao-linux-ime` **不会编译**后端代码，也就看不出后端里的编译错误（`ibus_backend` 的单测曾经因此长期编译不过而无人发现）。改这个 crate 后必须在 Linux 上跑一次：

```bash
# Linux 机器上
cargo test -p keytao-linux-ime

# 其它 host 上用容器（镜像里要有 rust、librime-dev、libxkbcommon-dev、libdbus-1-dev、freetype 依赖）
docker run --rm -v "$PWD":/app -w /app -e CARGO_TARGET_DIR=/target <linux-rust-image> \
  cargo test -p keytao-linux-ime
```

CI 也应该有一条 Linux target 的 `cargo test -p keytao-linux-ime`，否则后端代码只有发版构建时才第一次被编译。

## 排查入口

- 日志：`$XDG_STATE_HOME/keytao/log/keytao-ime.log`（默认 `~/.local/state/keytao/log/keytao-ime.log`）。提交文本、preedit 和 keysym 只在 `debug`/`trace` 级输出，默认 filter 是 `info`，需要时用 `RUST_LOG=keytao_ime=trace` 打开。
- App 调试日志聚合：`read_debug_logs`（先读上面的状态目录，再兼容旧的 `/tmp`）
- App 状态诊断：`linux_ime_status` 返回 `daemon_owner_pid`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`
- 进程：`pgrep -af keytao-ime`
- KDE：`kwriteconfig6 --file kwinrc --group Wayland --key InputMethod ...`
- Wayland：`WAYLAND_DISPLAY`、`WAYLAND_SOCKET`
- X11：`DISPLAY`、`XMODIFIERS=@im=keytao`
- IBus shim：`~/.config/ibus/bus/*`
- IBus component：`/usr/share/ibus/component/keytao.xml`、`ibus list-engine | grep keytao`
- 部署重载：`~/.local/share/keytao/keytao-ime.reload`
