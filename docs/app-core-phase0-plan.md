# KeyTao App 原生化 · 第 0 阶段详细计划（草案，2026-09-27）

总体方案：Flutter 一套 UI 覆盖五端 + flutter_rust_bridge（FRB）调用 Rust 核心；迁移顺序 Android → iOS → 桌面。
第 0 阶段只做两件事：**把后端从 Tauri 剥离成 UI 无关的 Rust 核心**，以及**Flutter 输入法实测（闸门）**。不改任何界面、不改任何行为，现有 Tauri App 全程可用。

## 1. 目标与边界

| 做 | 不做 |
|---|---|
| 新建 `crates/keytao-app-core`（不依赖 tauri），承载 App 全部业务逻辑 | 任何 UI 改动，React 界面原样保留 |
| Tauri 命令退化为几行的薄包装，事件经 `EventSink` 发出 | 删除 React / Tauri（第 3 阶段末才删） |
| 引导状态（只首次显示）与老用户登录态迁入核心状态文件 | 改输入法热路径：`keytao-core-ffi`（C ABI）和 Android JNI 导出行为不变 |
| 新建 FRB 绑定 crate + 最小 Flutter 冒烟 App | 行为变化、性能优化、顺手重构 |
| Flutter 输入法实测，五端出结论 | |

## 2. 改动面清单（冻结；机械生成，可复算）

复算：`python3 scripts/phase0-inventory.py`（开工时放进仓库；逻辑 = 扫 `src-tauri/src/**/*.rs` 的 `#[tauri::command]`、`generate_handler!`、`.emit(`、Tauri API 关键字，以及 `src/**/*.ts*` 的 `invoke("…")` / `listen("…")`）。

| 项 | 数量 |
|---|---|
| 命令定义 / 注册 | 68 / 68（lib.rs 8,765 行；windows_app_actions.rs 231；wanxiang.rs 649；windows_public_schemas.rs 445；rime.rs 264） |
| 前端实际调用 | 52（任何以引号字符串出现的命令名都算调用，含 `invoke(a ? "x" : "y")` 这类条件调用） |
| 从未调用（直接删，不移植） | 16：android_open_app、linux_enable_kde_support、linux_restart_ime、linux_start_ime、macos_install_ime、macos_uninstall_ime、rime_change_page、rime_has_schemas、rime_inject_text、rime_is_ready、rime_memory_usage、rime_process_key、rime_reset、rime_select_candidate、rime_setup、set_ime_ui_color_scheme |
| 带 `AppHandle` 的命令 | 51 |
| 事件 | 发送点 29 处；前端监听 4 种：install-progress、deploy-progress、windows-ime-status、windows-ime-action |
| Tauri API 调用 | resource_dir ×50、run_mobile_plugin ×16、async_runtime ×6、package_info ×3、get_webview_window ×3、app_cache_dir ×2、app_data_dir ×1、global_shortcut ×1 |
| Android JNI 导出（编在 Tauri 的 .so 里） | 约 43 个（lib.rs:4001-4935，库名 keytao_app_lib） |

后续各批只能对这张表打勾；出现表外改动面 = 拆批不对，停下重拆，不加轮。

### 52 个在用命令按功能分组（= 分批依据）

| 组 | 命令 |
|---|---|
| A 方案获取与安装 | rime_get_data_dir、android_keytao_data_dir、fetch_latest_release、fetch_scheme_release、download_to_temp、smart_install、rime_install_to_default、check_local_schema、read_local_schemas、list_dir、addon_schema_status/install/uninstall、wanxiang_status、manage_wanxiang、windows_prepare_search_schemas、check_app_update |
| B 部署与输入法状态 | rime_deploy_default、get_component_versions、windows_ime_status、windows_ime_ensure_registered、windows_pending/dismiss/redeploy_ime_action、macos_ime_status、linux_ime_status |
| C 输入法设置 | get/set_ime_ui_settings、set_ime_embedded_composition、get/set_desktop_english_mode、get/set_android_ime_input_settings |
| D 账号与日志 | keytao_login、keytao_me、sync_user_dictionary、get/set_runtime_log_settings、read/clear/share_runtime_log、read_debug_logs |
| E 纯 UI 平台动作（不进核心） | select_directory、android_pick_directory、android_list_files、android_read_local_schemas、android_smart_extract、android_ime_status、android_storage_permission_status、android_open_storage_permission_settings、android_open_input_method_settings、android_show_input_method_picker |

