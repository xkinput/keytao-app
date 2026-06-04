# Linux 安装

KeyTao Linux 版本包含两个组件：

- `keytao-app`：图形安装器，负责下载、合并、部署键道 Rime 方案。
- `keytao-ime`：系统输入法 daemon，负责 Wayland、XIM、IBus 输入前端。

从 GitHub Release 下载的 Linux deb/rpm 包会包含完整 `keytao-ime`，不需要额外单独安装输入法二进制。

## 标准 Linux 安装

从 [Releases](https://github.com/xkinput/keytao-app/releases) 下载适合发行版的包。

### Debian / Ubuntu

```bash
sudo apt install ./KeyTao_*_amd64.deb
```

### Fedora / openSUSE / RHEL

```bash
sudo dnf install ./KeyTao-*.x86_64.rpm
```

没有 `dnf` 的发行版可以直接用 rpm：

```bash
sudo rpm -Uvh ./KeyTao-*.x86_64.rpm
```

deb/rpm 安装的是一个完整包：图形 app 和内置的 `keytao-ime` 会一起安装。用户正常从桌面菜单启动 KeyTao，不需要单独下载或手动安装 `keytao-ime`。app 会从包内资源解析 `keytao-ime`；点击“启动 XIM+IBUS”或“启用 KDE 支持”时，会使用这个随包安装的 daemon。

首次启动后，在 app 的“安装”页中：

1. 安装或更新键道方案。
2. 点击“部署”。
3. 在“Linux 系统输入法”卡片中检查或启动 `keytao-ime`。
4. KDE 用户点击“启用 KDE 支持”，然后重新登录或重启 KWin 会话让 Virtual Keyboard 配置生效。

## Nix / NixOS 安装

本项目提供 flake package 和 NixOS module。

### 添加 flake input

```nix
inputs.keytao-app = {
  url = "github:xkinput/keytao-app";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

### NixOS module

```nix
{
  imports = [
    inputs.keytao-app.nixosModules.default
  ];

  services.keytao-app.enable = true;
}
```

module 会把 `keytao-app` 和 `keytao-ime` 加入系统环境，并设置基础 `XMODIFIERS=@im=keytao`。

### 只安装 package

```nix
{ pkgs, inputs, ... }:
{
  environment.systemPackages = [
    inputs.keytao-app.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

Home Manager 用户也可以放到 `home.packages`。

## KDE Plasma

KDE Plasma Wayland 的原生输入法路径由 KWin Virtual Keyboard 启动。普通应用里手动运行 `keytao-ime` 只能启动 XIM/IBus fallback，不能替代 KWin 的私有 `WAYLAND_SOCKET` 实例。

标准包用户可以在 app 中点击“启用 KDE 支持”。它会写入：

- `~/.local/share/applications/keytao-wayland-launcher.desktop`
- `~/.config/kwinrc` 的 `Wayland/InputMethod=keytao-wayland-launcher.desktop`

NixOS / Home Manager 可以用可复现配置：

```nix
{ pkgs, lib, inputs, ... }:

let
  keytaoPackage = inputs.keytao-app.packages.${pkgs.stdenv.hostPlatform.system}.default;
  kdeVirtualKeyboardDesktop = "keytao-wayland-launcher.desktop";
in
{
  home.packages = [ keytaoPackage ];

  home.sessionVariables.XMODIFIERS = "@im=keytao";
  systemd.user.sessionVariables.XMODIFIERS = "@im=keytao";

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

  xdg.configFile."autostart/keytao-ime.desktop".text = ''
    [Desktop Entry]
    Name=KeyTao IME Daemon
    Exec=${keytaoPackage}/bin/keytao-ime --backend=xim,ibus
    Type=Application
    NoDisplay=true
    X-KDE-autostart-phase=1
  '';

  home.activation.configureKeytaoKdeVirtualKeyboard =
    lib.hm.dag.entryAfter [ "writeBoundary" ] ''
      if [ -x "${pkgs.kdePackages.kconfig}/bin/kwriteconfig6" ]; then
        "${pkgs.kdePackages.kconfig}/bin/kwriteconfig6" \
          --file "$HOME/.config/kwinrc" \
          --group Wayland \
          --key InputMethod \
          "${kdeVirtualKeyboardDesktop}"
      fi
    '';
}
```

## QQ / WeChat

QQ 和微信在很多 Wayland 会话中运行于 XWayland，需要走 `keytao-ime` 的 `XIM+IBUS` 进程，而不是只依赖 `KWIN_WAYLAND`。

启动前先确认 `XIM+IBUS` 已经运行：

```bash
pgrep -a keytao-ime
```

app 中应显示 `XIM+IBUS 1`。如果没有，点击“启动 XIM+IBUS”。

### 标准 Linux wrapper

不要直接从桌面菜单启动原始 QQ / WeChat。用 wrapper 固定环境变量：

```bash
#!/usr/bin/env bash
set -euo pipefail

unset WAYLAND_DISPLAY
export DISPLAY="${DISPLAY:-:0}"
export QT_QPA_PLATFORM=xcb
export GDK_BACKEND=x11
export XMODIFIERS="@im=keytao"
export QT_IM_MODULE=ibus
export GTK_IM_MODULE=ibus
export IBUS_ADDRESS="${IBUS_ADDRESS:-${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}}"

exec qq --ozone-platform-hint=x11 "$@"
```

微信可以把最后一行换成：

```bash
exec wechat "$@"
```

这些变量的作用：

- `unset WAYLAND_DISPLAY`：强制目标应用走 XWayland。
- `QT_QPA_PLATFORM=xcb` / `GDK_BACKEND=x11`：避免 Qt/GTK 误走 Wayland。
- `XMODIFIERS=@im=keytao`：提供 XIM 标识。
- `QT_IM_MODULE=ibus` / `GTK_IM_MODULE=ibus`：让 Qt/GTK/Electron 走 IBus。
- `IBUS_ADDRESS=...`：把 IBus D-Bus 地址指向当前用户 session bus。

### GTK IBus immodule

如果 QQ / WeChat 仍然没有任何 `IBus ProcessKeyEvent` 日志，通常是 GTK/Electron 看不到 IBus immodule。此时 wrapper 还需要提供：

```bash
IBUS_SO="/usr/lib/gtk-3.0/3.0.0/immodules/im-ibus.so"
mkdir -p "$HOME/.cache"
gtk-query-immodules-3.0 "$IBUS_SO" > "$HOME/.cache/keytao-gtk-immodules.cache"

export GTK_PATH="$(dirname "$(dirname "$IBUS_SO")")${GTK_PATH:+:$GTK_PATH}"
export GTK_IM_MODULE_FILE="$HOME/.cache/keytao-gtk-immodules.cache"
```

不同发行版的 `im-ibus.so` 路径可能不同，可以用下面命令查找：

```bash
find /usr /lib /lib64 -path '*gtk-3.0*immodules*im-ibus.so' 2>/dev/null | head -1
```

### NixOS / Home Manager wrapper

NixOS 上推荐在 derivation 中预生成 `GTK_IM_MODULE_FILE`，再包装 QQ / WeChat：

```nix
{ pkgs, lib, ... }:

let
  gtkIbusImModulesCache = pkgs.runCommand "keytao-gtk-immodules.cache" { } ''
    ${pkgs.gtk3.dev}/bin/gtk-query-immodules-3.0 \
      ${pkgs.ibus}/lib/gtk-3.0/3.0.0/immodules/im-ibus.so > "$out"
  '';
in
{
  home.packages = [
    (lib.hiPrio (pkgs.writeShellScriptBin "qq" ''
      unset WAYLAND_DISPLAY
      export DISPLAY="''${DISPLAY:-:0}"
      export GDK_BACKEND=x11
      export QT_QPA_PLATFORM=xcb
      export XMODIFIERS="@im=keytao"
      export QT_IM_MODULE=ibus
      export GTK_IM_MODULE=ibus
      export IBUS_ADDRESS="''${IBUS_ADDRESS:-''${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}}"
      export GTK_PATH="${pkgs.ibus}/lib/gtk-3.0/3.0.0''${GTK_PATH:+:$GTK_PATH}"
      export GTK_IM_MODULE_FILE="${gtkIbusImModulesCache}"
      export ELECTRON_OZONE_PLATFORM_HINT=x11
      export NIXOS_OZONE_WL=0

      exec ${pkgs.qq}/bin/qq --ozone-platform-hint=x11 "$@"
    ''))

    (lib.hiPrio (pkgs.writeShellScriptBin "wechat" ''
      unset WAYLAND_DISPLAY
      export DISPLAY="''${DISPLAY:-:0}"
      export QT_QPA_PLATFORM=xcb
      export GDK_BACKEND=x11
      export XMODIFIERS="@im=keytao"
      export QT_IM_MODULE=ibus
      export GTK_IM_MODULE=ibus
      export IBUS_ADDRESS="''${IBUS_ADDRESS:-''${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}}"
      export GTK_PATH="${pkgs.ibus}/lib/gtk-3.0/3.0.0''${GTK_PATH:+:$GTK_PATH}"
      export GTK_IM_MODULE_FILE="${gtkIbusImModulesCache}"

      exec ${pkgs.wechat}/bin/wechat "$@"
    ''))
  ];
}
```

微信 AppImage 版本可能带有自己的 GTK runtime，仅 wrapper 外层变量还不够。更稳妥的 Nix 做法是用 `pkgs.appimageTools.wrapAppImage`，并在 `extraBuildCommands` 中把 `im-ibus.so` 追加进 AppImage 内部的 `immodules.cache`。

完整实现可参考本地 Nix 配置中的 `wechat-keytao-input` / `keytaoWechat` 封装思路。

## 验证

```bash
pgrep -a keytao-ime
tail -f /tmp/keytao-ime.log
```

关键日志：

- KDE 原生：`KWin Virtual Keyboard mode`、`KDE input-method-v1 context activated`
- XIM：`X11 XIM server running`、`XIM CreateIC`
- IBus：`IBus D-Bus backend started`、`IBus ProcessKeyEvent`
