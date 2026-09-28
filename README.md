# 键道

键道输入方案与配套工具，主 App 基于 Flutter 和 Rust 构建，负责下载、安装、合并和部署 Rime 方案；系统输入法前端负责把系统按键送进同一套 librime 核心，并用平台原生接口提交文本、显示预编辑和候选。

各平台系统输入法的具体实现分别见：

- [输入法通用层实现规范](docs/ime-common-layer.md)
- [Linux IME](crates/keytao-linux-ime/IMPL.md)
- [macOS IME](crates/keytao-macos-ime/IMPL.md)
- [Windows IME](crates/keytao-windows-ime/IMPL.md)
- [Android IME](flutter_app/android/app/IMPL.md)
- [iOS IME](crates/keytao-ios-ime/IMPL.md)

## 工作逻辑

1. App 获取最新方案包，安装到当前平台的 KeyTao 用户数据目录。
2. 安装时智能合并 `default.custom.yaml` 和 `rime.lua`，保留用户非 KeyTao schema 与自定义 Lua module。
3. App 调用通用 deploy 能力，把 schema、dict、Lua、OpenCC 等资源编译到用户目录。
4. 系统输入法进程启动后读取同一个用户目录，并通过 `ImeRuntime` 创建独立 session。
5. 平台输入法把按键转换成 X11 keysym + Rime modifier mask，调用 `ImeRuntimeSession::process_key_result` 或 FFI per-session API。
6. librime 返回统一的 `ImeState`：`committed` 用平台原生接口提交，`preedit` 用平台 composition/marked-text 接口更新，`candidates` 由平台候选窗口展示。
7. 部署后 Linux daemon、macOS IMK、Windows TSF、Android `InputMethodService` 和 iOS 键盘扩展都会通过用户目录下的 reload stamp 刷新。

## 输入法架构

系统输入法按“通用 runtime + 平台 adapter”拆分：

- `keytao-core` 负责 librime setup、deploy、session、reload generation、modifier mask 和 `ImeState` 抽取。
- `keytao-core-ffi` 给 macOS 等非 Rust 前端暴露 per-session C ABI。
- Linux/macOS/Windows/Android/iOS 平台层只负责系统输入法协议、原生 key event 转换、commit/preedit/candidate UI 和诊断。

这样做的好处是：librime 调度只实现一次，词库重新部署和 session 刷新有统一入口，平台接入更薄；`theme.yaml` 由 `crates/keytao-theme` 解析成共享主题和 UI model，再由各平台 renderer 映射到自己的窗口或系统候选服务。

## 主要能力

- 自动获取最新键道方案并下载安装
- 智能合并 `default.custom.yaml` 和 `rime.lua`
- 可选安装 Easy English 附加方案；移动端安装并部署后可把“英文模式”切换为完整英文词库
- 自动检测 Rime 配置目录，也可手动选择
- 安装进度、部署状态、调试日志实时展示
- Linux 版本内置完整 `keytao-ime` 系统输入法 daemon
- macOS 版本包含正式支持的 IMKit 系统输入法 bundle
- Windows 版本包含实验性 TSF 系统输入法 DLL
- Android 版本包含正式支持的 `InputMethodService` 系统输入法，native engine 通过 JNI 接入 `keytao-core`，Android ABI 的 `librime` runtime 通过 `scripts/android-librime-runtime.sh` 导入并同步到 APK
- iOS 版本包含 `UIInputViewController` 键盘扩展，主 App 与键盘通过 App Group 共享方案、主题和部署状态

## 平台状态

| 平台 | Rime 方案安装 | 系统输入法 |
| --- | --- | --- |
| Linux | 已支持 | 已支持，`keytao-ime` daemon 覆盖 Wayland、KDE、GNOME IBus、XIM、IBus 兼容路径 |
| macOS | 已支持 | 已支持，基于 IMKit，安装到 `/Library/Input Methods/KeyTao.app` |
| Windows | 已支持 | 实验性支持，基于 TSF TIP，注册 `keytao_windows_ime.dll` |
| Android | 已支持 | 已支持，基于 `InputMethodService`，发行包内置 Android ABI 的 native `librime` runtime 和基础 `rime-data` |
| iOS / iPadOS | 已支持 | 已支持，基于 `UIInputViewController` 自定义键盘扩展；Release 提供需自行签名的 unsigned IPA |

