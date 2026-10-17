# 键道 KeyTao

键道输入方案的安装工具与各平台系统输入法：App 负责下载、安装、合并和部署 Rime 方案，系统输入法通过同一套 librime 核心输入。

| 平台 | 系统输入法形态 | Release 产物 |
| --- | --- | --- |
| Linux | `keytao-ime` daemon（Wayland、KDE、GNOME IBus、XIM、IBus） | `keytao-app-<version>-linux-{x64,arm64}.{deb,rpm}` |
| macOS | IMKit，`/Library/Input Methods/KeyTao.app` | `keytao-app-<version>-macos.pkg`（universal） |
| Windows | TSF TIP（实验性） | `keytao-app-<version>-windows-x64-setup.exe` |
| Android | `InputMethodService` | `keytao-app-<version>-android-{arm,arm64,x86_64}-release.apk` |
| iOS / iPadOS | `UIInputViewController` 键盘扩展 | `keytao-app-<version>-ios-arm64-unsigned.ipa` |

## 下载与安装

从 [Releases](https://github.com/xkinput/keytao-app/releases) 下载对应平台的安装包。

- Linux：见 [docs/linux-install.md](docs/linux-install.md)。
- macOS：安装 pkg，按安装器提示注销并重新登录，让系统发现输入法。
- Windows：运行 setup.exe，安装包说明见 [packaging/windows/README.md](packaging/windows/README.md)。
- Android：安装 APK；首次打开会先进入系统输入法启用/切换引导，KeyTao 启用并选中后进入主界面。
- iOS / iPadOS：IPA 未签名，签名与安装见 [docs/ios-install.md](docs/ios-install.md)。

## 使用

在 App 中安装方案，再点击“部署”。安装只写文件，部署才让 librime 编译并加载新配置。

附加方案 Easy English 随 App 离线提供，安装后与键道方案一起部署。

| 平台 | 用户数据目录 |
| --- | --- |
| Linux | `$XDG_DATA_HOME/keytao`，通常是 `~/.local/share/keytao` |
| macOS | `~/Library/keytao` |
| Windows | `%APPDATA%/keytao` |
| Android | `/storage/emulated/0/keytao` |
| iOS / iPadOS | App Group `group.ink.rea.keytao-app` 容器下的 `keytao` |

## 架构与目录

系统输入法由操作系统加载并运行在独立进程中，必须用各平台原生接口实现；Flutter 只承担 App 界面。两者共用 `keytao-core` 和同一用户目录，部署后通过用户目录下的 reload stamp 刷新输入法 session。通用契约见 [docs/ime-common-layer.md](docs/ime-common-layer.md)。

```text
flutter_app/                 Flutter App（flutter_rust_bridge 配置：flutter_rust_bridge.yaml）
flutter_app/android/app/     Android InputMethodService
crates/keytao-app-core/      App 业务逻辑：方案下载、安装合并、部署、输入法状态
crates/keytao-app-bridge/    flutter_rust_bridge 桥接层
crates/keytao-core/          librime setup、deploy、session、ImeState
crates/keytao-core-ffi/      给 Swift 前端的 per-session C ABI
crates/keytao-theme/         theme.yaml 解析与共享 UI model
crates/librime-sys/          librime 绑定，经 [patch.crates-io] 引入，不是 workspace 成员
crates/keytao-linux-ime/     Linux daemon
crates/keytao-macos-ime/     macOS IMKit bundle（Swift）
crates/keytao-windows-ime/   Windows TSF DLL
crates/keytao-ios-ime/       iOS 键盘扩展（Swift）
packaging/                   Linux / Windows 打包
scripts/                     构建、打包、校验脚本
```

## 开发环境

Linux：`nix develop`，或用 direnv 自动加载（`.envrc` 为 `use flake`）：

```bash
direnv allow
```

macOS：flake devShell 目前无法在 macOS 求值，使用 vendored librime：

```bash
scripts/fetch-librime.sh --platform macos
export RIME_INCLUDE_DIR=$PWD/vendor/librime/macos-universal/include
export RIME_LIB_DIR=$PWD/vendor/librime/macos-universal/lib
```

运行 App：

```bash
cd flutter_app
flutter pub get
flutter run
```

Android 需要 Android NDK，并为目标 ABI 导入 librime runtime（版本默认 latest）：

```bash
scripts/android-librime-runtime.sh import-fcitx5-rime --abi arm64-v8a
# Or import a self-built SDK:
scripts/android-librime-runtime.sh import-sdk --abi arm64-v8a --source /path/to/android-librime-sdk
# Rust environment for one ABI:
source <(scripts/android-librime-runtime.sh env --abi arm64-v8a)
cargo check -p keytao-core --target aarch64-linux-android
```

iOS 需要先构建并导入 librime：

```bash
scripts/build-ios-librime.sh --target aarch64-apple-ios
```

## 测试

```bash
cargo test
# On macOS:
DYLD_FALLBACK_LIBRARY_PATH=$RIME_LIB_DIR cargo test
```

`RIME_INCLUDE_DIR` / `RIME_LIB_DIR` 按上一节设置。`keytao-linux-ime` 和 `keytao-windows-ime` 按 `cfg(target_os)` 隔离，只在对应系统上编译实际代码。

- `scripts/check-core-cross.sh`：在 macOS 上用 zig 对 `keytao-app-core` 和 `keytao_app_bridge` 做 Linux / Windows 交叉类型检查，需要 Nix 和 `vendor/librime/{macos-universal,windows-x64}/include`。
- `crates/keytao-ios-ime/test-*.sh`：在宿主工具链上跑 iOS 键盘的 Swift 单元测试；`scripts/verify-ios-ime.sh` 对 iOS 键盘源码做 SDK 类型检查并校验工程与 runtime 布局。
- `ruby scripts/check-release-workflow.rb`：离线检查 Release 工作流契约。
- `scripts/make-android-test-fixture.sh`：重新生成 Android 仪器化测试用的方案 fixture（`flutter_app/android/app/src/androidTest/assets/keytao-schema-fixture.zip`）。

## 打包构建

| 平台 | 命令 | 产物 |
| --- | --- | --- |
| Linux | `scripts/build-linux.sh`（需要 Docker，按 builder 原生架构构建） | `dist/keytao-app-<version>-linux-{x64,arm64}.{deb,rpm}` |
| macOS | `scripts/build-macos.sh` | `target/keytao-macos-pkg/keytao-app-<version>-macos.pkg` |
| Windows | `powershell -ExecutionPolicy Bypass -File scripts\build-windows-flutter.ps1 -Arch x64` | `target\release\bundle\nsis\keytao-app-<version>-windows-x64-setup.exe` |
| Android | `cd flutter_app && flutter build apk --release --split-per-abi --target-platform android-arm,android-arm64,android-x64` | `flutter_app/build/app/outputs/flutter-apk/app-<abi>-release.apk` |
| iOS | `scripts/build-ios-librime.sh --target aarch64-apple-ios && scripts/build-flutter-ios.sh` | `flutter_app/build/ios/ipa/keytao-app-<version>-ios-arm64-unsigned.ipa` |

各平台发行包都自带 librime、OpenCC 数据、`rime-plugins` 和基础 `rime-data`，App 与系统输入法使用同一套 runtime。平台细节：macOS 见 [crates/keytao-macos-ime/README.md](crates/keytao-macos-ime/README.md#构建与排查)，Linux 见 [packaging/linux/README.md](packaging/linux/README.md)，Windows 见 [packaging/windows/README.md](packaging/windows/README.md)。

## 发版流程

版本唯一来源是根目录 `Cargo.toml` 的 `[workspace.package].version`。

1. `node scripts/sync-version.mjs --set <version>`，同步更新 `Cargo.toml` 和 `flutter_app/pubspec.yaml`，然后提交。
2. 可选预跑：在分支上手动触发 Release 工作流（`workflow_dispatch`），构建全部平台并运行 `build-nix`，不创建 Release。
3. 打 tag `v<version>` 并推送，`release.yml` 构建并发布 Release；tag 含 `alpha` 时标记为 prerelease。
4. 附件发布后，在已安装 Nix 和 Python 3 的环境执行 `scripts/update-nix-release.sh <version>`，提交 `flake.nix`。

`update-nix-release.sh` 用 `nix store prefetch-file` 获取 x64 / arm64 两份 deb 的 SHA-256，全部成功后才更新 `flake.nix` 的版本与哈希；不更新 `flake.lock`，不自动提交。`build-nix` 只在 `workflow_dispatch` 时运行，构建 `flake.nix` 固定的已发布 deb 并执行 `nix flake check --no-build`；tag 发布不运行该 job。

## 平台文档索引

- 用户与打包：[Linux 安装](docs/linux-install.md)、[iOS 安装](docs/ios-install.md)、[Linux 打包](packaging/linux/README.md)、[Windows 打包](packaging/windows/README.md)
- 平台概览：[Linux IME](crates/keytao-linux-ime/README.md)、[macOS IME](crates/keytao-macos-ime/README.md)、[Windows IME](crates/keytao-windows-ime/README.md)、[主题](crates/keytao-theme/README.md)
- 实现说明：[通用层](docs/ime-common-layer.md)、[Linux](crates/keytao-linux-ime/IMPL.md)、[macOS](crates/keytao-macos-ime/IMPL.md)、[Windows](crates/keytao-windows-ime/IMPL.md)、[Android](flutter_app/android/app/IMPL.md)、[iOS](crates/keytao-ios-ime/IMPL.md)

## 许可证

本项目以 [MIT](LICENSE) 许可证发布。第三方组件：

- librime：BSD-3-Clause，见 [crates/librime-sys/LICENSE-librime](crates/librime-sys/LICENSE-librime)。
- Easy English 附加方案：LGPL-3.0，来源与许可证见 [NOTICE](resources/addon-schemas/easy_en/NOTICE.md) 和 [LICENSE](resources/addon-schemas/easy_en/LICENSE)。
