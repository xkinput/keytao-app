# 键道 Win/Linux/iOS 迁移到 Flutter 与 alpha 发版计划（2026-09-28）

本文保留 alpha.89 迁移计划和项目方决定。第 1、2 节为实施前快照，不代表当前状态；alpha.89 已发布，当前构建入口见根 README 和各平台打包文档。路径默认相对 `keytao-app/`。

## 1 迁移前快照（历史）

- **Windows**
  - core 已经有 TSF 状态、注册和部署：`crates/keytao-app-core/src/ime_status/windows.rs:970-1139`、`deploy.rs:58-102`。
  - 公共方案发布和 `finish_windows_scheme_install` 当时只在旧外壳里；现已迁到 `crates/keytao-app-core/src/windows_public_schemas.rs` 和 `install.rs`。
  - bridge 的 Windows 回调全部返回 unsupported（`crates/keytao-app-bridge/src/host.rs:149,178-205`），也没有 Windows IME API 和 AppActions API（`api/core.rs:506`）。
  - Flutter 侧：bootstrap 直接抛 UnsupportedError（`flutter_app/lib/src/app/bootstrap.dart:45-64`）；build hook 没有 Windows 分支（`flutter_app/hook/build.dart:77-79`，已复核）；runner 还是模板，没有单实例，也不处理 keytao:// 参数（`windows/runner/main.cpp:28-30`）。
  - 安装器当时依赖旧外壳的 NSIS hooks；现由 `packaging/windows/nsis-hooks.nsh` 维护。TSF DLL 的构建可以原样复用（`scripts/build-windows-ime.ps1:332-386`）。
- **Linux**
  - 输入法是自研 daemon keytao-ime（`crates/keytao-linux-ime/README.md:3`），core 已有状态查询和启动（`ime_status/linux.rs:496-534`）。
  - bridge 没把 Linux API 暴露给 Dart（`api/core.rs:506`）。
  - bridge 部署用的 shared 目录是 `resource_dir/rime-data`，daemon 用的是 `runtime/rime-data`，两边对不上（`host.rs:124-139`）。这是一个真 bug。
  - Dart 没有 Linux 分支（`controller.dart:476-486`）。runner 是模板，APPLICATION_ID 为 ink.rea.keytao（`flutter_app/linux/CMakeLists.txt:7,10`）。
  - 当时由旧外壳打包 deb/rpm；现入口是 `scripts/container-build.sh` 和 `packaging/linux/build-packages.sh`。包里没有 autostart，也没有 KDE launcher。
- **iOS**
  - 当时的 Apple 工程是未跟踪生成物；现工程保存在 `flutter_app/ios`。键盘扩展源码在 `crates/keytao-ios-ime`（SwiftPM）。
  - Rust 和 build hook 已就绪：`hook/build.dart` 的 iOS 分支、`crates/librime-sys/build.rs:35-45`。
  - Flutter Runner 还是模板：bundle id 是 ink.rea.keytao（`ios/Runner.xcodeproj/project.pbxproj:387`），没有 entitlements、没有键盘扩展，也不拷贝 rime-data。
  - bridge 在 iOS 上必须传 `user_root_override`（`host.rs:88-90`），但 Runner 还没有原生 channel。UI 的移动端分支只认 isAndroid（`controller.dart:81`）。
  - 本机：没有模拟器 runtime（`xcrun simctl list runtimes` 为空），没连设备，provisioning profile 为 0。调查时 simctl 自动把 CoreSimulator 从 1051.55 升到了 1171.7，这是一个副作用。
- **Android / macOS**
  - 当时 Flutter 版已验收，Android/macOS 发布链尚未迁移；alpha.89 已统一切换 Flutter。
  - F2 有一处一定会失败：Flutter macOS 产物是 universal，IME 只编本机架构，`scripts/verify-macos-pkg.sh:145-161` 的架构比对会不通过。该脚本里 exe 名硬编码了 3 处（`:98,:132,:145`），plan 只写了 `:98`。
- **发布管线**
  - 只有一个 `.github/workflows/release.yml`，推 v* tag 触发（`:3-6`）；tag 含 alpha 就标为 prerelease（`:12-32`）。
  - 所有平台都还是 Tauri，没有任何 Flutter 步骤（`grep -c -i flutter release.yml` → 0，已复核）。
  - secret 只有 GITHUB_TOKEN 和 ANDROID_KEY_*（`:274-276`）。
  - 本地 main 比 origin 多 40 个提交，最新 tag 是 v1.2.1-alpha.88（`git describe` → v1.2.1-alpha.88-40-g5861bea，已复核）。
  - CI 没有 tag 以外的触发方式，所以 Windows 和 x64 Linux 在打 tag 前无法验证。