E 组本质是"让系统弹个界面/查个系统状态"，在 Flutter 里由平台通道（Kotlin/Swift 插件）直接做，不经过 Rust；第 0 阶段它们留在 Tauri 外壳里不动，第 1/2 阶段随 Tauri 一起消失。

## 3. 核心接口草案

```rust
pub struct AppEnv { pub data_dir: String, pub cache_dir: String, pub resource_dir: String,
                    pub app_version: String, pub platform: Platform }
pub enum CoreEvent { Install(InstallProgress), Deploy(String),
                     WindowsImeStatus(WindowsImeStatus), WindowsImeAction }
pub trait EventSink: Send + Sync { fn emit(&self, e: CoreEvent); }

pub struct Core { /* env, http client, AppState, ime helpers */ }
impl Core {
    pub fn new(env: AppEnv, sink: Arc<dyn EventSink>) -> Arc<Self>;
    // A
    pub async fn fetch_scheme_release(&self, req: ReleaseQuery) -> Result<Release, CoreError>;
    pub async fn install_scheme(&self, req: InstallRequest) -> Result<InstallOutcome, CoreError>; // progress → sink
    pub fn check_local_schema(&self, id: &str) -> Result<LocalSchema, CoreError>;
    // B
    pub async fn deploy(&self) -> Result<DeployOutcome, CoreError>;                             // progress → sink
    pub fn ime_status(&self) -> Result<ImeStatus, CoreError>;                                   // per-platform enum
    // C / D …（与现有命令一一对应，签名去掉 AppHandle）
    // 引导与状态
    pub fn onboarding(&self) -> OnboardingState;        // steps + completed
    pub fn complete_onboarding(&self) -> Result<(), CoreError>;
    pub fn import_legacy_state(&self, s: LegacyState) -> Result<(), CoreError>; // localStorage → 核心，只执行一次
}
```

| 设计点 | 做法 |
|---|---|
| 进度与事件 | 核心只认 `EventSink`。Tauri 外壳的实现里调 `app.emit`（前端不用改）；FRB 层把 `StreamSink<CoreEvent>` 包成 `EventSink`，Dart 侧拿到 `Stream` |
| 路径和版本 | 启动时传入 `AppEnv`，取代 50 处 `resource_dir` 和 3 处 `package_info` |
| 异步 | 直接用 tokio（已是依赖），取代 6 处 `tauri::async_runtime` |
| 状态文件 | `AppState` JSON 放在 data_dir：登录 token、用户信息、`onboarding_completed`；原子写 |
| 引导只首次显示 | 核心判定：`onboarding_completed` 为真就不显示。老用户：首次运行新核心时，本地已装方案 → 直接记为已完成 |
| 老用户登录态 | 第 0 阶段的 Tauri 版本启动时，前端把 localStorage 里的 token/用户信息调用一次 `import_legacy_state`，之后以核心为准 |
| 绑定 | 桌面与移动都用 FRB 2.x 生成 Dart；Android 上 FRB 的 C ABI 与 43 个 JNI 导出编进同一个 cdylib（库名仍 keytao_app_lib），避免两份 Rime 全局状态 |

## 4. 分批计划（每批纯重构，验收 = 现有测试不改即全绿）

| 批 | 内容 | 验收 | 评审 |
|---|---|---|---|
| 0a 骨架 | 建 crate；`AppEnv`/`EventSink`/`CoreError`/`AppState`；Tauri 适配层；删除 16 个死命令及其死依赖 | `cargo tree -p keytao-app-core` 无 tauri；原有测试全绿；前端 `src/` 无 diff | 轻量 |
| 0b 方案获取与安装（A 组） | 下载、镜像、缓存、解压合并、附加方案、万象、更新检查迁入核心；install-progress 走 sink | 同上 + wanxiang_tests 原样通过；macOS 本机冒烟：安装→进度正常 | Opus 评审（下载与文件写入） |
| 0c 部署与状态（B、C 组） | 部署、各平台输入法状态、输入法设置迁入核心；deploy-progress / windows-ime-* 走 sink | 同上 + macOS 冒烟：部署→日志 | 轻量 |
| 0d 账号与日志（D 组）+ 状态 | 登录、同步词库、运行日志迁入核心；`AppState` 持久化；`import_legacy_state` 接线；引导状态接口 | 同上 + 老用户升级路径：登录态保留、已装方案不再显示引导 | Opus 评审（token 与迁移） |
| 0e Android 拆分 | 43 个 JNI 导出挪进核心 cdylib，库名不变；E 组命令留在外壳 | APK 冒烟（你在真机上）：输入法输入、`:rime_deployer` 部署、`/sdcard/keytao` 授权 | Opus 评审（.so 与进程） |
| 0f FRB 冒烟 | 新建 `crates/keytao-app-bridge` + 最小 Flutter App：调 `install_scheme`，Dart 侧显示进度流 | macOS 与 Android 模拟器各跑通一次带进度的安装 | 轻量 |

