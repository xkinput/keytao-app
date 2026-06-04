# 键道

键道输入方案与配套工具，基于 [Tauri](https://tauri.app/) 构建，支持桌面端与 Android。

## 功能

- 自动从 GitHub 获取最新键道版本并下载
- 智能合并 `default.custom.yaml`，保留用户已有的非键道方案
- 安装进度实时展示
- 自动检测 Rime 配置目录（也可手动选择）
- 安装完成后提示重新部署 Rime

## 支持平台

| 平台    | Rime 前端              | keytao-ime 前端                                          | 配置目录                   |
| ------- | ---------------------- | -------------------------------------------------------- | -------------------------- |
| macOS   | 鼠须管（Squirrel）     | 待实现                                                   | `~/Library/Rime`         |
| Windows | 小狼毫（Weasel）       | 待实现                                                   | `%APPDATA%\Rime`         |
| Linux   | Fcitx / IBus           | 已支持（Wayland input-method-v1/v2 + XIM + IBus）        | `~/.local/share/keytao`  |
| Android | 同文输入法（Trime）    | 待实现                                                   | `/sdcard/rime`（SAF）    |
| iOS     | iRime                  | 待实现                                                   | 需手动导入，仅提供下载链接 |

## 下载

前往 [Releases](https://github.com/xkinput/keytao-app/releases) 下载对应平台的安装包。

---

## Linux 安装

### deb

从 [Releases](https://github.com/xkinput/keytao-app/releases) 下载 `.deb` 包后安装：

```bash
sudo dpkg -i keytao-app_*.deb
```

### NixOS / nix-darwin（推荐）

本项目提供 Nix flake，可以用 Home Manager 或 NixOS module 直接集成：

**1. 在 `flake.nix` 中添加 input**

```nix
inputs.keytao-app = {
  url = "github:xkinput/keytao-app";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

**2. 使用 Home Manager 一句安装**

```nix
imports = [ inputs.keytao-app.homeManagerModules.default ];
```

模块会安装 GUI + `keytao-ime` daemon，并在支持 `programs.niri` 的配置中自动加入启动项和输入法环境变量。

如果只想手动安装包，也可以直接引用默认包：

```nix
# home.packages（或 environment.systemPackages）
inputs.keytao-app.packages.${pkgs.stdenv.hostPlatform.system}.default
```

---

## Linux IME 架构

Linux 下只有一个协议入口：`keytao-ime`。GUI 应用只负责下载、合并和部署 Rime 配置，并在启动时确保 `keytao-ime` 已运行。

```
keytao-app（Tauri GUI）
  └── 部署 Rime 资源并启动 keytao-ime

keytao-ime（Linux IME daemon）
  ├── Wayland frontend ──→ zwp_input_method_v2 ──→ 原生 Wayland 应用
  ├── X11 frontend ──────→ XIM ──────────────────→ X11 / XWayland 应用
  └── IBus frontend ─────→ org.freedesktop.IBus ─→ GTK / Chromium / CEF 兼容路径
                           └─ preedit / lookup table / commit signals
```

`keytao-ime` 会为每个输入上下文创建独立 librime session，避免多个应用或窗口共享同一个 composition 状态。

### Wayland（原生应用）

启动 `keytao-app` 或直接启动 `keytao-ime`。GTK 应用会通过 `text-input-v3` 协议自动连接；Electron 应用需要设置以下环境变量：

```
NIXOS_OZONE_WL=1
ELECTRON_OZONE_PLATFORM_HINT=wayland
```

### XWayland（XIM）

针对使用 X11/XCB 的应用（如微信、旧版 Qt 应用），需要在应用环境中设置 `XMODIFIERS`：

```bash
XMODIFIERS=@im=keytao <your-app>
```

#### niri（Wayland compositor）配置示例

```nix
programs.niri.settings = {
  spawn-at-startup = [
    { command = [ "keytao-app" ]; }
  ];

  environment = {
    # Electron / Chromium → native Wayland
    "NIXOS_OZONE_WL" = "1";
    "ELECTRON_OZONE_PLATFORM_HINT" = "wayland";
    # GTK: prefer Wayland, fall back to X11
    "GDK_BACKEND" = "wayland,x11";
    # Qt: prefer Wayland, fall back to xcb (XWayland)
    "QT_QPA_PLATFORM" = "wayland;xcb";
    # XWayland display — niri usually assigns :0.
    "DISPLAY" = ":0";
    "XMODIFIERS" = "@im=keytao";
    "GTK_IM_MODULE" = "xim";
    "QT_IM_MODULE" = "xim";
  };
};
```

#### KDE Plasma 配置示例

KDE Plasma 的 Wayland 输入法通过 KWin Virtual Keyboard 接口驱动，配合 XDG autostart 同时启动 IBus 后端，覆盖原生 Wayland 和 XWayland 两条路径。

```nix
let
  keytaoPackage = inputs.keytao-app.packages.${pkgs.stdenv.hostPlatform.system}.default;
  kdeVirtualKeyboardDesktop = "keytao-wayland-launcher.desktop";
in
{
  home.packages = [ keytaoPackage ];

  # XWayland apps read this for XIM
  home.sessionVariables.XMODIFIERS = "@im=keytao";
  systemd.user.sessionVariables.XMODIFIERS = "@im=keytao";

  # Register as KDE Virtual Keyboard (Wayland input-method-v2)
  xdg.dataFile."applications/${kdeVirtualKeyboardDesktop}".text = ''
    [Desktop Entry]
    Name=KeyTao Input Method (Wayland)
    Exec=${keytaoPackage}/bin/keytao-ime
    Icon=input-keyboard
    Type=Application
    NoDisplay=true
    OnlyShowIn=KDE;
    X-KDE-Wayland-VirtualKeyboard=true
  '';

  # Autostart daemon with both xim and ibus backends
  xdg.configFile."autostart/keytao-ime.desktop".text = ''
    [Desktop Entry]
    Name=KeyTao IME Daemon
    Exec=${keytaoPackage}/bin/keytao-ime --backend=xim,ibus
    Type=Application
    NoDisplay=true
    X-KDE-autostart-phase=1
  '';

  # Point KWin at keytao as the active input method
  home.activation.configureKeytaoKdeVirtualKeyboard =
    lib.hm.dag.entryAfter [ "writeBoundary" ] ''
      if [ -x "${pkgs.kdePackages.kconfig}/bin/kreadconfig6" ]; then
        "${pkgs.kdePackages.kconfig}/bin/kwriteconfig6" \
          --file "$HOME/.config/kwinrc" \
          --group Wayland \
          --key InputMethod \
          "${kdeVirtualKeyboardDesktop}"
      fi
    '';
}
```

---

### QQ / WeChat 中文输入修复

QQ 和微信在 Wayland 会话下均强制运行于 XWayland（xcb），不支持原生 Wayland 输入协议。需要通过 keytao-ime 的 **IBus 后端**桥接，而不是直接裸用 XIM。

> **为什么不能只用 XIM？**
> Chromium/Electron（QQ）和 wechat-uos 的 XIM 实现会在未完成 XIM 握手的情况下直接截获按键，导致退格、回车等普通按键在候选词状态下失效。通过 IBus D-Bus 信号通道则没有此问题。

确保 `keytao-ime --backend=xim,ibus` 已在后台运行，然后以如下环境变量启动 QQ 或微信：

```bash
unset WAYLAND_DISPLAY
export DISPLAY="${DISPLAY:-:0}"
export QT_QPA_PLATFORM=xcb
export GDK_BACKEND=x11
export XMODIFIERS="@im=keytao"
export QT_IM_MODULE=ibus
export GTK_IM_MODULE=ibus
# Forward IBus listen address from D-Bus session
export IBUS_ADDRESS="${IBUS_ADDRESS:-${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}}"

exec qq "$@"         # or: exec wechat "$@"
```

**微信额外步骤**：wechat-uos 是内嵌 GTK 的 AppImage，打包时不含 IBus immodule，需要手动注入：

```bash
# One-time: generate a cache that includes the ibus immodule
IBUS_SO=$(nix build nixpkgs#ibus --no-link --print-out-paths)/lib/gtk-3.0/3.0.0/immodules/im-ibus.so
gtk-query-immodules-3.0 "$IBUS_SO" > ~/.cache/keytao-gtk-immodules.cache

export GTK_PATH="$(dirname $(dirname $IBUS_SO))"
export GTK_IM_MODULE_FILE="$HOME/.cache/keytao-gtk-immodules.cache"
exec wechat "$@"
```

#### NixOS / Home Manager 完整可复现封装

用 Nix derivation 可以把 immodules cache 构建、wrapProgram 和 AppImage 重新打包全部固化到 Nix store，做到零运行时副作用。完整实现参考：

👉 [`nix-config/home/rea/linux.nix`](https://github.com/reaink/nix-config/blob/main/home/rea/linux.nix)

核心思路：

1. `pkgs.runCommand` 预构建 `gtkIbusImModulesCache`（`gtk-query-immodules-3.0 im-ibus.so`）
2. `makeWrapper` 给 wechat/qq 的二进制加上上述所有环境变量和 `--set GTK_IM_MODULE_FILE`
3. 微信：`pkgs.appimageTools.wrapAppImage` 在 `extraBuildCommands` 里追加 ibus 到内嵌 `immodules.cache`，确保沙箱内也能找到

---

## 开发

推荐使用 `direnv` 自动加载 flake 开发环境：

```bash
direnv allow
```

进入仓库目录后，`direnv` 会自动提供 Tauri / Android / Linux 打包所需环境变量。

```bash
pnpm install
pnpm tauri dev
```

构建：

```bash
pnpm tauri build
```

Linux 下如果要让 Tauri 包内嵌 `keytao-ime` sidecar，需要先构建 daemon 并注入 Linux-only Tauri 配置：

```bash
cargo build -p keytao-linux-ime --release
KEYTAO_IME_PATH="$PWD/target/release/keytao-ime" \
TAURI_CONFIG='{"bundle":{"externalBin":["binaries/keytao-ime"]}}' \
pnpm tauri build --bundles deb
```

Linux 打包（deb + tar.gz）：

```bash
pnpm build:linux
```

Android 构建需要配置 Android SDK 与 NDK，参考 [Tauri 移动端文档](https://tauri.app/start/prerequisites/#android)。

## 许可证

MIT
