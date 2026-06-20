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
- `src/x11_backend.rs`：X11 XIM server，注册为 `@im=keytao`。
- `src/panel.rs`：候选窗和模式提示的 BGRA 像素渲染。
- `src/kimpanel.rs`：KDE Kimpanel/impanel2 D-Bus 候选服务。
- `src/tray.rs`：Linux 托盘菜单，打开 `keytao-app` 或退出 daemon。

Tauri 主 App 不直接处理 Linux 系统输入法按键。它负责安装方案、部署方案、状态展示，并在部署后写 `keytao-ime.reload` 通知 daemon 重载；Linux 启动阶段可以尝试拉起 fallback `keytao-ime`，但正式 UI 不再提供系统输入法启动/重启按钮。

当前系统输入法维护边界以 `crates/keytao-linux-ime` 为准。旧的 `src-tauri/src/ime/linux.rs` 内嵌 Wayland IME/overlay 路径已清理，Tauri 主 App 不再保留 Linux 系统输入法前端实现。

## 跨平台前端契约

Linux daemon 要和 macOS IMK、Windows TSF 等前端共享同一套实现边界：

1. `keytao-core` 只负责 librime 初始化、部署、session、按键处理和 `ImeState` 抽取。
2. Linux daemon 负责后端选择、协议接入、按键转发、文本提交、预编辑、候选 UI、托盘和日志。
3. 每个输入上下文必须创建独立 `ImeSession`；daemon 级 `CoreEngine` 是 `keytao-core::ImeRuntime`，只管理部署、reload generation 和 session 重建。
4. 所有后端都应向 core 传 X11 keysym + Rime modifier mask；`engine::rime_modifier_mask()` 负责过滤 CapsLock、NumLock、鼠标等噪声。
5. 没有 composition 时，导航键、删除键、空格、回车、Tab、Escape、Ctrl/Alt 组合键应放行或原样转发给客户端。
6. 应用 `ImeState` 时必须按固定语义处理：`committed` 走当前协议提交，`preedit` 走当前协议预编辑，`candidates` 走候选服务或 overlay，`ascii_mode` 只驱动状态/模式提示。
7. reload 只通过 `~/.local/share/keytao/keytao-ime.reload` 触发；daemon 重载 core 后，已有 session 通过 generation 懒刷新。
8. 后端不得各自发明样式、候选 label 或分页语义；这些应来自 `ImeState` 和统一主题/布局模型。

这套契约也是后续补 Android/Windows/macOS UI 一致性时的基础：协议不同可以分叉，key event 语义、state 应用顺序和 UI 输入模型不能分叉。

## 统一 `theme.yaml` 接入方式

Linux 自绘候选窗已经接入 `crates/keytao-theme`，通过 FreeType + tiny-skia 渲染 BGRA buffer：

- Wayland input-method-v2：`PanelRenderer::render()` / `render_mode_hint()` 输出 SHM buffer，模式提示的颜色、尺寸、圆角和时长来自 `modeHint`。
- KDE Wayland：overlay panel 使用 `PanelRenderer::render()` / `render_mode_hint()`；Kimpanel/impanel2 同时收到结构化候选。
- X11 XIM：XCB overlay 使用同一个 `PanelRenderer`。
- IBus D-Bus shim：系统 lookup table + Kimpanel + 共享 X11 overlay fallback；X11 fallback 的模式提示也走 `render_mode_hint()`。
- GNOME IBus engine：IBus lookup table 负责协议兼容；当会话提供 `DISPLAY` 时，同时启动共享 X11 overlay fallback，候选窗和模式提示使用同一个 `PanelRenderer`。

Linux 把“可完全主题化”和“受系统限制”的通道分开：

| 通道 | 可主题化范围 | 限制 |
| --- | --- | --- |
| Wayland SHM popup | 完整控制颜色、字体、间距、圆角、模式提示、尺寸 | 受 compositor popup 定位约束 |
| KDE input panel overlay | 完整控制自绘 overlay | Kimpanel/impanel2 系统候选服务只能表达 label/candidate/page/cursor |
| X11 overlay | 完整控制颜色、字体、间距、圆角、尺寸 | 位置来自 XIM spot location，窗口管理器行为可能不同 |
| IBus D-Bus shim overlay | X11 fallback overlay 可主题化 | IBus lookup table 和 Kimpanel 样式由桌面环境决定 |
| GNOME IBus engine | X11 overlay fallback 可主题化；系统 lookup table 保持结构兼容 | 纯 Wayland 且无 `DISPLAY` 时只能使用 GNOME/IBus 系统候选 UI |

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
2. 初始化 `/tmp/keytao-ime.log` 日志，按天滚动并保留 3 天。
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