顺序：0a → 0b → 0c → 0d → 0e → 0f，串行（都改 lib.rs，热点文件只允许一个写者）。估算 4–5 人周。

## 5. Flutter 输入法实测（闸门，与 0a 并行）

一个只有几个 `TextField` 的最小 Flutter App，五端各跑一遍：

| 平台 | 测什么 | 谁来跑 |
|---|---|---|
| macOS | 键道 + 系统拼音：组字显示、候选框跟随光标、失焦提交、中文标点 | 本机 |
| Windows | 键道（TSF 经 IMM32 兼容层）+ 微软拼音：同上，重点看候选框定位（Flutter #92050） | 你的 Windows 机器（我提供构建好的包） |
| Linux | fcitx5 + ibus + 键道自带 Wayland 输入法：能否输入（Flutter #91798）、组字、候选 | 你的 Linux 机器 |
| Android | 键道键盘在 Flutter 输入框里：组字、退格、`[微笑]`、回车 | 你的手机 |
| iOS | 键道键盘在 Flutter 输入框里：组字、退格 | 你的 iPhone |

产出：每端"能用/不能用"表。不过关的平台在第 1 阶段开工前决定对策（修 Flutter 嵌入层 / 该平台单独用原生界面）。

## 6. 风险

| 风险 | 对策 |
|---|---|
| Flutter 桌面中文输入（Windows 候选定位、Linux ibus/fcitx） | 第 5 节闸门先行 |
| FRB 与 JNI 同一个 .so | 0e/0f 在真机和模拟器上验证 |
| Flutter + cargo 构建链与现有脚本、CI 的整合 | 0f 选定方案（FRB 自带的 cargokit 集成） |
| 拆批中途冒出新改动面 | 对照第 2 节清单，超出即停下重拆 |

## 7. 需要确认

1. E 组"纯 UI 平台动作"在 Flutter 里用平台通道（Kotlin/Swift 小插件）实现，不进 Rust 核心 —— 推荐。
2. 版本锁定：Flutter 3.47 stable、flutter_rust_bridge 2.13（调研时的最新稳定版），第 0 阶段开工时再核一次。
3. Windows / Linux / iOS / Android 的实测需要你在对应设备上跑我提供的包，可以吗？


---

# Phase 0 计划补遗：版本锁定与最新优化特性（2026-09-27）

原则：只锁 stable 版本。beta/preview 不进主干。标“待核实”的项在 Phase 0 spike 中实测确认。

## 1. 版本锁定表

