# 键道

键道输入方案与配套工具，基于 Tauri 构建。主 App 负责下载、安装、合并和部署 Rime 方案；桌面系统输入法前端负责把系统按键送进同一套 librime 核心，并用平台原生接口提交文本、显示预编辑和候选。

各平台系统输入法的具体实现分别见：

- [输入法通用层实现规范](docs/ime-common-layer.md)
- [Linux IME](crates/keytao-linux-ime/IMPL.md)
- [macOS IME](crates/keytao-macos-ime/IMPL.md)
- [Windows IME](crates/keytao-windows-ime/IMPL.md)

## 工作逻辑

1. App 获取最新方案包，安装到当前平台的 KeyTao 用户数据目录。
2. 安装时智能合并 `default.custom.yaml` 和 `rime.lua`，保留用户非 KeyTao schema 与自定义 Lua module。
3. App 调用通用 deploy 能力，把 schema、dict、Lua、OpenCC 等资源编译到用户目录。
4. 系统输入法进程启动后读取同一个用户目录，并通过 `ImeRuntime` 创建独立 session。
5. 平台输入法把按键转换成 X11 keysym + Rime modifier mask，调用 `ImeRuntimeSession::process_key_result` 或 FFI per-session API。
6. librime 返回统一的 `ImeState`：`committed` 用平台原生接口提交，`preedit` 用平台 composition/marked-text 接口更新，`candidates` 由平台候选窗口展示。
7. 部署后 Linux daemon、macOS IMK 和 Windows TSF 都会通过用户目录下的 reload stamp 刷新。

## 输入法架构

桌面输入法按“通用 runtime + 平台 adapter”拆分：

- `keytao-core` 负责 librime setup、deploy、session、reload generation、modifier mask 和 `ImeState` 抽取。
- `keytao-core-ffi` 给 macOS 等非 Rust 前端暴露 per-session C ABI。
- Linux/macOS/Windows 平台层只负责系统输入法协议、原生 key event 转换、commit/preedit/candidate UI 和诊断。

这样做的好处是：librime 调度只实现一次，词库重新部署和 session 刷新有统一入口，平台接入更薄；`theme.yaml` 由 `crates/keytao-theme` 解析成共享主题和 UI model，再由各平台 renderer 映射到自己的窗口或系统候选服务。

## 主要能力

- 自动获取最新键道方案并下载安装
- 智能合并 `default.custom.yaml` 和 `rime.lua`
- 自动检测 Rime 配置目录，也可手动选择
- 安装进度、部署状态、调试日志实时展示
- Linux 版本内置完整 `keytao-ime` 系统输入法 daemon
- macOS 版本包含实验性 IMKit 系统输入法 bundle
- Windows 版本包含实验性 TSF 系统输入法 DLL

## 平台状态

| 平台 | Rime 方案安装 | 系统输入法 |
| --- | --- | --- |
| Linux | 已支持 | 已支持，`keytao-ime` daemon 覆盖 Wayland、KDE、GNOME IBus、XIM、IBus 兼容路径 |
| macOS | 已支持 | 实验性支持，基于 IMKit，安装到 `/Library/Input Methods/KeyTao.app` |
| Windows | 已支持 | 实验性支持，基于 TSF TIP，注册 `keytao_windows_ime.dll` |
| Android | 已支持 | 暂无系统 IME，当前是方案安装/合并工具 |
| iOS | 手动导入 | 暂无系统键盘 extension |

## 数据与部署

桌面系统输入法共用 `keytao-core`：

- macOS 用户目录：`~/Library/keytao`
- Windows 用户目录：`%APPDATA%/keytao`
- Linux 用户目录：`$XDG_DATA_HOME/keytao`，通常是 `~/.local/share/keytao`

App 的“安装方案”只负责写文件；“部署”才会让 librime 编译并加载新配置。`rime.lua` 是否生效，取决于它是否安装到了系统输入法实际使用的用户目录，并且是否完成部署。

## 下载

前往 [Releases](https://github.com/xkinput/keytao-app/releases) 下载对应平台的安装包。

Linux 安装方式见 [docs/linux-install.md](docs/linux-install.md)。

## 发行打包

KeyTao 是系统输入法，不按普通桌面小工具的分发方式处理：

- macOS 只构建 `pkg`。pkg 同时安装 `/Applications/KeyTao.app` 和 `/Library/Input Methods/KeyTao.app`，不构建 dmg。
- Linux 只构建 `deb` 和 `rpm`，不构建 AppImage 或 tarball。deb/rpm 同时安装图形 App、`keytao-ime` 和包内 runtime，保证可以作为系统输入法安装。
- Windows 只构建 NSIS `.exe` 安装包，并把 TSF 输入法 DLL 与 librime runtime 放进稳定的 `keytao-windows-ime-runtime/x64` 资源目录。
- macOS、Linux 和 Windows 发行包都应自带完整 Rime runtime：`librime`、OpenCC 数据、`rime-plugins` 和基础 `rime-data`。主 App 与系统 IME 使用同一套包内 runtime，避免 Lua 方案在 App 部署时可用、到 IME 进程里不可用。

### 通用准备

```bash
pnpm install
```

如需同步版本号，先改 `package.json` 的版本，再执行：

```bash
pnpm sync-version
```

### macOS

完整发行包从仓库根目录构建：

```bash
pnpm build:macos
scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg
```

产物：

- `target/keytao-macos-pkg/KeyTao.pkg`

本机安装测试需要管理员权限，安装动作单独执行：

```bash
sudo installer -pkg target/keytao-macos-pkg/KeyTao.pkg -target /
```

安装后快速检查：

```bash
test -d "/Applications/KeyTao.app"
test -x "/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME"
"/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME" --list-input-sources
open -a KeyTao
```

### Linux

Linux 发行包通过 Docker builder 构建，需要本机可运行 Docker：

```bash
pnpm build:linux
```

产物在 `target/release/bundle/` 下，包含：

- `deb`
- `rpm`

### Windows

Windows 需要在 Windows 开发环境中执行，推荐从 PowerShell 运行。构建机需要 MSVC Rust target、LLVM/libclang 和可用的 `pnpm`；脚本会按需下载 librime Windows SDK。

```powershell
pnpm install
pnpm build:windows
```

该命令会先构建 Windows TSF 输入法 runtime，再构建 Tauri NSIS 安装包。只构建输入法 runtime 时使用：

```powershell
pnpm build:windows-ime
```

产物通常位于：

- `target\keytao-windows-ime-runtime\x64`
- `target\release\bundle\nsis`

## 开发

推荐使用 `direnv` 自动加载 flake 开发环境：

```bash
direnv allow
```

进入仓库目录后安装依赖并启动开发环境：

```bash
pnpm install
pnpm tauri dev
```

构建：

```bash
pnpm build
```

发行包构建命令见上面的“发行打包”。
