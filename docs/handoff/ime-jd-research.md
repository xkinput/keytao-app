# ext
ime-jd 是 hronro 独立写的键道引擎 libjd。核心是 Zig，附带自己的各平台前端，不依赖 RIME。最影响打包的是两点：许可证是 GPL-3.0，词库在编译期嵌进库里，运行时不读任何数据文件。
- 仓库：https://github.com/hronro/ime-jd 。调研时读的是 HEAD 9991e82（2026-11-20）。最新版 v0.13.0，1 star。
- 许可证：GPL-3.0，见 `LICENSE`。`APPSTORE-EXCEPTION.md` 按第 7 条额外允许经 Apple App Store / TestFlight 分发，只此一项。KeyTao 无论链接 dylib 还是运行时下载加载，大概率构成衍生作品，要按 GPL 处理。是否必须整体开源 GPL，需要法务或作者确认。
- 词库来源：`core/scripts/update-tables.py` 从 xkinput/KeyTao 的 `rime/keytao.{single,phrase,symbol,supplement,link,english}.dict.yaml`（master 分支）转出 `core/tables/1..7.txt`，共 14.3 万行；`7.css.txt` 是 525 声笔笔词组。也就是说词库就是 KeyTao 的键道6，只是同步时间点不同。
- 词库编进二进制：`gen_trie` 生成 trie.bin，再用 `@embedFile` 嵌入。要换词库或跟 KeyTao 的词库版本走，只能重新编译。README 明确不支持单键/并击、主题、Lua。
- 规则写死，不可配置。
  - 顶功：键不在当前节点下、但在根节点下时，先上屏再重新开始；不在 trie 里的键，上屏锚点候选并追加该字节。
  - 空格上首选，`;` 上次选。`;` 不在编码中时走符号方案（`;`→`；`、`;;`→`：`）。
  - 唯一码不自动上屏，等下一个键顶出。数字选重交给前端。
  - 标点由引擎处理，成对标点会交替。
  - 见 `core/include/jd.h:320-345` 和 `core/docs/integration.md:165,212,380`。
- C ABI（`core/include/jd.h`，494 行；dylib 只导出 16 个 `jd_*` 符号）：
  - 创建/销毁：`jd_init(user_dict_blob, len)` / `jd_deinit`
  - 输入：`jd_press_key(ctx, char)`、`jd_backspace`、`jd_reset`
  - 选择与读取：`jd_set_anchor(idx)` 用于选择或翻页；`jd_state_ptr` 给出 commit_a + commit_b + commit_lit、options_count、anchor_index；`jd_read_range(start, count, out)` 按下标读候选
  - 其他：`jd_scratch_ptr`、`jd_lookup_codes`（反查）、`jd_alloc`/`jd_free`、`jd_abi_layout`（自检结构体大小）
  - 没有 preedit 字符串，只有每个候选的 `hint`（还差的编码），前端要自己记已输入的键。
- 用户词典：`jd_user_dict_build/_check/_blob_bound`。宿主把 `词<TAB>编码` 文本转成 blob，传给 `jd_init`，上限 10000 条。用户词排在同码内置词之前。换词典要新建 ctx。没有自动调频或学习。
- 线程：不同 ctx 可以并行，同一个 ctx 不能并发调用（`jd.h:81-84`）。内置表全局共享只读。每个 ctx 只在 `jd_init` 时分配一次内存。
- 构建：`cd core && zig build`，产出 `zig-out/lib/libjd.{a,so,dylib,dll}`，要求 zig 0.17.0 以上。版本号只看 `core/build.zig.zon`（0.13.0）。另有 wasm32 目标。
- 绑定：`bindings/{rust,swift,java,javascript}`。Rust crate 名 `jd`，`links="jd"`，版本号是占位的 0.0.0，未发布到 crates.io。`build.rs` 读环境变量 `LIBJD_PATH` 使用预编译包，否则自己调 zig 编译。
- 发布资产（v0.13.0，命名 `libjd-v{ver}-{os}-{arch}.{a|so|dylib}.tar.xz`，Windows 为 `.zip`）：

| 平台 | 资产 |
|---|---|
| macOS | arm64、x86_64 的 dylib 和 .a，各自单架构，**没有 universal** |
| Linux | x86_64、arm64 的 so 和 .a |
| Windows | x86_64、arm64、x86 |
| Android | arm64、x86_64 的 so，**没有 armeabi-v7a** |
| iOS | 只有 arm64 静态库 .a，**没有 xcframework，也没有模拟器切片** |
| 其他 | wasm |

  macOS 包里还带 `include/jd.h` 和 `module.modulemap`。