## 数据与部署

系统输入法共用 `keytao-core`：

- macOS 用户目录：`~/Library/keytao`
- Windows 用户目录：`%APPDATA%/keytao`
- Linux 用户目录：`$XDG_DATA_HOME/keytao`，通常是 `~/.local/share/keytao`
- Android 用户目录：`/storage/emulated/0/keytao`
- iOS / iPadOS 用户目录：App Group `group.ink.rea.keytao-app` 容器下的 `keytao`

App 的“安装方案”只负责写文件；“部署”才会让 librime 编译并加载新配置。`rime.lua` 是否生效，取决于它是否安装到了系统输入法实际使用的用户目录，并且是否完成部署。

“附加方案”中的 Easy English 随 App 离线提供，安装时复制 `easy_en` schema、词典和 Lua 到同一用户目录，并与当前键道方案一起部署。重新安装或升级键道方案会保留 `easy_en`，将它放在所有包内方案之后，且不会把它选作启动方案；卸载附加方案会删除其源码、编译产物和用户词典，移动端若正在使用“English 方案”则自动退回“ASCII 模式”。14,566,541 B 的原始词典 gzip 后为 4,153,342 B；当前 debug APK 中六个 add-on asset 的 deflate 体积合计 4,694,956 B，即 APK 增量约 4.7 MB。安装源码与 28,216,820 B 编译产物合计约占 43 MB。上游来源、固定提交和许可证见 [`resources/addon-schemas/easy_en/NOTICE.md`](resources/addon-schemas/easy_en/NOTICE.md)。

## 下载