| 组件 | 锁定版本 | 发布日期 | 稳定性 | 来源 |
|---|---|---|---|---|
| Flutter | 3.47.5。硬下限为 3.47.4：该版修复了 Xcode 27 白屏挂起（#189284），以及 iOS native assets 部署目标导致的拒审（PR #191964） | 2026-09-18（3.47.0 为 2026-08-12） | stable。3.49.0-0.1.pre（2026-09-21）是 beta，不用。3.50 预计 2026-11 发布，10-06 切分支 | https://storage.googleapis.com/flutter_infra_release/releases/releases_macos.json ；https://raw.githubusercontent.com/flutter/flutter/stable/CHANGELOG.md ；https://flutter.dev/blog/whats-new-in-flutter-3-47 |
| Dart | 3.13.4（随 Flutter 3.47.5） | 3.13.0 为 2026-08-12；3.13.4 为 2026-09-18 | stable（3.14.0 是 beta） | https://dart.dev/blog/announcing-dart-3-13 |
| flutter_rust_bridge（运行库、codegen、flutter_rust_bridge_hooks 用同一版本） | 2.13.0，要求 Dart >=3.9.2 | 2026-08-23 | stable。2.14.0-beta.2（2026-09-12）是 beta，不用 | https://pub.dev/api/packages/flutter_rust_bridge ；https://github.com/fzyzcjy/flutter_rust_bridge/releases/tag/v2.13.0 |
| native_toolchain_rust | 1.0.7，由 hooks 传递引入，不单独声明 | 2026-09-10 | stable | https://pub.dev/packages/native_toolchain_rust |
| Rust | 1.98.1，写入 rust-toolchain.toml：`channel = "1.98.1"` 加各平台 targets | 2026-09-03 | stable。1.99.0 约在 2026-10-01，这是按六周节奏推算的，官方未公布 | https://static.rust-lang.org/dist/channel-rust-stable.toml ；https://blog.rust-lang.org/releases/ |
| Rust edition | 2024，仅用于新建的 keytao-app-core | 随 1.85.0 发布，2025-02-20 | stable | https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/ |
| Android NDK | r30 30.0.16248370（LTS），待 §4-2 拍板。Flutter/AGP 默认是 28.2.13676358 | 2026-09-08 | stable LTS | https://developer.android.com/ndk/downloads ；https://github.com/android/ndk/releases |
| AGP | 9.4.0，待 §4-2 拍板。Flutter 3.47.5 的 maxKnownAgpVersion 是 9.2，模板用 9.1.0，所以 9.4 未经 Flutter 验证（待核实） | 2026-09 | stable | https://developer.android.com/build/releases/agp-9-4-0-release-notes ；https://raw.githubusercontent.com/flutter/flutter/stable/packages/flutter_tools/lib/src/android/gradle_utils.dart |
| Gradle | 9.7.x。AGP 9.4 要求至少 9.6.0，Kotlin 2.4.20 声明支持到 9.7.0。补丁号和发布日期待核实 | — | stable。9.8.0 是 2026-09-27 当天发布的，等热修后再升 | https://docs.gradle.org/current/release-notes.html |
| Kotlin | 2.4.20，走 AGP 9 内置 Kotlin。实际版本由 AGP 决定，能否覆盖到 2.4.20 待核实。Flutter 模板用 2.4.0 | 2026-09-07 | stable | https://kotlinlang.org/docs/releases.html ；https://blog.jetbrains.com/kotlin/2026/09/kotlin-2-4-20-released/ |
| JDK | 17（Flutter 硬下限） | — | stable | https://raw.githubusercontent.com/flutter/flutter/stable/packages/flutter_tools/gradle/src/main/kotlin/DependencyVersionChecker.kt |
| Xcode / SDK | Xcode 27.0（27A266a，本机装的就是正式版），Swift 6.4，iOS 27 / macOS 27 SDK | 2026-09-14 | stable。27.1 b1 和 27.2 b1 是 beta，不用 | https://xcodereleases.com/ ；https://developer.apple.com/tutorials/data/documentation/xcode-release-notes/xcode-27-release-notes.json |
| Android 系统 | minSdk 24 / targetSdk 36 / compileSdk 37。Android 17 已于 2026-06-16 出 stable；API 37 要求 AGP ≥ 9.1.1。Flutter CI 只测到 36，所以 targetSdk 暂不升。Play 自 2026-08-31 起要求 targetSdk 36 | — | stable | https://docs.flutter.dev/reference/supported-platforms （2026-09-22 更新）；https://developer.android.com/about/versions/17/release-notes ；https://support.google.com/googleplay/android-developer/answer/11926878 |
| iOS | 15（Flutter 3.47 和 Xcode 27 的共同下限） | — | — | https://docs.flutter.dev/reference/supported-platforms ；https://developer.apple.com/support/xcode/ |
| macOS | Flutter UI 最低 12；IMK 输入法现为 13.0 | — | Flutter 正在淘汰 x64 | https://docs.flutter.dev/reference/supported-platforms |
| Windows | 10 / 11，x64 和 Arm64。需要“最新版 VS”加 Desktop development with C++；VS 大版本和 Windows SDK 下限官方没写，待核实 | — | — | https://docs.flutter.dev/platform-integration/windows/setup |
| Linux | Debian 10–13、Ubuntu 20.04–24.04，x64 和 Arm64，GTK3。官方未公布 glibc 下限，按 Debian 10 推断为 2.28 | — | GTK4 还是未合入的 PR | https://docs.flutter.dev/platform-integration/linux/building ；https://github.com/flutter/flutter/issues/94804 |

