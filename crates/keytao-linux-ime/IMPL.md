# Linux IME 实现说明

本文只记录 `crates/keytao-linux-ime` 里的 Linux 系统输入法 daemon 实现，并按当前代码同步。

## 代码地图

- `src/main.rs`：进程入口、日志轮转、单实例控制、reload watcher、后端选择、托盘启动。
- `src/engine.rs`：共享 librime 初始化、每个输入上下文的 `ImeSession`、reload generation。
- `src/wayland_backend.rs`：wlroots/非 KDE Wayland，使用 `zwp_input_method_v2`。
- `src/wayland_backend_kde.rs`：KDE/KWin Wayland，使用 KWin 私有启动的 `input-method-v1`。
- `src/gnome_ibus_engine.rs`：GNOME IBus engine，连接现有 `ibus-daemon`。
- `src/ibus_backend.rs`：IBus 兼容 D-Bus shim，服务 Chromium/CEF/Electron 等 IBus 客户端。
- `src/x11_backend.rs`：X11 XIM server，注册为 `@im=keytao`。
- `src/panel.rs`：候选窗和模式提示的 BGRA 像素渲染。
- `src/kimpanel.rs`：KDE Kimpanel/impanel2 D-Bus 候选服务。
- `src/tray.rs`：Linux 托盘菜单，打开 `keytao-app` 或退出 daemon。

Tauri 主 App 不直接处理 Linux 系统输入法按键。它负责安装方案、部署方案、启动/重启 `keytao-ime`，并在部署后写 `keytao-ime.reload` 通知 daemon 重载。

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

`CoreEngine` 的行为：

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
8. 有 preedit 时按 Enter 会直接提交当前 preedit，然后 reset session。
9. 空格在有候选时优先选中当前高亮候选。
10. IBus engine 和 IBus shim 会根据 `select_keys` 把数字/选择键转成候选索引；Wayland/XIM 目前主要让 librime 自己处理非空格选择键。
11. `accepted=false` 时放行或转发按键。
12. 有 `committed` 时用当前后端原生接口提交。
13. 有 `preedit` 时用当前后端原生接口更新客户端预编辑。
14. 有 `candidates` 时更新候选窗或候选服务。

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
- 中英模式变化时显示 3 秒 `英`/`中` 模式提示。
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
- 当前只记录 ascii mode 变化日志，没有像 input-method-v2 那样显示模式提示浮窗。

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

已实现的 UI 信号包括 `CommitText`、`UpdatePreeditText`、`UpdateLookupTable`、show/hide preedit 和 lookup table。`page_up`、`page_down`、`cursor_up`、`cursor_down`、`candidate_clicked` 当前是空方法。

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

`page_up`、`page_down`、`cursor_up`、`cursor_down`、`candidate_clicked`、property activate/show/hide 当前也是空方法。

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
- 默认视觉为固定暗色主题。
- 正文优先使用 `KEYTAO_IME_FONT`，否则通过 fontconfig 查找中文字体，再尝试常见 CJK 字体路径。
- 符号/emoji 优先使用 `KEYTAO_IME_SYMBOL_FONT`，否则查找 Noto Symbols/Emoji。
- 缩放读取 `KEYTAO_IME_PANEL_SCALE`、`GDK_SCALE`、`QT_SCALE_FACTOR`、`QT_SCREEN_SCALE_FACTORS`；X11 还会读取 `xrdb -query` 的 `Xft.dpi`。
- `render_mode_hint()` 渲染 `英`/`中` 模式提示，目前由 input-method-v2 使用。

## App 对接点

Tauri 主 App 的 Linux 相关命令在 `src-tauri/src/lib.rs`：

- `linux_ime_status`
- `linux_start_ime`
- `linux_restart_ime`
- `linux_enable_kde_support`

App 启动时也会尝试启动 fallback `keytao-ime`。KDE 原生 Wayland 由 `linux_enable_kde_support` 写入：

```text
~/.local/share/applications/keytao-wayland-launcher.desktop
kwinrc [Wayland] InputMethod=keytao-wayland-launcher.desktop
```

普通 fallback daemon 和 KWin 私有进程是两个角色：前者服务 XIM/IBus，后者服务 KDE 原生 Wayland。

## 构建

- `scripts/build-linux.sh` 通过 Docker builder 生成 Linux 包。
- `scripts/container-build.sh` 在容器里构建 Tauri 包和 `keytao-ime`。
- 开发时也可以直接运行 `cargo build -p keytao-linux-ime --release`。

## 排查入口

- 日志：`/tmp/keytao-ime.log`
- App 调试日志聚合：`read_debug_logs`
- 进程：`pgrep -af keytao-ime`
- KDE：`kwriteconfig6 --file kwinrc --group Wayland --key InputMethod ...`
- Wayland：`WAYLAND_DISPLAY`、`WAYLAND_SOCKET`
- X11：`DISPLAY`、`XMODIFIERS=@im=keytao`
- IBus shim：`~/.config/ibus/bus/*`
- 部署重载：`~/.local/share/keytao/keytao-ime.reload`