前往 [Releases](https://github.com/xkinput/keytao-app/releases) 下载对应平台的安装包。

- Linux 安装方式见 [docs/linux-install.md](docs/linux-install.md)。
- iOS / iPadOS IPA 签名与安装方式见 [docs/ios-install.md](docs/ios-install.md)。

## 发行打包

KeyTao 是系统输入法，不按普通桌面小工具的分发方式处理：

- macOS 只构建 `pkg`。pkg 同时安装 `/Applications/KeyTao.app` 和 `/Library/Input Methods/KeyTao.app`，安装完成后要求注销并重新登录，不构建 dmg。
- Linux 只构建 `deb` 和 `rpm`，不构建 AppImage 或 tarball。deb/rpm 同时安装图形 App、`keytao-ime` 和包内 runtime，保证可以作为系统输入法安装。
- Windows release 提供 x64 Flutter App 的 NSIS `.exe` 安装包，包含 `current`、`x86` 和 `arm64x` TSF runtime；ARM64/ARM64X 输入法由源码构建链路生成。
- macOS、Linux、Windows、Android 和 iOS 发行包都应自带完整 Rime runtime：`librime`、OpenCC 数据、`rime-plugins` 和基础 `rime-data`。主 App 与系统 IME 使用同一套包内 runtime，避免 Lua 方案在 App 部署时可用、到 IME 进程里不可用。iOS Release 上传 unsigned IPA，安装前需要按 [iOS / iPadOS 签名与安装指南](docs/ios-install.md) 完成签名。

### 通用准备

```bash
cd flutter_app
flutter pub get
cd ..
```

版本唯一来源是根目录 `Cargo.toml` 的 `[workspace.package].version`。修改后同步 Flutter 版本：

```bash
node scripts/sync-version.mjs
# Or update Cargo.toml and pubspec.yaml together:
node scripts/sync-version.mjs --set 1.2.1-alpha.89
```

### macOS

完整发行包从仓库根目录构建：

```bash
scripts/build-macos.sh
scripts/verify-macos-pkg.sh target/keytao-macos-pkg/keytao-app-1.2.1-alpha.89-macos.pkg
```

`scripts/build-macos.sh` 构建 Flutter 主 App 和 IMK 输入法，使用 `macOS-universal` librime SDK。Release CI 发布一个 `-macos.pkg`。

产物：

- `target/keytao-macos-pkg/keytao-app-<version>-macos.pkg`

本机安装测试需要管理员权限，安装动作单独执行：

```bash
sudo installer -pkg target/keytao-macos-pkg/keytao-app-1.2.1-alpha.89-macos.pkg -target /
```

安装后先注销并重新登录 macOS，让当前用户会话重新发现 `/Library/Input Methods/KeyTao.app`。打开 KeyTao 后仍需手动安装方案并点击“部署”，完成前 App 会保持“未安装”或“待部署”状态。

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
scripts/build-linux.sh
```

`scripts/build-linux.sh` 构建当前 Docker builder 的原生 Linux 架构。builder 安装发行版提供的 `librime-dev`、`librime-plugin-lua` 等 native 包，再把 `librime*.so*`、插件、OpenCC 数据和基础 `rime-data` 放入包内 runtime。Release CI 分别构建 `linux-x64` 和 `linux-arm64` 包。

产物在 `target/release/bundle/` 下，包含：

- `deb`
- `rpm`

### Windows

Windows 需要 Flutter、Visual Studio C++ 工具、MSVC Rust target、LLVM/libclang 和 NSIS 3，推荐从 PowerShell 运行：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows-flutter.ps1 -Arch x64
```

脚本依次构建 x86、x64、ARM64 和 ARM64X 输入法 runtime，再构建 Flutter App 与 NSIS 安装包。只构建某个输入法 runtime 时使用：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows-ime.ps1 -Arch x64
```

产物通常位于：

- `target\keytao-windows-ime-runtime\current`
- `target\keytao-windows-ime-runtime\x64`
- `target\release\bundle\nsis`

### Android

Android 使用 `flutter_app/android` 和 `InputMethodService`。发布支持 `arm64-v8a`、`armeabi-v7a`、`x86_64` 三个 ABI；构建前为目标 ABI 导入 librime runtime，并安装 Android NDK：

```bash
# Import an SDK for each target ABI.
scripts/android-librime-runtime.sh import-sdk --abi arm64-v8a --source /path/to/android-librime-sdk

# Or bootstrap from the Fcitx5 Android Rime plugin.
scripts/android-librime-runtime.sh import-fcitx5-rime --abi arm64-v8a --version 0.1.2

# Build split APKs; Gradle syncs assets and the hook packages native libraries.
(cd flutter_app && flutter build apk --release --split-per-abi --target-platform android-arm,android-arm64,android-x64)

# Check one Rust target.
source <(scripts/android-librime-runtime.sh env --abi arm64-v8a)
cargo check -p keytao-core --target aarch64-linux-android
```

Release CI 导入上述三个 ABI 的 runtime，构建 split APK 并上传到 GitHub Release。用户首次打开 Android 版 App 时，会先进入系统输入法启用/切换引导，KeyTao 已启用并选中后再进入主界面。

Gradle `preBuild` 通过 `scripts/android-librime-runtime.sh sync --all --assets-only` 同步 shared data；Flutter native-assets hook 打包 bridge 和依赖库。Rust 编译要求匹配 ABI 的 `vendor/librime/android/<abi>` 和可用 NDK sysroot。

### iOS / iPadOS

使用 `scripts/build-flutter-ios.sh` 构建 unsigned IPA。签名、App Group 和真机安装说明见 [iOS 安装指南](docs/ios-install.md)。

## 开发

推荐使用 `direnv` 自动加载 flake 开发环境：

```bash
direnv allow
```

进入仓库目录后安装依赖并启动开发环境：

```bash
cd flutter_app
flutter pub get
flutter run
```

构建：

```bash
flutter build macos --release
```

发行包构建命令见上面的“发行打包”。
