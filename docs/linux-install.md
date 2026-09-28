# Linux 安装与使用

本文面向按[项目方 Linux 决定](app-flutter-inc2-platforms-plan.md#4-项目方决定2026-09-28)打包的 Flutter deb/rpm：包含自启动项和 KDE 启动项，退出 App 后输入法继续运行。这些是新包的约定；旧包缺少启动项时，需要升级到包含这些改动的版本。

## 1. 安装

从 [KeyTao Releases](https://github.com/xkinput/keytao-app/releases) 下载对应架构的 deb 或 rpm。x64 对应 x86_64，arm64 对应 aarch64。

**Flutter 包当前计划沿用以下 Release 文件名**；`<version>` 不带前缀 `v`，最终以 Release 附件为准。命名调整时，统一更新本表和下面两条安装命令。

| 架构 | Debian / Ubuntu | Fedora 等使用 rpm 的发行版 |
| --- | --- | --- |
| x64 | `keytao-app-<version>-linux-x64.deb` | `keytao-app-<version>-linux-x64.rpm` |
| arm64 | `keytao-app-<version>-linux-arm64.deb` | `keytao-app-<version>-linux-arm64.rpm` |

将下面的文件名替换为实际下载的文件；arm64 用户同时替换架构。

Debian / Ubuntu：

```bash
sudo apt install "./keytao-app-<version>-linux-x64.deb"
```

Fedora（使用 dnf 的系统）：

```bash
sudo dnf install "./keytao-app-<version>-linux-x64.rpm"
```

安装后包含：

- `keytao-app`：图形应用，安装、更新和部署方案，查看输入法状态。
- `keytao-ime`：系统输入法守护进程，不依赖 Fcitx5 进程。
- 包内 runtime：librime、OpenCC 数据、rime-plugins、基础 rime-data；无需另装系统 librime。
- `/usr/share/ibus/component/keytao.xml`：供 IBus 发现并按需启动 KeyTao。
- `/etc/xdg/autostart/keytao-ime.desktop`：登录后自启动，设置 `NotShowIn=GNOME;`；GNOME 由系统 IBus 启动。
- `/usr/share/applications/keytao-wayland-launcher.desktop`：供 KDE 的虚拟键盘设置选择。

包安装完成后，仍需在 App 中安装并部署键道方案，再按桌面启用输入法。

### Nix / NixOS

[flake](../flake.nix) 的默认包 `keytao-app-bin` 重新打包**官方已发布的 deb**，支持 `x86_64-linux` 和 `aarch64-linux`，包含 Flutter App、`keytao-ime` 与包内 Rime runtime，不从源码编译 App。初始固定为 `1.2.1-alpha.89`，实际版本以 `flake.nix` 的 `releaseVersion` 为准；两种架构分别校验 SHA-256，版本独立于工作区的 `Cargo.toml`。

在仓库目录运行：

```bash
nix build .#keytao-app-bin
nix run .
# Optional: install into the current user's Nix profile.
nix profile install .#keytao-app-bin
```

NixOS 配置导入 [module](../nix/nixos.nix)：

```nix
{ inputs, ... }: {
  imports = [ inputs.keytao-app.nixosModules.default ];
  services.keytao-app.enable = true;
}
```

module 默认安装 `keytao-app-bin`，将自启动项接入 `/etc/xdg/autostart`，并通过系统包提供应用菜单、KDE 虚拟键盘启动项和 IBus 描述，设置 `XMODIFIERS=@im=keytao`。桌面项和 IBus 的启动路径均指向 Nix store 中的 wrapper。普通 Nix profile 安装不会写入 `/etc`；如需登录自启动，可将包内启动项复制到用户配置目录：

```bash
nix build .#keytao-app-bin
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
cp result/etc/xdg/autostart/keytao-ime.desktop "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/"
```

更新时，先更新仓库 checkout（NixOS flake 用户更新 `keytao-app` input），然后重新构建、更新 Nix profile 或执行 `nixos-rebuild switch`。手动复制的自启动项也需重复制，以更新其中的 store 路径。

**维护者每次正式发布后**，等 x64 / arm64 两份 deb 都已上传，在可联网、已安装 Nix 和 Python 3 的环境执行：

```bash
scripts/update-nix-release.sh <version>
```

`<version>` 不带 `v`。脚本用 `nix store prefetch-file` 获取两份 deb 的 SHA-256，全部成功后才更新 `flake.nix` 的版本与两个哈希；不会更新 `flake.lock` 或自动提交。审阅变更后运行 Release 工作流的 `workflow_dispatch` 预跑：`build-nix` 构建固定的已发布 deb，并执行 `nix flake check --no-build`。它不使用 dispatch 的待发布版本参数，tag 发布也不会运行或等待该 job。只有 Linux 预跑成功才能证明实际 Nix 构建通过；GUI、登录自启动及 KDE/GNOME 输入仍需桌面验收。

仅需从源码构建输入法守护进程时，仍可使用独立包（不含 Flutter App）：

```bash
nix build .#keytao-linux-ime
```

## 2. 分桌面启用

先在终端确认当前会话：

```bash
printf '%s\n' "$XDG_CURRENT_DESKTOP" "$XDG_SESSION_TYPE"
```

### KDE Plasma：Wayland

1. 打开「系统设置 → 键盘 → 虚拟键盘」，选择 **KeyTao / 键道输入法 (Wayland)**。
2. 按下一节设置 `XMODIFIERS=@im=keytao`，供 XWayland 应用使用；不要全局设置 `GTK_IM_MODULE`、`QT_IM_MODULE`。
3. 注销后重新登录，再打开应用测试输入。

KWin 会单独启动原生 Wayland 输入进程；包的自启动项另开普通 `keytao-ime`，服务 XIM/IBus 客户端。两类进程同时存在是正常现象。终端手动运行 `keytao-ime` 不能代替 KWin 虚拟键盘进程。

列表里没有 KeyTao 时，先检查包内启动项：

```bash
ls /usr/share/applications/keytao-wayland-launcher.desktop
```

文件存在但选择未保存时，Plasma 6 可用以下命令设置，然后注销重登：

```bash
kwriteconfig6 --file "$HOME/.config/kwinrc" \
  --group Wayland --key InputMethod keytao-wayland-launcher.desktop
```

### KDE Plasma：X11

使用普通 `keytao-ime` 的 XIM/IBus 前端，无需配置 Wayland 虚拟键盘。按下一节设置 XIM 环境变量，注销重登后由自启动项运行；需要 IBus 模块的应用再单独配置。

### GNOME：Wayland / X11

两种会话都使用系统 IBus 的 KeyTao engine：

1. 安装包后注销重登，让 IBus 读取 `keytao.xml`。
2. 打开「设置 → 键盘 → 输入源 → 添加」，在「其他 / 中文」中选择 **KeyTao / 键道**；也可用 `ibus-setup` 添加。
3. 用 `Super+Space` 切换到 KeyTao，`Shift+Super+Space` 切到上一个输入源。快捷键来源：[GNOME 帮助](https://help.gnome.org/users/gnome-help/stable/keyboard-layouts.html.en)。

没有找到输入源时，在当前 GNOME 会话中检查：

```bash
ls /usr/share/ibus/component/keytao.xml
ibus restart
ibus list-engine | grep keytao
```

**保留系统 `ibus-daemon`。** GNOME 不通过 KeyTao 的通用 Wayland 后端接入，也不使用 KDE launcher。不要把下文非 GNOME 的 `IBUS_ADDRESS` 覆盖设置带进 GNOME；KeyTao 需要连接系统 IBus 的总线。

GNOME 有系统 IBus 面板时使用系统候选窗，KeyTao 的自绘主题不能完全控制其外观。

### 其他桌面

- Unity、Budgie、Pantheon、Cinnamon：代码选择 IBus engine 路径，通过桌面的输入源设置或 `ibus-setup` 选择 KeyTao，并保留 IBus。
- 其他 Wayland 会话：合成器须支持 `input-method-v2`；有 XWayland 时还会启用 XIM/IBus，纯 Wayland 会话只启用 Wayland 前端。不能仅凭“使用 Wayland”判断兼容。
- 其他 X11 会话：使用 XIM/IBus，按下一节设置环境变量。

非 GNOME 的 XDG 自启动由包提供；若窗口管理器没有执行 XDG 自启动，需在它的会话启动配置中运行 `keytao-ime`。诊断时也可在该图形会话的终端运行此命令。

## 3. 环境变量：设置什么、写在哪里

| 场景 | 设置 |
| --- | --- |
| 使用 KeyTao XIM 的 X11 / XWayland 应用 | `XMODIFIERS=@im=keytao`；前提是 daemon 已启用 XIM |
| KDE 原生 Wayland、GTK 原生 Wayland | 不全局设置 `GTK_IM_MODULE` / `QT_IM_MODULE`；不要把 X11 应用的设置扩散到整个会话 |
| 非 GNOME 的 GTK / Qt 应用需要 IBus 兼容路径 | 只为目标应用设置 `GTK_IM_MODULE=ibus` / `QT_IM_MODULE=ibus`；GTK 需要可用的 `im-ibus.so` |
| 非 GNOME 应用需要显式连接 KeyTao IBus 兼容层 | 只在应用 wrapper 中设置 `IBUS_ADDRESS` 为当前 session bus；见 QQ 示例 |
| GNOME 的正常输入源流程 | 沿用桌面管理的 IBus 环境，不照搬非 GNOME wrapper |

不需要为普通安装设置 `RIME_LIB_DIR`、`LD_LIBRARY_PATH` 或共享数据目录变量；包内 runtime 由程序定位。

选择**会被当前会话读取的一处**保存环境变量：

| 文件 | 写法与适用范围 |
| --- | --- |
| `~/.config/environment.d/keytao.conf` | `NAME=value`，不写 `export`。survey 核实的图形会话范围为 GDM 或 Plasma 5.22+；其他登录方式不能直接套用 |
| `~/.profile` | Shell 写法 `export NAME=value`，仅在登录 shell / 图形会话确实读取它时适用；survey 未核实各登录管理器读取此文件的链路，不能作为通用兜底 |
| `/etc/environment` | survey 未核实其加载范围，仓库也没有配套配置；本指南不提供通用写入步骤。需先确认发行版的会话配置规则，不能假定它与前两者等价 |

例如，适用 `environment.d` 的会话可创建 `~/.config/environment.d/keytao.conf`，内容为：

```ini
XMODIFIERS=@im=keytao
```

若已确认会话读取 `~/.profile`，则在该文件添加：

```sh
export XMODIFIERS="@im=keytao"
```

保存后**注销并重新登录**，重新启动目标应用；仅在终端 `source ~/.profile` 不会更新已运行桌面和应用的环境。新会话中可用 `printenv XMODIFIERS` 核对；若仍未继承，检查文件是否被加载，必要时重启。

环境变量机制参考 survey 已核对的 [Fcitx Setup](https://fcitx-im.org/wiki/Setup_Fcitx_5) 与 [Wayland 说明](https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland)。这里只采用通用会话机制，变量值按 KeyTao 设置。

## 4. 启动与使用

1. 从应用菜单打开 KeyTao，或在终端运行 `keytao-app`。
2. 首次打开按引导选择键道方案和版本，点击「安装方案」，等待安装完成。
3. 点击「部署方案」，确认部署完成后结束引导；随后按前面的桌面步骤启用 KeyTao。
4. 在输入框中测试：单独按下再松开 `Shift` 切换中英文；`F4` 打开 Rime 方案 / 选项菜单。
5. 更新方案或修改配置后，回到「方案」页点击「部署」，完成重新部署；「检查本地」可刷新方案状态。

部署后 App 写入重载通知，daemon 读取后加载新方案；IBus 路径中已有的预编辑内容可能到下一次按键才刷新。新包按发布决定在退出 `keytao-app` 后保留 `keytao-ime`，不必一直开着 App 窗口。

在「输入法」页查看「Linux 系统输入法」状态。KDE Wayland 重点确认 `KWIN_WAYLAND` 与 `XIM+IBUS` 两类进程；仅“运行中”不能证明所有应用的输入路径都已启用。也可检查：

```bash
pgrep -af keytao-ime
```

## 5. 数据与日志

| 内容 | 默认位置 |
| --- | --- |
| KeyTao 方案、词库和用户配置 | `~/.local/share/keytao`（设置了 `XDG_DATA_HOME` 时为 `$XDG_DATA_HOME/keytao`） |
| 输入法重载通知 | 数据目录内的 `keytao-ime.reload` |
| 输入法日志目录 | `~/.local/state/keytao/log`（设置了有效的绝对路径 `XDG_STATE_HOME` 时为 `$XDG_STATE_HOME/keytao/log`） |
| 当前输入法日志 | 日志目录内的 `keytao-ime.log`，按日滚动 |

默认路径下查看日志：

```bash
tail -f "$HOME/.local/state/keytao/log/keytao-ime.log"
```

KeyTao 的数据目录与下文手动 ibus-rime 的目录独立。不要把方案安装到 ibus-rime 目录后，期待 `keytao-ime` 自动读取。

## 6. 常见问题与避坑

### 登录后没有输入法，或只有部分应用能输入

先看进程和日志，再确认桌面启用方式及应用实际运行在 Wayland 还是 X11 / XWayland。KDE 的原生 Wayland 和 XWayland 分属不同进程；GNOME 要检查输入源是否选中了 KeyTao。

可用于定位前端的日志：`KWin Virtual Keyboard mode`、`X11 XIM server running`、`IBus D-Bus backend started`。需要详细日志时，可在非 GNOME 图形会话中运行 `RUST_LOG=keytao_ime=trace keytao-ime`；它会替换普通 daemon，仍不能代替 KWin 私有进程。

### IBus / Fcitx5 冲突

- 非 GNOME、使用 KeyTao IBus 兼容层时，不要同时启动另一套 `ibus-daemon`；它们可能争用 `org.freedesktop.IBus`。对应日志是 `IBus: failed to request IBus name`。
- 停用其他输入法的自动启动，避免 Fcitx5 等占用 Wayland 输入法槽位；`zwp_input_method_v2: Unavailable` 表示该前端不可用。KDE 虚拟键盘应选择 KeyTao。
- **GNOME 以及使用 IBus engine 的桌面是例外：保留系统 IBus。** 无需为使用 KeyTao 再运行 Fcitx5 或选择 ibus-rime。

### Chromium / Electron

survey 已核对上游的以下参数规则，但未实测 KeyTao 与这些应用的原生 Wayland 组合；以下用于排查，不代表逐应用兼容承诺：

- 应用已运行于原生 Wayland 时，需添加 `--enable-wayland-ime`。
- Chromium 可配 `--wayland-text-input-version=3`，KWin 上优先使用 `--wayland-text-input-version=1`。
- survey 所核对的 Electron 只支持 text-input-v1，不能照搬 Chromium 的 v3 参数。原生路径不通时，改走 XWayland / IBus 兼容路径。

参数依据：[Fcitx Wayland 说明](https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland)。

### QQ / 微信的 XWayland 路径

这是非 GNOME、使用 KeyTao IBus 兼容层时的应用级 wrapper。先用 `pgrep -af keytao-ime` 确认普通 daemon 已启动，再用以下脚本启动 QQ：

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

微信将最后一行改成 `exec wechat "$@"`。这些设置仅作用于该应用，不要写进全局环境文件；GNOME 应用应连接系统 IBus，不使用此 wrapper 的总线覆盖。

若 GTK/Electron 仍未连接 IBus，检查系统是否提供 GTK 的 `im-ibus.so`。模块存在但未被发现时，可在 wrapper 的 `exec` 前生成并指定缓存；先把 `IBUS_SO` 改成发行版的实际路径：

```bash
IBUS_SO="/usr/lib/gtk-3.0/3.0.0/immodules/im-ibus.so"
mkdir -p "$HOME/.cache"
gtk-query-immodules-3.0 "$IBUS_SO" > "$HOME/.cache/keytao-gtk-immodules.cache"
export GTK_PATH="$(dirname "$(dirname "$IBUS_SO")")${GTK_PATH:+:$GTK_PATH}"
export GTK_IM_MODULE_FILE="$HOME/.cache/keytao-gtk-immodules.cache"
```

## 7. 备选：手动 ibus-rime

需要使用系统 ibus-rime 时，可选择这条独立路径：

1. 通过发行版包管理器安装 `ibus-rime`，例如 Debian / Ubuntu 执行 `sudo apt install ibus-rime`，Fedora 执行 `sudo dnf install ibus-rime`。
2. 从 [键道方案 Releases](https://github.com/xkinput/KeyTao/releases) 获取 Linux 方案包（文件名含 `keytao-linux`），将方案文件放入 `~/.config/ibus/rime`。
3. 在系统 IBus 中添加并选择 Rime，重新部署方案，再通过 Rime 的方案菜单选择键道。

这条路径使用系统 IBus/Rime 和 `~/.config/ibus/rime`，不使用 `keytao-ime` 的数据目录。若从 KeyTao 独立 daemon 切换过来，应先停用其自启动及 KDE 虚拟键盘选择，避免与系统 IBus 争用。
