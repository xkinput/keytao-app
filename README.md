# 键道

键道输入方案与配套工具，基于 Tauri 构建。主 App 负责下载、安装、合并和部署 Rime 方案；桌面系统输入法前端负责把系统按键送进同一套 librime 核心，并用平台原生接口提交文本、显示预编辑和候选。

各平台系统输入法的具体实现分别见：

- [Linux IME](crates/keytao-linux-ime/IMPL.md)
- [macOS IME](crates/keytao-macos-ime/IMPL.md)
- [Windows IME](crates/keytao-windows-ime/IMPL.md)

## 工作逻辑

1. App 获取最新方案包，安装到当前平台的 KeyTao 用户数据目录。
2. 安装时智能合并 `default.custom.yaml` 和 `rime.lua`，保留用户非 KeyTao schema 与自定义 Lua module。
3. App 调用 librime 部署，把 schema、dict、Lua、OpenCC 等资源编译到用户目录。
4. 系统输入法进程启动后读取同一个用户目录，并创建独立 Rime session。
5. 平台输入法把按键转换成 librime keysym/modifier，调用 `process_key_result`。
6. librime 返回统一的 `ImeState`：`committed` 用平台原生接口提交，`preedit` 用平台 composition/marked-text 接口更新，`candidates` 由平台候选窗口展示。
7. 部署后 Linux daemon 会收到 reload stamp；macOS 和 Windows 当前主要依靠重新部署和重新激活输入法进程刷新。

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
pnpm tauri build
```

Linux 下如果要让 Tauri 包内嵌 `keytao-ime` sidecar：

```bash
cargo build -p keytao-linux-ime --release
target_triple="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p src-tauri/binaries
cp target/release/keytao-ime "src-tauri/binaries/keytao-ime-${target_triple}"
chmod +x "src-tauri/binaries/keytao-ime-${target_triple}"
KEYTAO_IME_PATH="$PWD/target/release/keytao-ime" pnpm tauri build --bundles deb,rpm --config src-tauri/tauri.linux.conf.json
```