## 2. 要启用的新优化特性

| 特性 | 带来什么 | 启用阶段 | 前提或风险 |
|---|---|---|---|
| Dart build hooks 加 FRB native-assets 后端（`--integration-backend native-assets`） | 用一个 hook/build.dart 替代各平台的 CMake、Gradle、Podspec 胶水代码。Flutter 3.47 在五个平台都默认开启 native assets | Phase 0，在 spike 分支做，Cargokit 兜底 | 在 FRB 里是非默认的可选后端，目前只经历过 2.13.0 一个稳定版。需要 `crate-type = ["staticlib","cdylib"]` 和固定版本的 rust-toolchain.toml。vendored 的 librime、Fcitx5、libc++_shared 要么注册成 code asset，要么留在 jniLibs/Xcode 里。Kotlin IME 和 iOS 扩展在 Flutter 之外加载核心，加载路径要实测。Apple 要求 dylib 名在各架构之间、模拟器和真机之间保持一致 |
| @RecordUse 加 link hook 摇树 | 裁掉没用到的 FFI 符号 | 与上一项同步 | 只对用 record_use 标注的符号生效，而且必须走 code assets。说它 stable 的依据是 flutter_tools stable 分支里 enabledByDefault，Dart 文档没有明写 |
| FRB SSE codec（不加 `--full-dep` 时就是它） | 候选、预编辑这类大量小结构体，堆分配更少 | Phase 0 | CI 不需要装 LLVM。CST+DCO 只对词库导入导出这种大块字节有意义 |
| FRB async、StreamSink、RustAutoOpaque | async 和 stream 返回的字节缓冲自动零拷贝；引擎和会话都用句柄式 API | Phase 0 API 设计 | sync 调用仍会拷贝，`#[frb(sync)]` 只给小 getter 用。2.13.0 里空闲 RustStreamSink 的 cancel 可能挂起，修复只在 2.14 beta，所以暂时避免高频取消订阅。不用实验性的 lifetime 和第三方 crate 解析 |
| Cargo `[profile.release]`：`lto="fat"`、`codegen-units=1`、`opt-level=3`、`panic="unwind"`、`strip="symbols"`、`debug="line-tables-only"`、`split-debuginfo="packed"` | .so 更小更快，同时保留符号化文件。ELF 的 .dynsym 会保留，已实测 JNI/FRB 导出不受影响 | Phase 0 | 工作区目前没有 release profile；dev 配置在 .cargo/config.toml，新配置不能与之冲突。iOS 键盘扩展要实测 opt-level 3 和 "s" 再定。禁止 `panic="abort"` |
| edition 2024，加 rust-toolchain.toml 固定 1.98.1 | 新 crate 用最新语言版本；构建可复现（native assets 后端强制要求固定版本） | Phase 0 | 不能用浮动的 stable 通道 |
| NDK r28+（r30）默认 16 KB 对齐 | 不加链接参数就满足 Play 的 16 KB 要求 | 从 Phase 0 开始 | 见 §3.3 |
| Impeller（Android API 29+） | 优先用 Vulkan，失败时自动回落到 Impeller GLES | Android | API 24–28 和 Vivante GPU 仍走 Skia GLES，QA 两类设备都要覆盖 |
| 桌面默认 Impeller 加 SDF 文本 | 着色器在构建期预编译，没有着色器编译卡顿 | 桌面 | 遇到驱动问题可以临时关闭：macOS 用 `FLTEnableImpeller=false`，Windows/Linux 用 project switch 或 `--no-enable-impeller`。官方说关闭能力以后会移除 |
| SwiftPM（3.44 起默认） | 不再依赖 CocoaPods；Rust xcframework 以 binaryTarget 交付 | iOS / macOS | CocoaPods registry 从 2026-12-02 起永久只读，不要再新增 podspec。FRB/Cargokit 的 iOS 集成是否已脱离 CocoaPods，待核实 |
| `--obfuscate --split-debug-info` | 包更小，逆向成本更高，符号文件单独保留 | 各平台 release | Windows x64 产出的是 PDB，flutter symbolize 读不了 |
| AAB / `--split-per-abi` | 按 ABI 分发，安装包更小 | Android | — |
| material_ui / cupertino_ui 1.0 独立包 | 避开 11 月 SDK 内置副本弃用带来的迁移 | Phase 0 新工程 | — |
| Gradle configuration cache | 配置阶段更快 | Android | 插件兼容性要实测 |