daemon 每秒检查一次该文件的 mtime，变化后调用 `CoreEngine::reload()`。因此已存在的输入上下文会在下一次访问 session 时自动刷新。

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
3. modifier mask 保留 Shift、Control、Alt 和 release mask；`engine::rime_modifier_mask()` 会过滤 CapsLock、NumLock、鼠标等噪声位。
4. Ctrl/Alt 组合键通常是“基键 + modifier mask”；没有 composition 时直接放行，避免截获应用快捷键。
5. 没有 composition 时，空格、回车、退格、删除、Tab、Escape、导航键等直接放行。
6. Shift 自身的按下不切换模式；Shift release 送入 Rime，用于中英模式切换。
7. Shift+字母、Shift+数字符号不是 solo Shift，必须走普通 key press 路径，让 librime 的 ASCII composer 决定提交大写或符号。
8. F4 不属于空 composition bypass；Wayland/X11/IBus 后端收到 `XK_F4` / `0xffc1` 时会送入 librime 打开 Rime schema / options 菜单。
9. 有 preedit 时按 Enter 会直接提交当前 preedit，然后 reset session。
10. 空格在有候选时优先选中当前高亮候选。
11. IBus engine 和 IBus shim 会根据 `select_keys` 把数字/选择键转成候选索引；Wayland/XIM 目前主要让 librime 自己处理非空格选择键。
12. `accepted=false` 时放行或转发按键。
13. 有 `committed` 时用当前后端原生接口提交。
14. 有 `preedit` 时用当前后端原生接口更新客户端预编辑。
15. 有 `candidates` 时更新候选窗或候选服务。

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
- `commit_string` 提交文本，`set_preedit_string` 更新预编辑，随后 `commit(serial)`。
- Ctrl+` 这类 librime 接受但应用也需要的快捷键会额外转发。
- 候选窗使用 SHM buffer 和 popup surface 绘制在光标附近。
- 中英模式变化时显示 `modeHint` 配置的 `英`/`中` 模式提示，颜色、尺寸、圆角和时长来自主题。
- `Deactivate` 有 180ms debounce，避免焦点切换瞬间误丢按键。
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
- 未消费按键通过 `context.key(serial, time, evdev_key, state)` 转发。
- modifier 通过 `context.modifiers(...)` 转发给 KWin。
- 提交使用 `context.commit_string(serial, text)`，预编辑使用 `preedit_cursor` + `preedit_string`。
- 候选状态同时更新 Kimpanel 和 input panel overlay。
- overlay panel 设置空 input region，避免候选窗拦截鼠标点击。
- 失焦有 180ms debounce，随后 reset session、清理 Kimpanel 和候选窗。
- 中英模式变化时，input panel overlay 用 `render_mode_hint()` 显示主题化模式提示，并按 `modeHint.duration` 自动隐藏。

## GNOME IBus Engine

文件：`src/gnome_ibus_engine.rs`

GNOME/mutter 不提供 `zwp_input_method_manager_v2`，所以 KeyTao 作为 IBus engine 接入 GNOME 自带 `ibus-daemon`：

1. 连接 session bus 上的 `org.freedesktop.IBus`。
2. 在 `/org/freedesktop/IBus/Factory` 暴露 `org.freedesktop.IBus.Factory`。
3. 构造 `IBusComponent` 和 `IBusEngineDesc`，调用 `RegisterComponent` 注册 `keytao` engine。
4. GNOME/IBus 调用 `CreateEngine("keytao")` 时创建一个独立 `ImeSession`。
5. `ProcessKeyEvent` 转给 `ImeSession`。
6. 用 IBus signals 提交文本、更新 preedit 和 lookup table。
7. `focus_out`、`reset`、`disable` 会 reset session 并隐藏 UI。

已实现的 UI 信号包括 `CommitText`、`UpdatePreeditText`、`UpdateLookupTable`、show/hide preedit 和 lookup table。`page_up` / `page_down` 调用 `ImeSession::change_page()`，`cursor_up` / `cursor_down` 送 `XK_Up` / `XK_Down` 给 Rime，`candidate_clicked` 调用 `select_candidate(index)` 后按统一 `ImeState` 顺序提交、清 preedit 并刷新 lookup table。

## IBus 兼容 D-Bus 后端

文件：`src/ibus_backend.rs`

这是面向 Chromium/CEF/Electron 等应用的轻量 IBus 协议实现，不依赖真实 `ibus-daemon`：

- 自己申请 `org.freedesktop.IBus`。
- 在 `/org/freedesktop/IBus` 暴露 `CreateInputContext`、engine list、global engine 等方法。
- 每个 input context 都创建独立 `ImeSession`。
- 写入 `~/.config/ibus/bus/*` 地址文件，方便 GTK/Chromium 类客户端发现当前 D-Bus 地址。
- 同时申请/服务 `org.kde.kimpanel.inputmethod`，给 Kimpanel 发候选和预编辑信号。
- 另起线程显示 X11 override-redirect overlay 候选窗，作为没有桌面候选服务时的 fallback。
- 对 Chromium/CEF，提交文本前先发送空 `UpdatePreeditText`，避免旧 preedit 区域残留或提交位置错乱。
- `SetCursorLocation` 会记录光标坐标，用于 Kimpanel 和 X11 overlay 定位。

`page_up` / `page_down`、`cursor_up` / `cursor_down` 和 `candidate_clicked` 与 GNOME IBus engine 共享同一套行为：翻页走 `change_page()`，上下移动送 `XK_Up` / `XK_Down`，点击候选走 `select_candidate(index)`。property activate/show/hide 当前仍为空方法。

## X11 XIM 后端

文件：`src/x11_backend.rs`

实现要点：

- 注册 XIM server 名称 `keytao`。
- 需要会话环境 `XMODIFIERS=@im=keytao`。
- 如果 XWayland 懒启动导致 `DISPLAY` 暂时不可连，会每秒重试。
- 初始化时读取 X11 keycode 到 keysym 的 mapping。
- `ForwardEvent` 里根据 Shift bit 在 unshifted/shifted keysym slot 中选择 keysym。
- 使用 XIM commit 提交文本。
- 候选窗口用 XCB override-redirect window 绘制，并根据 XIM spot location 定位。
- 为避免 Electron/Chromium X11 客户端 preedit 卡住，优先声明 `PREEDIT_NOTHING | STATUS_NOTHING` 和 `PREEDIT_NONE | STATUS_NONE`。
- 当前 `filter_events()` 只过滤 KeyPress；单独的 Shift release 不会走该后端送入 Rime，所以中英模式切换主要由 Wayland/IBus 路径覆盖。

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
| 重载 | watcher 看 reload stamp mtime，session generation 懒刷新 | 激活或按键时比较 reload stamp 内容 |
| 日志 | `/tmp/keytao-ime.log` 滚动日志，App 可读取 | 主要 `NSLog`，尚未纳入 App 日志采集 |
| 模式提示 | input-method-v2、KDE overlay、IBus X11 fallback 自绘；GNOME/系统候选服务受桌面环境限制 | AppKit HUD |

Linux 的复杂度主要来自桌面协议分裂，不应该让这些差异泄漏到 core 或主题配置。统一规范应把“后端能力”作为 adapter 层能力声明，而不是把 GNOME/KDE/X11 的细节写进 `theme.yaml`。

## 后续补齐顺序

建议按风险从低到高推进：

1. 把 IBus/Kimpanel 结构通道也显式标注 `UiCapabilities::system_lookup_table()`，避免误认为视觉会完全生效。
2. 继续把可复用的按键行为向 `keytao-core::key_policy` 收敛；当前 Enter、空 composition bypass、Space/`select_keys` 候选选择和 Ctrl+grave 转发已在 core 有共享实现，后续重点是补真实 Linux 桌面 golden 回归。

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
- runtime 必须包含 `librime.so.*`、OpenCC 数据、`rime-plugins`、基础 `rime-data`，以及 librime/OpenCC 需要的非系统依赖。
- `keytao-app` 和 `keytao-ime` 构建时写入 RUNPATH，覆盖 `$ORIGIN/runtime/lib`、Tauri resource runtime、deb/rpm 的 `/usr/lib/keytao-app/...` 布局。
- 构建镜像安装 `librime-dev` 只作为编译来源；打包阶段会把构建镜像里的 librime runtime 闭包复制进 KeyTao runtime。用户安装 deb/rpm 后不应再依赖系统预装 `librime` 或 `opencc` 才能运行 KeyTao 输入法。
- 开发时也可以直接运行 `cargo build -p keytao-linux-ime --release`。

## 排查入口

- 日志：`/tmp/keytao-ime.log`
- App 调试日志聚合：`read_debug_logs`
- App 状态诊断：`linux_ime_status` 返回 `daemon_owner_pid`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`
- 进程：`pgrep -af keytao-ime`
- KDE：`kwriteconfig6 --file kwinrc --group Wayland --key InputMethod ...`
- Wayland：`WAYLAND_DISPLAY`、`WAYLAND_SOCKET`
- X11：`DISPLAY`、`XMODIFIERS=@im=keytao`
- IBus shim：`~/.config/ibus/bus/*`
- 部署重载：`~/.local/share/keytao/keytao-ime.reload`