- **文档**
  - 网站唯一的安装页 `keytao-next/app/install/page.tsx`（1023 行）里，Linux 部分仍在教装 ibus-rime、用 `~/.config/ibus/rime`（`:87-89,179-191,981-986`）。实际数据目录是 `~/.local/share/keytao`（`crates/keytao-core/src/lib.rs:5106-5108`），两者不符。
  - `docs/linux-install.md` 有两处过时：包文件名（`:21,27`）和日志路径（`:265`，实际是 `~/.local/state/keytao/log`）。另外缺 deb/rpm 用户在 KDE、GNOME 上的启用步骤，以及 autostart 和环境变量说明。

## 2 批次计划

以下文件每个同一时间只能有一个写者：bridge 的 `host.rs`、`api/core.rs`、`api/types.rs` 和 frb 生成物；Dart 的 `bootstrap.dart`、`controller.dart` 和 `ui/*`；`hook/build.dart`；`release.yml`。所以三个平台在这些文件上只能串行。各平台的原生壳和打包目录互不相交，可以并行。

B3 和 B4i 共用 iOS 的 channel 契约。主会话要在两批开工前定死：channel 名 `keytao/ios`，方法 getPaths 和 openSettings。

| # | 批次 | 文件范围 | 依赖 | 并行 | 本机可验 | 规模 |
|---|---|---|---|---|---|---|
| B1 | Windows 纯重构：公共方案发布、finish_install、prepare_search_schemas 下沉到 core，Tauri 改为调用 core | keytao-app-core、旧应用外壳 | — | 与 B4*/D* 并行 | cargo test + check-core-cross.sh | M |
| B2 | bridge 三平台：Windows 宿主、Windows API 和 receive_app_args；Linux 的 status/start/stop，并修 deploy_paths；iOS userRootOverride；check-core-cross 加 bridge；重新生成 frb | crates/keytao-app-bridge、flutter_app/lib/src/rust/**、scripts/check-core-cross.sh | B1 | 与 B4*/D* 并行 | 交叉 cargo check + test | M |
| B3 | Dart 平台分支：bootstrap/controller、WindowsImeCard、Linux 状态卡、iOS 移动端引导、ios_host.dart | flutter_app/lib/**、test/** | B2 | 与 B4*/D* 并行 | dart analyze + flutter test | L |
| B4h | build.dart 的 Windows 和 Linux librime 分支 | flutter_app/hook/build.dart | — | 是 | Linux 可在 colima 验；Windows 只能 CI | S |
| B4w | Windows 壳：exe 名 keytao-app、单实例、WM_COPYDATA 转发、CMake 打包 rime.dll 和 addon-schemas、NSIS 移植 hooks、构建和校验脚本 | flutter_app/windows/**、scripts/*windows*.ps1 | B4h | 与 B4l/B4i 并行 | 否，只能 Windows 机或 CI | L |
| B4l | Linux 壳和打包：runner 标识、RPATH、Dockerfile 加 Flutter、flutter build linux + deb/rpm（包名不变）、verify 脚本 | flutter_app/linux/**、scripts/{Dockerfile.linux-builder,container-build.sh,build-linux.sh,verify-linux-bundles.sh}、packaging/linux/* | B4h、Q4 | 与 B4w/B4i 并行 | colima arm64 可跑全流程；x64 只能 CI | L |
| B4i | iOS 壳：bundle id、真机和模拟器分开的 entitlements、rime-data 拷贝、KeyTaoKeyboard 扩展、AppDelegate channel、出未签名 IPA 的脚本 | flutter_app/ios/**、scripts/build-flutter-ios.sh | Q2 | 与 B4w/B4l 并行 | `flutter build ios --no-codesign` + 检查 IPA 里有 appex；模拟器冒烟要下载数 GB runtime，需 owner 同意 | L |
| B4m | F2 macOS：build-macos.sh 换成 flutter build、调整重签顺序、写入版本号、verify 脚本 3 处 exe 名、架构对齐 | scripts/build-macos.sh、scripts/verify-macos-pkg.sh | Q3 | 是 | 是 | M |
| B5 | release.yml（只一个写者）：加 flutter-action 3.47.5；Android(F1)、macOS、Windows、Linux、iOS 各 leg 换成 Flutter，没过门禁的平台保留 Tauri leg；加 workflow_dispatch，只出 artifacts、不建 Release | .github/workflows/release.yml | 各 B4* | 否 | 否，只能 CI | M |
| D1 | linux-install.md 修包名和日志路径，补分桌面启用、环境变量写在哪、启动与使用、避坑；crate README 改日志路径 | docs/linux-install.md、crates/keytao-linux-ime/README.md | Q4 | 随时 | 是 | M |
| D2 | 网站 Linux 部分重写：推荐 keytao-app 的 deb/rpm，Linux 教程抽成 LinuxGuide.tsx | keytao-next/app/install/{page.tsx,LinuxGuide.tsx} | D1 定稿 | 随时 | next build + 渲染检查 | M |
| V | Windows 真机验收：覆盖升级旧 Tauri 版、UAC 注册 TSF、keytao://ime/redeploy、候选框定位（#92050、#191196） | — | B5 预跑产物 | — | 否，在 Rea 的 Windows 机上 | S |

keytao-next 工作区里还有别人未提交的改动（BatchPRList、Navbar、phrases）。D2 只能 add 自己的两个文件。

**打 alpha tag 前必须全绿：**
1. 本机：`cargo test --workspace`；`scripts/check-core-cross.sh`（含 bridge 的 windows-gnu 检查）；`dart analyze` + `flutter test`；`build-macos.sh` + `verify-macos-pkg.sh`；colima 里跑 `build-linux.sh` + `verify-linux-bundles.sh`；iOS no-codesign 出的 IPA 里有 `PlugIns/KeyTaoKeyboard.appex`。
2. CI：workflow_dispatch 预跑，所有 leg 都绿，产物命名和 alpha.88 一致。这次预跑也顺带首次验证 rust-toolchain 1.98.1。
3. Windows 真机验收 V 通过。如果没过，Windows 这次继续发 Tauri 包。
4. D1 和 D2 已合入。

## 3 发版步骤（v1.2.1-alpha.89）

1. 本地执行 `node scripts/sync-version.mjs --set 1.2.1-alpha.89` 并 commit。当前脚本只写 `Cargo.toml` 和 `flutter_app/pubspec.yaml`。
2. **【需 owner 确认】** `git push origin main`，包含 40 个旧提交和本轮所有批次。
3. **【需 owner 确认】** `gh workflow run release.yml --ref main` 做预跑（依赖 B5 加的 workflow_dispatch），用 `gh run watch` 跟进，约 25 分钟。失败不自动重跑，停下来报告。
4. 在 Rea 的 Windows 机上做验收 V。
5. **【需 owner 确认】** `git tag -a v1.2.1-alpha.89 -m "Release v1.2.1-alpha.89"`，然后 `git push origin v1.2.1-alpha.89`。CI 会自动建 prerelease（`release.yml:12-32`）。
6. 用 `gh release view v1.2.1-alpha.89 --json assets` 核对产物，按第 4 节最终决定应为 10 个，Android 不发布 x86 APK：
   - Android：3 个 APK（arm / arm64 / x86_64），release keystore 签名。
   - macOS：1 个 universal pkg（arm64 + x86_64），ad-hoc 签名。
   - Windows：1 个 x64 setup.exe，未签名。
   - Linux：x64 和 arm64 各一个 deb、一个 rpm，共 4 个。
   - iOS：1 个 arm64 未签名 IPA。
7. **【需 owner 确认】** push keytao-next 的 D2 提交，触发网站部署。
8. 已知风险：
   - owner 手机上装的是本机 debug 签名版，CI 的 release 签名 APK 覆盖安装会报 UPDATE_INCOMPATIBLE。需要先卸载一次，`/sdcard/keytao` 会保留。
   - 本机装的是 Tauri alpha.84，pkg 开了版本比较，新包版本必须 ≥ 1.2.1-alpha.84（`scripts/build-macos.sh:240-262`）。
   - Windows 安装包未签名会触发 SmartScreen，和 alpha.88 一样。
   - app 内的更新检查只看 `/releases/latest`，会跳过 prerelease（`crates/keytao-app-core/src/update.rs:10`），所以 alpha 用户不会收到更新提示。

## 4 项目方决定（2026-09-28）

1. **发版范围**：Windows、Linux、iOS、Android、macOS 五个平台全部完成并验收后，alpha.89 一次性全部换成 Flutter；不保留 Tauri leg。release.yml 仍加 workflow_dispatch，用于打 tag 前的预跑。
2. **标识全部沿用**：Windows 用 keytao-app.exe、%LOCALAPPDATA%\ink.rea.keytao-app 和 NSIS（移植 Tauri hooks）；Linux 沿用原包名、/usr/lib/KeyTao，APPLICATION_ID 为 ink.rea.keytao-app；iOS 用 ink.rea.keytao-app / .keyboard / group.ink.rea.keytao-app，继续发未签名 IPA，不上 TestFlight。
3. **macOS 出一个 universal pkg**：IME 与 FFI 编成 universal，Flutter app 保持 universal；删掉 macos-15-intel CI leg；verify-macos-pkg.sh 改为校验 universal（x86_64 + arm64）。
4. **Linux**：deb/rpm 自带 /etc/xdg/autostart/keytao-ime.desktop（NotShowIn=GNOME）和 KDE 的 keytao-wayland-launcher.desktop，app 退出不再停 keytao-ime；教程以网站 /install 为主写全（分桌面启用、环境变量写在哪、启动与使用、避坑），docs/linux-install.md 同步；ibus-rime 手动方式作为备选；未实测的 Flatpak / SDL / kitty 不写。
5. **提醒**：CI 出的 Android 包为 release 签名，项目方手机上的本机 debug 版需先卸载一次（/sdcard/keytao 保留）。

6. **Android 发行 ABI**：只发布 armeabi-v7a、arm64-v8a 和 x86_64 三个 APK，不发布 x86 APK。