不采用的项：
- deferred components：只支持 Android + Play。
- Baseline Profile：Dart 的 AOT 代码从中得不到收益。
- 桌面多窗口 API：目前只在 main 通道。
- Linux GTK4：还没合入。
- 所有 beta：Flutter 3.49、FRB 2.14、Xcode 27.1/27.2。

## 3. 对计划的影响

### 3.1 最低系统版本

| 平台 | 结论 | 理由 |
|---|---|---|
| Android minSdk 24 | 不抬 | Flutter 支持 24–37，NDK r30 也没抬最低 API。代价是 API 24–28 走 Skia，QA 在 24–28 和 29+ 上各要一台设备 |
| iOS 15 | 不抬 | 15 正好是 Flutter 3.47 和 Xcode 27 的下限。要关注 3.50（2026-11）会不会再抬 |
| macOS | Flutter UI 被动抬到 12；建议统一到 13（§4-3） | Flutter 3.47 把最低版本从 10.15 抬到了 12 |
| Windows | 不变 | 10/11，x64 和 Arm64 |
| Linux | 不变 | 实际的 glibc 下限取决于构建镜像。用 Debian 10 或 Ubuntu 20.04 级别的镜像构建 |

### 3.2 现有工具链与原生代码

| 对象 | 要不要升级 | 做法 |
|---|---|---|
| Kotlin 1.9.25 | 必须升 | Flutter 3.47 遇到 KGP < 2.2.20 直接报错。迁移到 AGP 9 内置 Kotlin，步骤见下方。FRB/Cargokit 的 Gradle 脚本是否仍 apply KGP，待核实 |
| NDK 27.0.12077973 和 flake.nix 里的 26.1.10909125 | 必须统一 | Gradle 的 ndkVersion、cargo-ndk/Cargokit/native assets、flake.nix 用同一个版本。APK 里只保留一份 libc++_shared.so。cargo-ndk 是否支持 r30，待核实 |
| Rust 核心的 panic 策略 | 新增约束 | 保持 `panic="unwind"`。Rust 1.81 起，unwind 穿越 `extern "C"`/`"system"` 会直接 abort，所以所有 JNI 导出都要包 `catch_unwind`，或改用 `"system-unwind"` ABI（https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/ ；FRB PR #3330）。建议 FRB 和 JNI 导出合成一个 cdylib，避免出现两份 std 和两份 librime 状态 |
| iOS 键盘扩展（Swift） | 保持原生 | Flutter 文档只建议内存 ≥100MB 的扩展使用 Flutter UI。Runner 必须改用 UIScene：TN3187 规定用 iOS 27 SDK 构建时不采用就无法启动，而 2027-04 起 App Store 强制使用 iOS 27 SDK。键盘扩展不走 App 生命周期，不受影响。Rust 核心以 xcframework + SwiftPM binaryTarget 交付 |
| Apple 构建脚本（librime、Rust、IMK） | 必须清理 | Xcode 27 已移除 ld64，`-ld_classic` 也不再支持。删掉所有 `-ld64` 和 `-ld_classic` |
| Swift 语言模式 | 暂不动 | 并入 Runner 时，每个 target 保留原来的 SWIFT_VERSION。“Xcode 27 新 target 默认 MainActor 隔离”这一说法来自二手资料，待核实 |
| Windows TSF 和 Linux IME（Rust） | Phase 0 不动 | 旧 crate 迁移到 edition 2024 单独做，不和核心抽取捆在一起 |

Kotlin 1.9.25 迁移到 AGP 9 内置 Kotlin 的步骤：
1. 删除 `id("kotlin-android")`。
2. 把 `kotlinOptions` 改成顶层的 `kotlin { compilerOptions { jvmTarget = JvmTarget.JVM_17 } }`。
3. 在 gradle.properties 设置 `android.newDsl=false` 和 `android.builtInKotlin=true`。AGP 10 会强制新 DSL，`newDsl=false` 只是过渡。
4. IME 代码用 K2 编译器重编并验证。
5. Rust 通过 JNI 反向调用的 Kotlin 类和方法，补上 R8 keep 规则。