- 校验与签名：没有 SHA256SUMS 文件，也没有 GPG/sigstore 签名，只有 GitHub API 每个资产的 `digest`。例如 macos-arm64.dylib.tar.xz 为 `sha256:0fd59816…1dfc`，我本地 shasum 结果一致。
- 发版节奏：v0.1.0（2026-06-19）到 v0.13.0（2026-11-18），共 21 个版本，最近每周 1–2 个。0.x 阶段，不承诺 ABI 稳定，只有 `jd_abi_layout` 可以做运行时校验。
- macOS dylib 签名：`codesign -dv` 显示 `Signature=adhoc`、`linker-signed`、`TeamIdentifier=not set`，install name 是 `@rpath/libjd.dylib`。它只依赖 libSystem。
- KeyTao 现在的签名：`crates/keytao-macos-ime/build.sh:368-378` 用同一个身份、加 `--options runtime`，对 `Contents/Frameworks/*.dylib` 逐个重签，再签 app。`dev.entitlements.plist` 里有 `com.apple.security.cs.disable-library-validation = true`，没开沙盒。
- 这对接入意味着：
  - 放进 Frameworks 打包，会被重签，没问题。
  - 运行时下载加载，也因关闭了库验证而可行，但等于加载未校验的第三方代码，需要自己用固定 sha256 校验。
  - `build.sh:369-371` 对每个 dylib 都要求同时含 x86_64 和 arm64，所以得先用 `lipo` 把两个单架构 dylib 合成 universal。
- 现成的前端：只有作者自己的几套，没有 Squirrel 或 fcitx-rime 插件。可参考的接入代码在 `macos/JdIME`（IMK + Swift 绑定）、`linux/fcitx5`、`linux/ibus`、`linux/common/engine.h`、`ios/Keyboard`、`android/`、`cli/`（Rust）。

# host
KeyTao 现有的输入引擎链路已经摸清。所有前端都走 `keytao_core::ImeRuntime` / `ImeRuntimeSession`（会话 API 返回 `ImeState`），只有 Windows 已经有一个引擎后端 trait。librime 的硬依赖全部集中在 keytao-core 里的 `mod desktop::Engine`。以下全部基于 main 分支（git show），只读，没有改任何文件，也没有下载东西。

**核心抽象（crates/keytao-core/src/lib.rs）**
1. 文件头注释写的是 "Pure librime engine wrapper"（lib.rs:1）。Cargo.toml:21-23 按 cfg 给五个平台都硬依赖 `librime-sys 0.4.0` 和 `rime-api 0.12.2`。
2. `ImeState`（lib.rs:223-254）包含 preedit、commit_preview、cursor、sel_start、sel_end、candidates、highlighted_candidate_index、page_size、page、is_last_page、committed、select_keys、ascii_mode、chord_composer、schema_id、schema_name。另有 `KeyProcessResult`（lib.rs:487）、`Candidate`（lib.rs:412）、`InputContextPolicy`（lib.rs:261）。
3. `EngineCapabilities`（lib.rs:354-390）描述能否翻页、能否高亮，降级时有对应实现，可以直接给 libjd 用来声明能力。
4. librime 实际调用都在 `mod desktop`（lib.rs:811 起）。`pub struct Engine`（lib.rs:3017）内部持有 `rime_api::Session`，靠全局锁 `RIME_API_LOCK` 串行化。它的方法有 process_key_result（3141）、state（3181）、select_candidate_on_page（3189）、highlight（3219）、change_page（3394）、commit_composition（3453）、select_schema（3567）、set_option（3647）等。
5. 门面层是 `ImeRuntime`（lib.rs:5305，new/with_dirs/init/reload/prewarm 在 5695-6100）和 `ImeRuntimeSession`（lib.rs:5315，方法在 6291-6707：process_key_result 6313、process_keys_result 6318、process_enter 6470、select、change_page 6615、set_english_mode 6654、capabilities 6694）。
6. 会话内部 `ImeRuntimeSessionInner.engine: Option<Engine>`（lib.rs:5670-5676）是具体类型，由 create_session（lib.rs:6175，6235 行赋值）和 rebuild_engine（lib.rs:5549）构造。
7. `key_policy` 模块（lib.rs:4560-4823）与引擎无关，只依赖 ImeState，可以原样复用。`rime_modifier_mask` 在 lib.rs:4823。
8. english_mode 里已经有一个 `pub(crate) trait EnglishBackend`（english_mode.rs:86-94，方法有 current_schema、schema_switches、get/set_option、select_schema、discard_composition、state、apply_ascii_mode）。它是现成的 trait 雏形。
9. 部署、方案相关的全局函数也都绑在 librime 上：deploy、setup_only（lib.rs:4400）、librime_runtime_version（605）、lua/octagram 检测（635/662）、deployed_schemas（7196）。

