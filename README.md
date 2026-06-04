# 键道

键道输入方案与配套工具，基于 Tauri 构建，支持桌面端与 Android。

## 功能

- 自动获取最新键道方案并下载安装
- 智能合并 `default.custom.yaml` 和 `rime.lua`
- 自动检测 Rime 配置目录，也可手动选择
- 安装进度、部署状态、调试日志实时展示
- Linux 版本内置完整 `keytao-ime` 系统输入法 daemon

## 支持平台

| 平台 | Rime 方案安装 | 系统输入法 |
| --- | --- | --- |
| Linux | 已支持 | 已支持，包含 `keytao-ime` |
| macOS | 已支持 | 开发中，快好了 |
| Windows | 已支持 | 开发中，快好了 |
| Android | 已支持 | 开发中，快好了 |
| iOS | 手动导入 | 开发中，快好了 |

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
KEYTAO_IME_PATH="$PWD/target/release/keytao-ime" \
TAURI_CONFIG='{"bundle":{"externalBin":["binaries/keytao-ime"]}}' \
pnpm tauri build --bundles deb,rpm,appimage
```