### 3.3 16 KB 页对齐

| 项 | 内容 |
|---|---|
| 规则 | Play 要求 targetSdk ≥ 35 的应用在 64 位设备上支持 16 KB 页。2027-02-01 起，不合规的更新将无法发布（https://developer.android.com/guide/practices/page-sizes ，2026-09-16 更新）。实际执行日期以 Play Console 为准 |
| 现状 | 本地的 arm64 `libkeytao_app_lib.so` 是 LOAD align 2**12，不合规。这是 debug 产物，需要在 release 构建上复核。vendored 的 64 位 librime、libFcitx5*、libc++_shared 已经是 2**14。32 位 ABI 不在要求范围内 |
| 修复 | 首选用 NDK r28+（r30）重新构建，默认就是 16 KB。如果暂时留在 r26/r27，就在 .cargo/config.toml 的 `[target.aarch64-linux-android]` 和 `[target.x86_64-linux-android]` 下加 rustflags：`-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384`，两个参数都要加。FRB 2.13.0 的 Cargokit 已经适配 16 KB。zip 对齐由 AGP ≥ 8.5.1 负责 |
| CI 门禁 | `llvm-objdump -p <so>` 的 LOAD align 必须是 2**14；APK 过 `zipalign -v -c -P 16 4 app.apk`；AAB 用 `bundletool dump config` 检查出 PAGE_ALIGNMENT_16K；在 16 KB 模拟器上 `getconf PAGE_SIZE` 返回 16384。每次重新导入预编译库都要复检 |

### 3.4 桌面输入法问题：最新版仍未修复

截至 2026-09-27，Flutter 3.47.5 里两个都没修。这些问题只影响 KeyTao 自己 Flutter 界面里的文本框（词库、短语编辑），不影响原生的 TSF、IMK 和 Linux IME 引擎。

| issue | 状态 | 处理 |
|---|---|---|
| #92050：Windows 搜狗候选框在提交后错位 | open，P2，最后更新 2025-10-09 | 桌面阶段用搜狗和 KeyTao TSF 实测候选框定位 |
| #91798：Linux iBus/fcitx 无法在 TextField 里输入 | open，P2，最后更新 2024-06-27。在 3.47 + fcitx5/GTK3 下是否仍复现，没有官方结论，待实测 | 用 fcitx5、ibus 配合 GTK_IM_MODULE 实测 |
| 3.47 已修的部分 | 只修了 #186353（Windows 韩文组字光标位置）。#191780（macOS 越界文本范围崩溃）只在 master 上，没进 3.47.5。3.47.1 到 3.47.5 的热修都与输入法无关 | — |
| 新出现且未修 | #191196：Windows 组字内容串到下一个 TextField（P3）。#190525：macOS 按回车确认组字时同时触发 Shortcuts（P2） | macOS 文本框在组字期间屏蔽 Enter 快捷键 |

## 4. 需要项目方拍板

| # | 待拍板 | 选项 | 建议 |
|---|---|---|---|
| 1 | FRB 集成后端 | A：native assets（最新，带 build hooks 和 @RecordUse）。B：Cargokit（FRB 默认，成熟） | Phase 0 先在 spike 分支跑通 A，包括预编译库的注册，以及 IME 和扩展的加载路径。跑不通就用 B，以后再迁 |
| 2 | Android 版本基线和迁移时机 | 版本 A：全用最新（AGP 9.4.0、Gradle 9.7.x、NDK r30、Kotlin 2.4.20），超出了 Flutter 3.47 验证过的范围。版本 B：Flutter 验证过的组合（AGP 9.1–9.2、Gradle 9.3.1、NDK 28.2.13676358、KGP 2.4.0）。迁移时机：现有 IME 工程在 Phase 0 同步迁到内置 Kotlin，或者 Flutter UI 先单独建一个 Gradle 工程 | 选 A，哪一项不兼容就单独回落到 B 的对应版本。IME 工程在 Phase 0 一起迁，因为 Kotlin 1.9.25 本身已经是硬错误 |
| 3 | macOS 最低版本和架构 | 最低版本：12（Flutter 下限）或 13（IMK 现值）。架构：Universal 或仅 arm64。Flutter 正在淘汰 x64，目前只给警告；macOS 27 只支持 Apple silicon | 统一到 13。先保留 Universal，等 Flutter 把 x64 警告改成报错时再切到仅 arm64 |
---