**各前端怎么调**
10. macOS 和 iOS 都走 keytao-core-ffi（crates/keytao-core-ffi/src/lib.rs）。全局持有 `runtime: Option<ImeRuntime>`（27 行），初始化在 507 行。C API 包括 keytao_init（459）、keytao_create_session（818）、keytao_session_process_key_json（1383）、keytao_session_state_json（1376），ImeState 以 JSON 返回。macOS 的调用顺序见 crates/keytao-macos-ime/README.md:16，iOS 见 KeyTaoIOSEngine.swift:507 和 :777。
11. Windows 已有 `pub trait ImeEngineBackend`（crates/keytao-windows-ime/src/engine_backend.rs:47-70），方法有 build、process_key/keys/enter、select、change_page、reset、state、set_input_policy、set_english_mode。两个实现是 InProcEngine（engine_backend.rs:525）和 PipeEngine（engine_pipe.rs:60），由 build_backend（engine_backend.rs:72）选择。管道协议 crates/keytao-ime-ipc/src/lib.rs:21-110 的 Request/Response 直接传 keytao_core::ImeState，引擎实现换掉后协议不用改。
12. Linux：crates/keytao-linux-ime/src/engine.rs:6 是 `pub use keytao_core::ImeRuntime as CoreEngine`，:56 是 `ImeSession = Arc<ImeRuntimeSession>`，:74 是 create_session。
13. Android：crates/keytao-app-core/src/android_jni/mod.rs:156 有全局 `ANDROID_IME_RUNTIME`，:350 是 android_new_runtime，:390 是 android_session（裸指针转 &ImeRuntimeSession）。

**第二后端 trait 放在哪**
14. 建议放在 keytao-core 里：把 `ImeRuntimeSessionInner.engine`（lib.rs:5674）从具体的 `Engine` 改成 `Box<dyn ImeBackend>`，并在 create_session（6175）和 rebuild_engine（5549）处按包选择后端。方法集合可以参考 Windows 的 ImeEngineBackend（engine_backend.rs:47）加上 EnglishBackend（english_mode.rs:86）。这样 FFI、JNI、Linux、Windows InProc 都不用动，Windows 那个 trait 继续包在外层。
15. 需要注意：deploy/setup/lua/octagram 和 RIME_API_LOCK 都是 librime 专用。libjd 后端要在 ImeRuntime::init/reload（5778/5914/6100）里绕开它们。

**下载与存储（可复用给 libjd 发布包）**
16. 构建期：scripts/fetch-librime.sh 先调 GitHub releases API（154-156 行），用 curl 下载（210 行），再按 asset digest 校验 sha256（216-217 行）。版本钉在 scripts/librime-versions.env:2（LIBRIME_VERSION=1.17.0），Windows 对应 scripts/fetch-librime-windows.ps1。CI 有 .github/workflows/librime-update.yml。
17. 运行期：crates/keytao-app-core/src/download.rs:10 的 `download_to_temp`（流式下载、发进度事件、可取消），以及 downloaded_addon.rs:30-75（Manifest 带 archive_sha256，每个文件有 sha256，校验长度为 64 位十六进制；比对在 149 行）。云端方案包见 cloud_packages.rs:63-70（CloudPackage.sha256，列表时校验在 296-297 行，zip 比对在 33-39 行）。

**引擎选择存哪**
18. 按包存：crates/keytao-app-core/src/packages.rs:19 的 `PackageMeta` 有 scheme_key、version、source、manifest、zip_sha256，其余字段都用 serde(default)，可以加 `engine: Option<String>`，默认 rime。
19. 全局：crates/keytao-app-core/src/state.rs:114 的 `InstalledPackage` 和 AppState（`installed_packages`、`active_scheme`）。
20. IME 进程能读到的设置文件是 keytao-ime.json（english_mode.rs:9 的 SETTINGS_FILE，读写在 :29 和 :44）。ime_settings.rs:13-24 已经按同一模式存候选页大小。引擎类型可以照这个模式写进去，让 keytao-core 在 create_session 时读取。

