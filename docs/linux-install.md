# Linux 安装

KeyTao Linux 版本包含两个组件：

- `keytao-app`：图形安装器，负责下载、合并、部署键道 Rime 方案。
- `keytao-ime`：系统输入法 daemon，负责 Wayland、XIM、IBus 输入前端。

从 GitHub Release 下载的 Linux 包会包含完整 `keytao-ime`，不需要额外单独安装输入法二进制。

## 标准 Linux 安装

从 [Releases](https://github.com/xkinput/keytao-app/releases) 下载适合发行版的包。

### Debian / Ubuntu

```bash
sudo apt install ./keytao-app_*_amd64.deb
```

### Fedora / openSUSE / RHEL

```bash
sudo rpm -i ./keytao-app-*.x86_64.rpm
```

### AppImage

```bash
chmod +x ./KeyTao_*.AppImage
./KeyTao_*.AppImage
```

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

QQ 和微信在很多 Wayland 会话中运行于 XWayland，需要走 keytao-ime 的 IBus fallback，而不是只依赖 XIM。

推荐环境：

```bash
unset WAYLAND_DISPLAY
export DISPLAY="${DISPLAY:-:0}"
export QT_QPA_PLATFORM=xcb
export GDK_BACKEND=x11
export XMODIFIERS="@im=keytao"
export QT_IM_MODULE=ibus
export GTK_IM_MODULE=ibus
export IBUS_ADDRESS="${IBUS_ADDRESS:-${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}}"
```

如果 GTK/Electron 应用仍然看不到 IBus immodule，还需要让 wrapper 暴露 `GTK_PATH` 和 `GTK_IM_MODULE_FILE`。NixOS 上可以用 derivation 预生成 `gtk-query-immodules-3.0` cache，再通过 `makeWrapper` 注入到应用环境。

## 验证

```bash
pgrep -a keytao-ime
tail -f /tmp/keytao-ime.log
```

关键日志：

- KDE 原生：`KWin Virtual Keyboard mode`、`KDE input-method-v1 context activated`
- XIM：`X11 XIM server running`、`XIM CreateIC`
- IBus：`IBus D-Bus backend started`、`IBus ProcessKeyEvent`