## 8. 进度与遗留事项（2026-09-28 更新）

| 批 | 提交 | 验收 |
|---|---|---|
| 0a 骨架 + 删除 16 个死命令 | e6b12ad | 44/1 测试不变 |
| 0b 方案获取与安装 | d94dcae（Windows 回归修复 a6d432a） | Opus 评审零发现；外壳 Windows 编译后补 |
| 0c 部署、输入法状态与设置 | 7f3f192 | Opus 等价评审 PASS |
| 0d 账号、同步、日志 + 会话/引导状态 | 6c62bad | 两轮 Opus 评审，8 项修复后 PASS；78/1 测试 |
| 0e Android JNI 导出并入核心 | （见 git log） | 67 个 Java_* 符号 diff 为空；Opus 评审 PASS |
| 0f Flutter 桥接 + 冒烟 | 0bbbe98（骨架 e988769） | macOS release App 内 Dart→Rust 初始化成功；flutter test 通过；arm64 APK 含全部库且 16 KB 对齐 |

编译门禁：`scripts/check-core-cross.sh`（核心 Linux/Windows）、`scripts/check-tauri-windows.sh`（外壳 Windows）、Android APK 构建、macOS 本机测试。

尚未覆盖的编译盲区：
- iOS 外壳：Tauri 构建脚本在本机混用 macOS SDK，暂无法 `cargo check --target aarch64-apple-ios`；cfg(ios) 代码目前只靠审读，第 2 阶段真机兜底。
- Linux 外壳：需要目标平台 GTK/WebKit，本机不做；请在 Linux 机器上 `cargo check -p keytao-app` 一次。

第 1 阶段（Flutter 接管后）必须处理：
- 核心成为会话唯一持有方时，删除 `src/App.tsx` 挂载时的 `app_state_clear_auth` 调用，否则每次启动都会登出。
- 引导处于"待定"期间用户装好方案后，老用户规则会把新用户误判为已完成；由核心驱动引导界面前要收紧判定。
- Windows 上 `data_dir` 与 `cache_dir` 同为 `%LOCALAPPDATA%\<id>`，清理缓存时不能整目录删除（凭据文件在内）；建议状态文件移到独立子目录。
- 清理：Android/iOS 的 `rime_deploy_default` 包装里残留的 Windows 参数；核心 `time` 依赖多余的 `parsing` feature；`android_jni` 里多余的逐项 cfg。
- 首次构建 release APK（LTO）时，重新比对 `libkeytao_app_lib.so` 的 67 个 `Java_*` 导出。

### 0f 的经验（第 1 阶段直接沿用）
- FRB 2.13.0 的 native-assets 后端可用，但要在 `flutter_app/hook/build.dart` 里补三件事：把 vendored Rime SDK（Android 还有由 C 编译器路径推出的 NDK 根目录）通过 `extraCargoEnvironmentVariables` 传给 Cargo（hook 环境被清空）；把 librime 注册为代码资源（macOS 先按架构 `lipo -thin`，Android 连同 Fcitx5 三个库和 `libc++_shared`）；**始终交给 Flutter 私有副本**——直接交 vendor 路径时，资源安装步骤删掉过 `vendor/librime/macos-universal/lib/librime.1.dylib`。
- Flutter 会把 macOS 代码资源包装成 framework 并自动改写依赖路径（`@rpath/rime.1.framework/rime.1`），App 包内 FRB 默认加载器可用；`flutter test` 需显式 `ExternalLibrary.open(build/native_assets/<os>/…)`。
- Xcode 27 下 `flutter build macos --debug` 报 "conflicting deployment targets"（Flutter 自身 `debug_macos_framework` 步骤），release 正常；需关注 Flutter 修复。
- NDK 暂钉在已安装的 27.0（Flutter 默认 28.2、计划中的 r30 在当前网络下载需数小时）；有代理或镜像后升级。
- iOS 还没构建过 Flutter 版：需先 `sudo xcodebuild -runFirstLaunch`，并在 Xcode 里配置签名。