# design
ime-jd 接入设计（只读，未改仓库，未下载）

**各平台可行性**
- macOS：可做。IME 进程签名开了 `disable-library-validation`、没开沙盒，所以能 `dlopen` 自签名的 libjd.dylib（它只依赖 libSystem）。运行时下载按本机架构取单架构包，不用 lipo；但要是打进 Frameworks，`build.sh:369-371` 要求 universal，得先 lipo 合并。真正的风险在于等于加载未经验证的第三方代码，靠固定的 sha256 兜底。
- Linux：可做。fcitx5 和 ibus 进程直接 `dlopen` 用户目录里的 .so，有 x86_64 和 arm64 两种。
- Windows：只走进程外的 PipeEngine（`engine_pipe.rs:60`）。TSF 的 DLL 会被注入每个宿主进程，不能让它去加载下载来的 DLL。AppContainer 宿主（UWP、Edge 沙盒）读不到 %APPDATA% 里的文件，32 位宿主也没法加载 x64 的 DLL。走管道后 DLL 只进我们自己的引擎进程，架构跟随引擎进程，IPC 协议不用改。InProc 模式下这个开关灰掉。
- Android：技术上可行。从 `filesDir` 用 `System.load` 加载 .so 是允许的，W^X 只限制 exec，不限制 dlopen。但 Google Play 的 Device and Network Abuse 政策禁止从 Play 以外的来源下载可执行代码，所以只能给 GitHub 或侧载版本用。没有 armeabi-v7a 包，这类设备隐藏开关。
- iOS：不能运行时下载（App Store 审核指南 2.5.2，键盘扩展同样适用）。只能在构建期静态链接 arm64 的 .a，`APPSTORE-EXCEPTION.md` 允许经 App Store 分发。没有模拟器切片，用构建开关（feature flag）控制，模拟器构建时排除。按钮在 iOS 上改成"已内置/切换"，不走下载。

**安全**
- 来源写死：只调 `api.github.com/repos/hronro/ime-jd/releases/tags/<tag>`，资产名必须精确匹配 `libjd-v{ver}-{os}-{arch}.*`，拒绝重定向到其他 owner。
- 上游没有 SHA256SUMS 文件，也没有签名，只有 API 返回的 `digest`。所以建议照 `scripts/librime-versions.env` 的做法，新增 `ime-jd-versions.env`，把 tag 和各平台资产的 sha256 随 KeyTao 版本一起发布。客户端只下载已钉住的版本，作者账号被盗也换不了包。"最新"的含义是 KeyTao 发版时由 CI 更新这组钉值（照 `librime-update.yml` 的做法），不是客户端去追 latest。
- 下载后重算 sha256 并与钉值比对，失败就删掉文件。加载后先调 `jd_abi_layout`，与编译期期望的结构体大小比对，不一致就拒用并回退 Rime。
- 下载前弹确认框，显示：版本、大小、来源 URL、GPL-3.0、"会加载第三方原生代码"。
- 从不静默更新。KeyTao 升级带来新钉值时，只提示"可更新 libjd"，用户点了才下载。旧版本保留，直到新版本校验通过。

**引擎抽象（keytao-core）**
- 新增 `trait ImeBackend`，方法集合取 Windows `ImeEngineBackend`（`engine_backend.rs:47`）与 `EnglishBackend`（`english_mode.rs:86`）的并集：process_key、state、select、highlight、change_page、commit_composition、reset、set_option。
- 把 `ImeRuntimeSessionInner.engine: Option<Engine>` 改成 `Option<Box<dyn ImeBackend>>`。现在的 `Engine` 改名 `RimeBackend`，逻辑不动。只改 keytao-core 一处，FFI、JNI、Linux、Windows 都不用动，UI 原样复用 `ImeState`。
- `JdBackend` 用 libloading 按 `jd.h` 解析 16 个 `jd_*` 符号。iOS 走静态链接，用 cfg 区分，需要新增 libloading 依赖。
  - 前端自己维护已输入的键缓冲，拼成 preedit。
  - 候选由 `jd_read_range` 按 page_size 读取，翻页和选择用 `jd_set_anchor`，`committed` 由 commit_a + commit_b + commit_lit 拼出。
  - 能力通过 `EngineCapabilities` 声明。
  - 顶功、空格、`;`、标点都由 libjd 内部处理，KeyTao 侧不复写。
  - 每个 session 一个 ctx，持有 `Mutex`，和 jd.h:81 的线程约束一致。
- `ImeRuntime::init`、`reload`、`prewarm` 在 libjd 包下跳过 deploy、setup、lua、octagram 和 `RIME_API_LOCK`。schema 列表固定为一项"键道6 (ime-jd)"。英文模式由 KeyTao 的 english_mode 层直通，不进 libjd。

**状态与用户数据**
- 文件布局：`<data>/engines/libjd/<tag>/<os>-<arch>/libjd.*`，旁边放 `manifest.json`，内容为 {tag, asset, sha256, abi_layout, installed_at}。下载复用 `download_to_temp`（`download.rs:10`），校验复用 `downloaded_addon.rs` 的 Manifest 写法。
- 用户词：把 KeyTao 的自定义词（同步那一套）转成 `词\t编码`，再用 `jd_user_dict_build` 生成 blob 传给 `jd_init`，上限 10000 条，超出给出提示。改词后重建 ctx。
- libjd 不学习、不调频，所以没有 userdb，Rime 的 userdb 保持原样。切回 Rime 时数据零损失。
- 词库编译在 libjd 里，版本跟 libjd 发版走，不跟 KeyTao 包。UI 显示"词库随 libjd v0.13.0"。

**方案页 UI**
- 键道6 卡片下方放一个按钮："切换到 ime-jd 引擎"，或者"切回 Rime"。按下后依次是：确认框、带进度和取消的下载、校验、写入引擎选择、IME 重建 session。
- 失败（网络、sha、abi、dlopen）：保持或回退到 librime，toast 显示原因，下次可重试。IME 侧加载 libjd 失败也自动回退 Rime，并写日志。

**与包隔离重设计的关系**
- 引擎选择按包存：`PackageMeta` 加 `engine: Option<String>`，默认 rime（`packages.rs:19`，走 serde default）。只有 scheme_key 等于 keytao 的包允许设成 libjd。
- 这个值同步镜像到 `keytao-ime.json` 的 `engines: {<package>: "libjd"}`（照 `ime_settings.rs` 的模式），IME 在 create_session 和 rebuild 时读取。
- 两级 F4 列表里，包这一层显示引擎标记。libjd 包不进入"正在部署 <方案>"流程，构建缓存目录留空。
- packages.rs 和 lib.rs 都是热文件，必须排在隔离 B0–B5 之后串行做，或者至少等隔离批次释放这两个文件。

**批次（串行，每批可单独验收）**
- J0：纯重构。抽出 `ImeBackend`，`Engine` 改为 `RimeBackend`，换成 `Box<dyn>`，现有测试全部原样通过，用 golden 测试锁住 `ImeState`。
- J1：`JdBackend`（libloading + abi 校验）加离线测试。测试用 CI 按钉值拉 macOS 和 Linux 包，覆盖顶功、`;`、选择、翻页、backspace。先只做 macOS。
- J2：app-core 侧的钉值文件、下载、sha 校验、manifest、`PackageMeta.engine`、`keytao-ime.json` 镜像。
- J3：Flutter 方案页按钮、进度、确认框、失败回退。macOS 端到端验收。
- J4：Linux。
- J5：Windows，仅 Pipe 模式。
- J6：Android 侧载版（Play 版隐藏开关）。
- J7：iOS 构建期静态内置，加构建开关。

**需要你确认的问题**
1. 许可证：libjd 是 GPL-3.0，KeyTao 是 MIT。MIT 与 GPL 兼容，但 App Store 版和我们自己打包的二进制整体要按 GPL 履行义务（提供源码、写明许可）。这样能接受吗？要不要先和作者 hronro 确认一下运行时加载的做法，或者让对方给 KeyTao 一个额外许可？
2. "最新"是按 KeyTao 发版钉值（推荐），还是允许客户端追 GitHub latest？后者没有签名可校验，不建议。
3. 先支持哪些平台？建议 macOS，然后 Linux、Windows Pipe；iOS 和 Android Play 版暂不做。
4. 上游 0.x 阶段 ABI 不稳定、每周发版。是否接受每次更新钉值都要回归测试？
5. 用户自定义词是否同步给 libjd？上限 10000 条，超出部分怎么处理？
6. libjd 的词库与 KeyTao 当前词库版本可能不一致，需不需要在 UI 上提示？