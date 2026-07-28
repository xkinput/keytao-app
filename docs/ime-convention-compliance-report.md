# KeyTao 跨平台输入法规范符合度整改处置报告

整改日期：2026-07-26 起（跨平台规范符合度整改批次）

本报告记录本轮 KeyTao 输入法「跨平台规范符合度」整改的处置结果，覆盖核心 3 包（core-runtime / core-keyapi / theme）、FFI/JNI 包（ffi-jni）、五个平台包（linux-ibus / linux-wayland-xim / windows / macos / android / ios）以及编排方在七包之后单独派发的补充修复包。本报告是「整改处置报告」，不是评审报告；评审阶段的结论见 `docs/ime-implementation-review-report.md`，通用层规范见 `docs/ime-common-layer.md`（该文件已由上一环节同步，本报告只做引用，不改动其内容）。

## 一、背景与总览

### 1.1 整改范围与来源

- 本轮整改来源于 107 条经对抗性复核的审计发现，按严重度分布为 **P1×13 / P2×46 / P3×48**（13 + 46 + 48 = 107）。
- 整改成果按各工作包 `fixed` / `deferred` 列表汇总约为 **140 项修复、20 项 deferred**；按本报告第二节逐条统计的实际条目数详见「第二节区域小计」与文末数据汇总（部分 `fixed` 条目合并了多个 finding ID，例如 `core-13 + cross-11`、`macos-1 + cross-2`）。
- 整改被拆分为 11 个工作包并按依赖顺序推进：先行核心包 W0-A（core-runtime）、W0-B1（core-keyapi）、W0-C（theme）并行；随后 W0-B2（ffi-jni）；再后五平台包 W1-linux-1（linux-ibus）、W1-linux-2（linux-wayland-xim）、W1-win（windows）、W1-mac（macos）、W1-android（android）、W1-ios（ios）；最后编排方单独派发补充修复包收口 iOS 编译与原子写两处跨包遗留。
- **严重度口径说明（待核）**：输入材料（design.md / upstream-w0.md / doc-notes-all.md）的 `fixed` / `deferred` 列表未对每一条 finding 标注 P1/P2/P3 级别，故本报告第二节「代表性条目」按材料中描述的实际影响（含材料显式标注为「P1」「本包最高优先级的真实故障」等语的条目）挑选，无法逐条断言「某区域所有 P1」；项目级严重度分布（P1×13 / P2×46 / P3×48）为背景材料给出的总量。

### 1.2 严重度分布与整改成果

| 项目 | 数量 | 口径 |
| --- | ---: | --- |
| 审计发现总数 | 107 | 经对抗性复核 |
| P1（应优先修复） | 13 | 真实输入行为错误 / 部署后状态不刷新等 |
| P2（中期修复与设计债） | 46 | 平台漂移、误操作入口、工程化缺口 |
| P3（长期改进方向） | 48 | 收敛重复逻辑、诊断能力、测试补齐 |
| 跨切面设计决策 | 12 | D1–D12（见 1.3） |
| 修复项（各包 fixed 汇总） | 约 140 | 详见第二节与文末小计（逐条实测 141） |
| deferred（各包 deferred 汇总） | 约 20 | 详见第二节与文末小计（逐条实测 27，含 N/A 与后由补充修复包解决项） |

### 1.3 跨切面设计决策 D1–D12（每条一句话摘要）

- **D1 reload 语义**：librime 的 config/词典缓存是进程级 `weak_ptr`，reload 必须丢弃本 runtime 全部存活 session 的 Engine，再 `RimeFinalize` + 重新 `setup/initialize` 并递增 generation；reload stamp 的路径、格式、签名与变更检测统一收进 keytao-core 的 `ReloadStamp` / `ReloadStampWatcher`。
- **D2 线程模型**：librime C API 无线程安全承诺，keytao-core 用进程级可重入锁 `RIME_API_LOCK` 序列化全部 rime_api 调用（含 create/destroy session、setup/deploy/finalize、Engine 全部方法与 Drop）。
- **D3 FFI/JNI panic 边界**：keytao-core-ffi 每个 `extern "C"` 与 src-tauri 全部 `Java_*` JNI 导出用 `catch_unwind(AssertUnwindSafe)` 包裹，panic 返回 null/false/0 并记 error 日志、不 abort 进程；core 内 `lock().unwrap()` 一律改为抗中毒。
- **D4 候选/翻页/清空用官方 API**：弃用合成按键，改用 `RimeSelectCandidateOnCurrentPage` / `RimeHighlightCandidateOnCurrentPage` / `RimeChangePage` / `RimeCommitComposition` / `RimeClearComposition` / `RimeDeleteCandidateOnCurrentPage`；删除恒空的 `all_candidates` 字段。
- **D5 Enter/数字键语义统一**：有 composition 时 Enter 一律把 `XK_Return` 交给 Rime（`process_enter`），未接受再由 core fallback 到 `commit_raw_input`；平台不得本地拦截数字键选词，`select_keys` 为空时不做 `"1234567890"` 兜底。
- **D6 ascii_mode 与按键旁路**：任何模式下按键都先进 Rime，未接受再放行；Ctrl/Alt 组合先送 Rime；session 创建/重建不再强制 `ascii_mode=false`，refresh 时保留旧值；CapsLock 由 `normalize_key_for_modifiers` 统一折进 keysym。
- **D7 modifier mask**：mask 白名单加入 Super/Meta/Hyper 位；macOS 的 Command、Windows 的 Win 键统一映射为 Super；CapsLock 不进 mask。
- **D8 光标/选区单位**：`ImeState` 的 `cursor` / `sel_start` / `sel_end` 一律定义为 Unicode 标量（char）偏移，core 从 librime 的 UTF-8 字节偏移换算；平台各自换到原生单位（macOS/Windows/Android → UTF-16，IBus → char，Wayland → UTF-8 字节）。
- **D9 密码/敏感输入策略**：core 新增 `InputContextPolicy { composing, learning }` 与 `set_input_policy`；`composing=false` 时可打印键直通宿主、不组字、不学习；各平台按 IBus content-type、Wayland content_type、TSF compartment/InputScope、Android inputType、iOS `isSecureTextEntry` 检测。
- **D10 key_policy / text→keysym 收敛到 FFI**：core 的 `key_policy` 与 text→keysym 映射经 FFI/JNI 暴露，macOS/iOS(Swift) 与 Android(Kotlin) 删除各自分叉实现；补 Android ASCII 守卫与 macOS 小键盘 Enter 映射。
- **D11 theme 分层**：移动端软键盘布局从共享 `ResolvedImeTheme` 挪到独立的 `mobile_layout` 模块；FFI theme 查询改用带缓存的 `ThemeResolver`（按 stamp/mtime 失效）；macOS 改消费 keytao-theme 的 `CandidatePanelModel` / `ModeHintModel`。
- **D12 日志卫生**：Linux 日志移出 `/tmp` 固定路径，放 `$XDG_STATE_HOME/keytao`（0700）；提交文本/按键/keysym 只允许 trace/debug 级且 release 默认不开；全平台 release 路径不打按键与提交内容。

## 二、处置明细（按区域组织）

区域与工作包的对应关系以 design.md 的工作包定义为准；明细内容以 upstream-w0.md 与 doc-notes-all.md 为准。各区域「代表性条目」按材料描述的实际影响挑选（材料未逐条标注 P1/P2/P3，见 1.1 待核说明）；`deferred` 条目全部显性列出，不省略。

### 2.1 core-runtime（W0-A，`crates/keytao-core`）

- **fixed：6 条**（core-1、core-2、core-3、core-9、core-12、core-13+cross-11 的 core 部分）。**deferred：0 条**。

代表性条目：

- **core-1（D1 reload）**：桌面 `OnceLock` 短路改为可重入的 `RIME_INITIALIZED: Mutex<bool>`，把 Android 的 finalize+initialize 泛化为全平台 `reinitialize()`；`ImeRuntime` 增加 `Vec<Weak<Mutex<SessionInner>>>` 注册表与 RwLock reload 屏障，`reload_without_deploy` / `reload` 先丢弃全部存活 session 的 Engine 再 finalize/initialize 再递增 generation；`refresh_if_needed` 改为先 drop 旧 Engine 再建新 Engine。
- **core-2（D2 线程模型）**：新增进程级可重入锁 `RIME_API_LOCK`（thread_local 深度计数、抗中毒），Engine 全部方法与 Drop、create_session、setup/deploy/finalize、maintenance 等全部 rime_api 调用点入锁；修正错误的 SAFETY 注释为「由全局锁序列化」。
- **core-3（D3 core 部分）**：keytao-core 内不再有 `lock().unwrap()`，新增 `lock_ignore_poison` / `read_ignore_poison` / `write_ignore_poison`（`PoisonError::into_inner`）；中毒锁不再让 panic 跨 FFI 边界 abort。
- **core-12（不丢字）**：`extract_state` 拆为 `extract_state_with_commit`（变更类 API 用）与 `extract_state_readonly`（`state()` 用，不调 `RimeGetCommit`）；`ImeState.committed` 在 `state()` 结果中恒为 `None`，消除 macOS `refreshSessionState` 静默吞 pending commit 的丢字路径。
- **core-13 + cross-11（core 部分）**：在 keytao-core 提供唯一实现 `RELOAD_STAMP_FILE_NAME` + `ReloadStamp` / `ReloadStampWatcher`，签名统一为 `<len>:<mtime_nanos>:<fnv1a64 hex>`，stamp 缺失不算 reload 请求。
- **core-9（D6 ascii_mode）**：删除 `Engine::new_with_user_data_dir` 里无条件的 `set_session_option(ascii_mode, false)`；reload 重建 Engine 前记下 `is_ascii_mode()`、重建后经 `apply_ascii_mode` 写回（不回读状态以免吞 commit）。

deferred：无。

### 2.2 core-keyapi（W0-B1，`crates/keytao-core`）

- **fixed：14 条**（core-4、core-5、core-6、core-7、core-8、core-10、core-11、cross-4 core、cross-5 core、cross-6 core、cross-12 core、D9 core、D10 core、macos-12 前置）。**deferred：2 条**。

代表性条目：

- **core-4 / cross-12（D4 候选/翻页/清空）**：新增 `select_candidate_on_page` / `highlight_candidate_on_page` / `delete_candidate_on_page`，`change_page` 改调 `(*api).change_page`，`reset` 改为 `clear_composition` 的别名；仅在函数指针缺失（旧 ABI）时才退回合成按键，避免「选不中就把数字打进编码」。
- **core-5 / cross-5（D5 Enter）**：新增 `commit_composition` / `clear_composition` / `raw_input` / `commit_raw_input`，以及五端唯一的 `process_enter()`（先把 `XK_Return` 交给 Rime，未接受且确实在组字时才 fallback `commit_raw_input`，且保留 Rime 已产出的 commit 不丢字）。
- **core-11（删除恒空 all_candidates）**：删除 `ImeState.all_candidates` 及其赋值，按需拉取的 `all_candidates_limited()` 保留；同处最小越界改掉 FFI JSON 的 `allCandidates` 与 `AndroidImeStateJson.all_candidates`（正式收口归 W0-B2）。
- **core-8（D6 bypass）**：删掉 `should_bypass_empty_composition_key` 里「mask 含 Control/Alt 就整段 bypass」的分支，改为只对系统保留修饰键（Super/Hyper/Meta）提前放行；Ctrl+grave、F4 等现在都先送 Rime。
- **core-6 + macos-12 前置（D8 偏移单位）**：`ImeState.cursor` 单位定死为 Unicode 标量偏移，新增 `char_offset_from_utf8` / `utf16_offset_from_chars`；新增 `sel_start` / `sel_end`（取自 RimeComposition，同为 Unicode 标量）供 macOS 分段下划线。
- **D9 core（InputContextPolicy）**：新增 `InputContextPolicy { composing, learning }` 与 `set_input_policy()`；`composing=false` 时 `process_key_result` 完全不把按键交给 librime，直接返回 `accepted=false` + 只读快照，天然无 preedit/候选/用户词学习。

deferred：

- **D9 core — `InputContextPolicy.learning` 的独立强制**：librime 没有「按 session 关闭用户词记忆」的开关（memorize 是 translator 的 schema 级配置），`learning=false` 目前只能靠 `composing=false`「不向 Rime 送任何组字键」天然满足；单独把 learning 置 false 而保持 composing=true 时 core 无法强制，平台需自律约束剪贴板/输入历史。
- **新 Engine API 的自动化单元测试**：`select_candidate_on_page` / `change_page` / `commit_composition` / `clear_composition` / `commit_raw_input` / `process_enter` 全部要求真实 librime 与已部署用户目录，CI/无数据环境拿不到 session，故改为 `examples/candidate_api_smoke.rs` 手动冒烟；纯逻辑部分（key_policy、偏移换算、policy 默认值）已全部单测覆盖。

### 2.3 theme（W0-C，`crates/keytao-theme`）

- **fixed：2 条**（core-14、cross-10）。**deferred：0 条**。

代表性条目：

- **core-14（D11 mobile_layout 分家）**：新增 `crates/keytao-theme/src/mobile_layout.rs`，把 `KeyboardTheme` 系列类型全套迁入并改名为 `MobileLayout` / `MobileKey` / `MobileCommand` 等；`ResolvedImeTheme` 删除 `keyboard` 字段，`keytao_resolve_theme_json` 产出的 JSON 不再有 `keyboard` 键；同时删掉 `default-theme.yaml` 里那份 129 行手机键盘布局。
- **cross-10（共享层污染点收窄）**：`PartialTheme.keyboard` 降级为只喂 mobile_layout 的兼容入口，`PartialThemeVariant.keyboard` 直接删除（`light:` / `dark:` 变体不再能覆盖键盘布局）；crate 根保留 `KeyboardTheme` / `resolve_keyboard_from_paths` / `resolved_keyboard_json` 等别名，保证 keytao-core-ffi 与 src-tauri 零改动即可编译；README 补 `theme.yaml` 与 `keyboard.yaml` 的职责表与迁移说明。

deferred：无。

### 2.4 ffi-jni（W0-B2，`crates/keytao-core-ffi` + `src-tauri/src`）

- **fixed：14 条**（core-3 FFI、android-15、core-15、core-11 FFI 收口、core-13+cross-11 stamp FFI、D1 FFI、cross-13、cross-14、D10 FFI+D8、D9 FFI、D4/D5 FFI、macos-12 前置、cross-9 前置、Android 目标编译噪声清理）。**deferred：4 条**。

代表性条目：

- **core-3（FFI 侧）+ android-15（JNI 侧）（D3 panic 边界）**：新增 `guard(name, default, body)` 包住 keytao-core-ffi 的**全部 56 个** `extern "C"` 导出；新增 `android_jni_guard` + android_log 模块包住**全部 33 个** `Java_*` JNI 导出，panic → 返回 null/false/0 并写 logcat error，不再 abort `:ime` 进程；日志只打 panic 消息，不打按键与提交内容（D12）。
- **core-13 + cross-11（stamp FFI + App 写入端收口）（D1）**：FFI 暴露五个 stamp 入口（`keytao_reload_stamp_path/_signature`、`_path_at/_signature_at`、`keytao_reload_stamp_changed`、`keytao_reload_if_stamp_changed`），JNI 对称暴露 `nativeReloadStampSignature/Path`；App 侧删除三套手写时间戳/签名逻辑，全部改调 `keytao_core::ReloadStamp`。
- **core-15（FFI theme 缓存）（D11）**：`THEME_PATHS` 换成持 `keytao_theme::ThemeResolver`（带签名缓存），按键 JSON 路径不再每次重解析内置 theme.yaml；新增 `keytao_set_system_color_scheme` 让平台注入系统配色，消除 IME 进程内 fork `defaults`/`gsettings`。
- **D4/D5 FFI 暴露**：透出 `keytao_session_process_enter` / `highlight_candidate` / `delete_candidate` / `commit_composition` / `clear_composition`（各配 `_json` 版），JNI 对称新增 `nativeProcessEnter` 等；所有导出只转发，不含平台策略。
- **cross-9 前置（UI 能力可声明）**：新增 `keytao_set_ui_capabilities`（6 个 bool），修掉 `state_json` 里 `supports_vertical` 硬编码 false 的问题——macOS 声明 `supports_vertical=true` 才能拿到竖排候选模型，iOS 默认值零变化。
- **cross-13 / cross-14（text→keysym / key_policy 过 FFI）（D10）**：暴露 `keytao_text_to_keysym`、`keytao_key_policy_is_enter`、`keytao_key_policy_should_bypass` 与对称 JNI，Swift/Kotlin 侧不再各自维护 keysym 常量表。

deferred：

- **iOS target 编译失败（keytao-core，非本包 ownership）**：`cargo check -p keytao-core-ffi --target aarch64-apple-ios` 报两个 E0609，根因是 vendored iOS librime 为 1.8.5（源码 commit 08dd95f5），其 `rime_api_t` 缺 `change_page` 与 `highlight_candidate_on_current_page`，而 keytao-core 直接取这两个字段——是**编译期**问题而非运行期 Option 判空。**此项后由补充修复包解决**（见 2.11）。
- **`src-tauri/src/rime.rs` 与 App 进程内的裸 Engine**：App 自己的 overlay 输入通道「先 `Engine::new()` 再覆盖旧 Engine」同样会命中 librime 旧缓存，且不在 `ImeRuntime` 注册表内、不受 reload 保护；但它既非 JNI 也非 FFI，迁移会改掉 4 个 Tauri command 的行为，超出「最小必要」，登记建议单独派活。
- **`InputContextPolicy.learning` 在 FFI 层的强制**：沿用 core 结论，FFI/JNI 只能如实透传该标志，通用层无法再往前一步。
- **JSON 路径未注入系统配色时仍会走 keytao-theme 探测**：`keytao_set_system_color_scheme` 是可选的；不调用时 fallback 到 keytao-theme 的 `defaults`/`gsettings` 子进程探测（1 秒节流），删掉会让未接入平台配色恒为 Light，属行为回退，故保留。

### 2.5 linux-ibus（W1-linux-1，`crates/keytao-linux-ime` IBus/GNOME 路径）

- **fixed：10 条**（linux-1、linux-2、linux-6、linux-11、linux-12、linux-14、linux-18、cross-15、D9-ibus、D5/D6-ibus）。**deferred：2 条**。

代表性条目：

- **linux-2（D12 日志卫生）**：日志目录从 `/tmp` 固定名改到 `$XDG_STATE_HOME/keytao/log`（默认 `~/.local/state/keytao/log`），`DirBuilder` 0700 建目录，打不开日志退回 stderr 而非 panic，启动时清掉旧版 `/tmp` 日志；含用户输入的 `info!` 全部降级（commit/preedit 全文仅 trace，debug 只打字符数，keysym/keyval/keycode 一律 trace）。
- **linux-1（IBus 信号签名）**：`org.freedesktop.IBus.Engine.UpdatePreeditText` 补上第 4 个 `IBusPreeditFocusMode` 参数，body 签名从 `(vub)` 改为 `(vubu)`，三个调用点统一传 `IBUS_ENGINE_PREEDIT_CLEAR(0)`；核对并写明 InputContext 版仍是 3 参数、两者不能混。
- **linux-14（IBus 组件注册）**：新增 `keytao.xml` 并安装到 `/usr/share/ibus/component/keytao.xml`（deb/rpm/Nix 三条打包路径 + `verify-linux-bundles.sh` 校验）；引擎连接改到 ibus 私有总线（`IBUS_ADDRESS` / `~/.config/ibus/bus/*`，带自连保护），并在该连接上 `request_name(org.freedesktop.IBus.KeyTao)`。
- **D9-ibus（content-type 密码框）**：IBus shim 与 GNOME engine 把 `ContentType` 实现为 `(uu)` 属性（走 `Properties.Set`），`purpose==PASSWORD(8)||PIN(9)|| (hints & PRIVATE(1<<11))` 映射为 `InputContextPolicy::sensitive()`；两处 `ProcessKeyEvent` 第一件事就检查 `input_policy().composing`，为 false 直接 return false。**XIM 协议不表达输入用途，XIM 路径无法检测密码框**，已在 IMPL.md 写明。
- **D5/D6-ibus（Enter 与旁路）**：删掉 IBus shim 与 GNOME engine 里「有 preedit 就提交 preedit 字面量 + reset」的私有实现，改调唯一的 `ImeSession::process_enter()`；删掉数字/空格前置拦截（一律送 Rime）；bypass 判定唯一来源是 core 的 `should_bypass_empty_composition`。
- **linux-18（系统指示器跟随中英）**：GNOME engine 在 FocusIn/Enable 按 session 真实 `ascii_mode` 发 `RegisterProperties`、变化时发 `UpdateProperty`；IBus shim / kimpanel 属性串收敛为 `kimpanel::mode_property(ascii_mode)` 一处实现。

deferred：

- **cross-15 的 CI 部分（Linux target 的 `cargo test` 进 CI）**：仓库只有 `.github/workflows/release.yml` 一个 tag 触发的发版工作流，没有任何 test/CI 工作流可追加 job；新建 CI 工作流不在本包 ownership 且影响全仓门禁，属主会话决策。本轮已在 IMPL.md 写明 macOS/Windows host 上 `cargo test -p keytao-linux-ime` 会跳过全部后端代码，并给出容器内跑法，且已用 Linux 容器实测通过。
- **linux-1 的 `IBUS_ENGINE_PREEDIT_COMMIT(1)` 分支**：不是跳过，是明确选择只发 `CLEAR(0)`——按 D4/cross-2 的失焦处置矩阵，本引擎在 focus_out/reset/disable 里自己 `clear_composition()`，客户端不应再替它提交，同时发 COMMIT 会造成重复上屏；将来真引入「失焦提交」策略时再按上下文选 mode。

### 2.6 linux-wayland-xim（W1-linux-2，`crates/keytao-linux-ime` Wayland/XIM 路径）

- **fixed：13 条**（linux-3、linux-4、linux-5、linux-7、linux-8、linux-9、linux-13、linux-15、linux-16、linux-17、linux-19、cross-7、cross-5 本包三处 Enter）。**deferred：3 条**。

代表性条目：

- **linux-3（D9 Wayland content_type）**：v2（text-input-v3 编号）与 v1（text-input-v1 编号）两条通道各自接入 content_type，v2 双缓冲在 done 时应用、v1 收到即应用且每次 Activate 显式复位为 default 避免密码框策略泄漏；三套常量表严格分开，只共享 `sensitive()` 这个结论。
- **linux-9（v2 状态应用顺序，防丢 preedit）**：抽出唯一的 `apply_state_to_input_method(&ImeState)`（`commit_string → set_preedit_string → commit(serial)`），空格选词私有分支随 D5 删除，`accepted=false` 也先应用一次状态再转发按键（librime 拒键的同时可能 flush commit，直接转发会丢字）。
- **linux-19（reload 广播）**：新增 `src/reload_bus.rs`，每个 poll 型后端启动时挂一个 `eventfd`，watcher 在 `reload_without_deploy()` 成功后 `notify()` 唤醒全部订阅者执行与失焦相同的清理；XIM 侧新增 `reload_epoch`/`state_epoch`，避免 reload 后一次 `SetICValues` 把旧词库候选重画回来。
- **cross-7（XIM 订阅 KeyRelease）**：`filter_events()` 与 `set_event_mask` 统一为 `KeyPress|KeyRelease=3`，KeyRelease 只在 solo Shift 时把 keysym+`RIME_RELEASE_MASK` 送 Rime 让 ascii_composer 切中英；配套修了 Shift 抬起时 keysym 解析成 NoSymbol 的坑（新增 `keysym_at_level()`）。
- **linux-16（IME 抓键盘后自实现按键重复）**：两个后端各加 `repeat_delay/repeat_interval`，被吃掉的键按 `xkb_keymap_key_repeats` 判定是否重复；KWin 的 grab 键盘是 wl_keyboard v1（早于 repeat_info v4），按 xkb/Plasma 默认值（delay 600ms / rate 25）兜底。
- **cross-5（本包三处 Enter）**：删掉 Wayland v2 / KDE v1 / XIM 三处「有 preedit 就提交字面量 + reset」的私有实现，改调 `process_enter()`，是 W1-linux-1 交接单点名要求本包完成的部分。

deferred：

- **linux-19 的 IBus shim / GNOME engine 分支**：这两个后端是 zbus/tokio 异步事件循环，没有可直接挂 eventfd 的地方，且属 W1-linux-1 的 ownership，本包不能改其事件循环；当前两者仍靠 generation 懒刷新（残留窗口比 poll 型后端长）。已在 IMPL.md 标注「未接入」。
- **linux-5 的 `UpdateSpotLocation` / impanel2 `SetSpotRect` 实现**：input-method-unstable-v1 客观上不提供任何光标矩形，KDE 私有路径拿不到坐标，硬发只能发假值；按 finding 的 ADJUSTED 结论处理为文档化偏离，主候选 UI 是 KWin 定位的 overlay、不受影响。
- **linux-13 / linux-15 / linux-17 的自动化回归测试**：核心逻辑直接耦合在 wayland_client 的 Dispatch trait 与 xim 的 ServerHandler 上，无法在 `#[cfg(test)]` 里构造；能抽成纯函数的部分（`repeat_timings`、`content_type_policy`、`preedit_cursor_bytes`、`keysym_at_level`、filter mask 常量）已单测覆盖，协议层行为改为 IMPL.md 写死可复核要点。

### 2.7 windows（W1-win，`crates/keytao-windows-ime`）

- **fixed：19 条**（windows-1、windows-2、windows-3、windows-4、windows-5、windows-6、windows-8、windows-10、windows-11、windows-12、windows-13、cross-3、cross-5、cross-6、cross-8、cross-4 win、cross-11 win/D1、D8 win、D9 win）。**deferred：3 条**。

代表性条目：

- **windows-3 + D9（密码框直通）**：新建 `input_context` 模块，读 `GUID_COMPARTMENT_KEYBOARD_DISABLED` / `EMPTYCONTEXT`（焦点文档为空同样算禁用），焦点变化时用同步只读 edit session 查 `GUID_PROP_INPUTSCOPE` 的 `IS_PASSWORD`；命中即 `set_input_policy(sensitive())`，四个按键回调开头统一短路。
- **cross-3 / windows-2（TestKeyDown 超集 + accepted 决定吞否）**：`OnTestKeyDown` 刻意放宽为「所有能到 librime 的键都声明拦截」，真正放行下沉到 `OnKeyDown`，由「accepted || 产生 commit || preedit/候选/高亮/页码确实变化」决定；同时把 `should_consume_processed_state` 改严，避免有 preedit 时吞掉 Rime 拒收的 Ctrl+C/Ctrl+A（本轮风险最高的行为变更，需重点回归）。
- **windows-6（caret 定位 + COM 泄漏）**：`probe_caret` 在 `TF_E_NOLAYOUT`/GetSelection 失败时 `position=None` 而非硬编码 `(100,100)`，`resolve_caret` 按探测→缓存→系统 caret→宿主窗口左上角逐级回退，composition 可见时 AdviseSink `ITfTextLayoutSink` 跟随滚动；顺带修掉 GetSelection 返回的 `ManuallyDrop` range 未释放的引用泄漏。
- **windows-11（鼠标点击候选/翻页）**：`panel.rs` render 返回带 `hit_areas` 的 `RenderedPanel`，候选与翻页按钮命中矩形与绘制同源，`WM_LBUTTONUP` 命中后回到 STA 调 `select_candidate_on_page` / `change_page`，`WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE` 保证不抢焦点。
- **windows-8 + D6/D7（CapsLock/Super mask）**：`current_mod_mask` 增加 `RIME_MOD_LOCK`（CapsLock）与 `RIME_MOD_SUPER`（Win 键），字母 keysym 按 caps XOR shift 选大小写；带 Control 的可打印键跳过 `ToUnicodeEx` 避免 Ctrl+[ 被折成 U+001B。
- **cross-11 win / D1（stamp 收敛）**：删除 Windows 私有 `reload_stamp_signature`，改用 `keytao_core::ReloadStamp`，stamp 缺失不再误触发 reload，按键路径 stat 加 250ms 节流，focus/context/thread focus 回调强制失效缓存。

deferred：

- **windows-9（`GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT`）**：是能力声明而非协议实现，注册前提是 AppContainer 下 shared data 可读、user data 可写，这两项在打包/安装器范围（scripts/、NSIS、src-tauri），不在本包 ownership；前提未满足时抢先注册比不注册更危险。已从能力矩阵一行升级为 IMPL.md《与通用层的已知偏离》完整条目。
- **windows-10 的 context→session 映射（长期方案）**：design.md 明确允许退路，本轮取退路（保持「context 切换即 reset session」现状 + IMPL.md 登记偏离）；理由是按 `ITfDocumentMgr` 建/销 session 必须在 TSF 回调所在 UI 线程同步进 librime，与本包「TSF 回调线程不进 librime」硬约束冲突，且 finding 自核实际用户影响为零。
- **`sel_start` / `sel_end` 的分段 composition 显示属性**：D8 只要求 macOS 据此做分段下划线；Windows 要区分已转换/未转换段需新增第二个 display attribute GUID 并同步改注册表与发布验证脚本，属新增能力而非本轮 finding 要求，未做，记录于此以免被当成遗漏。

> **待核**：windows-7 未在本包 `fixed` / `deferred` 列表中显式出现；design.md 记 W1-win 范围为「windows 全部 12 条 + cross 若干」，而材料的 windows finding 编号出现到 windows-13。此处以材料为准，windows-7 归属待核。

### 2.8 macos（W1-mac，`crates/keytao-macos-ime`）

- **fixed：20 条**（macos-1+cross-2、macos-2、macos-3、macos-5、macos-6+D8、macos-7、macos-8、macos-9、macos-10+cross-14、macos-11、macos-12+D8、macos-13、macos-14、macos-15、macos-16+cross-6、cross-9、D1/cross-11、D5、D7+D10、D12）。**deferred：4 条**。

代表性条目：

- **macos-1 + cross-2（deactivateServer 交还 composition）**：新增统一的 `endComposition(commit:client:)`（commit 走 `commit_composition_json`、丢弃走 `clear_composition_json`），`deactivateServer` 有 composition 时走提交路径；按 D4/D5 失焦处置矩阵，不再伪造 `XK_Return`/`XK_Escape`。
- **macos-2（Cmd+Shift 误触发中英切换）**：任何 keyDown 无条件 `shiftPressedWithoutKey=false` 且提到 Command 早退之前；`handleFlagsChanged` 里新 flags 出现 Command/Control/Option 就立刻清标志，Shift 按下只在无其它修饰键时置位。
- **macos-11（初始化移出 IMK 同步回调线程）**：新增 `KeyTaoRuntime` 单例，`keytao_init` 丢到专属串行队列并带 2 秒退避；controller 未就绪时 session 为 nil、直接放行按键并异步 `requestInitialization()`，runtime 起来后主线程广播补建 session。
- **cross-9（消费 CandidatePanelModel/ModeHintModel）**：macOS 全面改吃 FFI 的 `_json` 状态，删除 Swift 里 `Array(selectKeys.isEmpty ? "1234567890" : ...)`、highlight clamp、label 拼接、翻页判定等私有逻辑；启动时 `keytao_set_ui_capabilities(true×5, false)` 声明可竖排，并用 `keytao_set_theme_paths` / `keytao_set_system_color_scheme` 注入主题与配色。
- **macos-14（删除 killall cfprefsd）**：删除全部 4 处 `killall cfprefsd`，保留 KeyTaoIME/imklaunchagent/TextInputMenuAgent 重启与 `lsregister -f` + TIS 注册；`verify-macos-pkg.sh` 增加守门断言。
- **macos-13（IME bundle 版本）**：`build.sh` 新增版本解析与 `numeric_bundle_version`，构建时用 PlistBuddy 写 `CFBundleShortVersionString`/`CFBundleVersion`，IME-only pkgbuild 版本从写死的 1.0.0 改为随 workspace 版本；`verify-macos-pkg.sh` 断言 bundle 版本 = pkg 版本。
- **D1 / cross-11（stamp 出按键热路径）**：删除 Swift 侧私有 stamp 逻辑，改后台队列上的 DispatchSource——stamp 存在时监听**文件**、不存在时监听**目录**（实测证明原地重写不触发目录事件），按键路径无任何文件 I/O。

deferred：

- **macos-3 的 `collectionBehavior` 部分**：finding 自身 ADJUSTED 结论已降为「可选优化项，需实测跨 Space/全屏是否失效后再定」，业界基线 Squirrel 亦未设置；本环境无法实测，盲加可能改变现有显示行为。`windowLevel` 部分（真正的规范依据）已修。
- **macos-7 的「恢复鼠标点击提交」分支**：ADJUSTED 二选一中选了「删死代码 + 修文档」；另一条（`recognizedEvents` 加 `leftMouseDown`）要求实测回调可达，声明鼠标 mask 会让每次左键都经 IMK 同步回调，本环境无可交互 IMK 客户端。
- **macos-13 的 `macos_ime_status` 返回 IME bundle 版本**：fix_sketch 后半要求改 `src-tauri/src/lib.rs` 的 `macos_ime_status_inner`，该文件属 W0-B2 ownership，不宜再越界；版本写入（真正根因）已在 build.sh 完成并有 verify 守门。
- **候选窗 hover 高亮接 `keytao_session_highlight_candidate_json`**：非本轮 finding，接入需重新设计 hover→core→重绘时序，会影响后续 Enter/空格选中项，风险大于收益；FFI 入口已具备。

### 2.9 android（W1-android，`src-tauri/gen/android/app`）

- **fixed：20 条**（android-1、android-2、android-3+D9、android-4、android-5、android-6、android-7、android-8、android-9、android-10、android-11、android-12、android-13、android-14、android-15、android-16、android-17 subtypeId、cross-13 android、cross-14 android+D6、D4/D5/D1 接线+core-11 Kotlin）。**deferred：3 条**。

代表性条目：

- **android-1（Scoped Storage）**：`userRoot()` 改为 `getExternalFilesDir(null)/keytao`（不可用时 `filesDir/keytao`），Manifest 删掉 `MANAGE_EXTERNAL_STORAGE` / `READ_`/`WRITE_EXTERNAL_STORAGE` 与 `requestLegacyExternalStorage`，`ScopedStoragePlugin` 删掉「所有文件访问」引导，新增 `migrateLegacyRootIfNeeded()` 一次性后台迁移。
- **android-3 + D9（密码框/隐私）**：新增 `resolvePrivacyMode(inputType, imeOptions)`，password 变体 → `setInputPolicy(composing=false, learning=false)` 走 JNI 直通，同时清空并禁用 `clipboardHistory`/`recentCommittedUnits`/`backspaceRestoreStack`。
- **android-5（生命周期回调不阻塞）**：`KeytaoImeEngine` 新增单线程 `backgroundExecutor`，目录探针 + `nativeInit` + `reloadIfNeeded` 全部投后台再 post 回主线程；`KeytaoThemeResolver` / `KeytaoAndroidImeConfig` 按 `(length, mtime[, 深浅色])` 签名缓存。
- **android-13（无障碍）**：用 androidx.customview 的 `ExploreByTouchHelper` 给自绘键盘补虚拟节点（复用触摸命中的 `keyRects`/`candidateRects` 等），每个节点给 `contentDescription`/bounds/`ACTION_CLICK`，激活时走与触摸相同的路径；未新增 Gradle 依赖。
- **android-16（部署前台服务）**：`onStartCommand` 一进来就 `startForeground`（`dataSync` 类型）显示「正在编译词库」通知，Manifest 补 `foregroundServiceType=dataSync` 与相关权限，编译结束先 `stopForeground` 再 `killProcess`（归还 librime 编译器 native heap，已在 IMPL.md 登记为偏离）。
- **cross-14 android + D6 / D4/D5/D1 接线**：删除 Kotlin 侧手写的 `shouldBypassHardwareKey` 白名单与「ascii_mode 整段绕过 core」私货，改调 `nativeShouldBypassKey`；补齐 14 个 `external fun`，Enter 走 `process_enter()`、失焦丢弃走 `clear_composition()`、删 `allCandidates` 死分支。

deferred：

- **android-17 的 `directBootAware` 部分**：真正落地需 device-protected 数据目录、`isUserUnlocked()==false` 时降级纯 ASCII 直通、shared data 三条链路支持双目录，改造面覆盖本包几乎全部路径，收益只在「首次解锁前输密码」；已在 IMPL.md 写明。subtypeId 部分已完成。
- **D8 android（preedit cursor → composing selection）**：`BaseInputConnection` 对 `setComposingText` 的实现无法表达「光标落在 composing 区间内部」，精确落点需自跟踪绝对起点再 `setSelection()`，各家编辑器（尤其 WebView）行为不一，属需真机逐个验证的改造；当前把光标放末尾对键道类方案是正确表现。
- **android-8 的 repeat 期间上下文缓存**：「降到 2048」与「复用 BreakIterator」已实现，「repeat 期间本地推进缓存」未做——退格 repeat / 手势批量删除 / 恢复栈 / 宿主改写文本四者耦合，本地缓存一旦失步就删错字符。
- **偏离登记（D9 Android，唯一一处偏离 design.md）**：`TYPE_TEXT_FLAG_NO_SUGGESTIONS` 不走完全直通，降级为「关用户词学习 + 关剪贴板记忆/建议 + 不留输入历史」但**保留组字能力**；理由是 `textNoSuggestions` 被大量普通字段（用户名、编号）使用，完全直通会让用户在这些字段里无法输入中文，AOSP LatinIME 亦只据此关建议与词典学习。密码框契约无任何放宽。

### 2.10 ios（W1-ios，`crates/keytao-ios-ime`）

- **fixed：21 条**（core-11/W0-B1 适配、ios-1、ios-2、ios-3、ios-4+D9、ios-5、ios-6、ios-7、ios-8、ios-9、ios-10、ios-11、ios-12、ios-13、ios-14、ios-15、cross-13+D10、cross-5+D5、cross-11+D1、D9 iOS、cross-10 iOS 收尾）。**deferred：6 条**。

代表性条目：

- **core-11 / W0-B1 破坏性变更适配（本包最高优先级的真实故障）**：W0-B1 从 state JSON 删掉 `allCandidates`，而 `KeyTaoImeState.allCandidates` 是非可选字段，`JSONDecoder` 会整份解码失败、键盘完全拿不到状态；删该字段与死分支、补 `selStart/selEnd`，并把 state 改成逐字段 `decodeIfPresent` + 默认值容错解码。
- **ios-1（P1 地球键）**：不改 keyboard.yaml（不在 ownership），在 View 层强制注入——`activeRows()` 末尾过 `applyInputModeSwitchKey()` 在 `needsInputModeSwitchKey` 为 true 且当前层无切换键时注入 🌐；VC 在该键位放透明按钮把 `handleInputModeList(from:with:)` 绑到 `.allTouchEvents`（轻点切下一键盘、长按弹选择器）。
- **ios-2（宿主 marked text 应用顺序）**：`apply()` 按 ime-common-layer 顺序（先清 marked text 再 `insertText`，再 `setMarkedText(preedit, selectedRange:)`）；关键坑：`unmarkText()` 只把 marked text 定稿而不删除，清 preedit 必须先 `setMarkedText("")` 再 `unmarkText()`，所有宿主写入经 `withHostTextMutation` 屏蔽自触发回调。
- **ios-6（异步初始化不丢字）**：`ensureReadyAsync()` 把种子写入 + `keytao_init` + `create_session` 放专用串行队列，只有 session 指针回主队列安装；按 ADJUSTED 结论不走 fallback 直通（会把 q/w/e 原样上屏），改为需要方案的按键进重放队列（上限 32）就绪后回放。
- **cross-13 + D10（text→keysym）**：删掉 Swift 私有 `rimeKey(from:)`，改调 `keytao_text_to_keysym`；按语义变化新增 `applyRejectedKeyFallback`——非 ASCII 现在会得到合法 keysym 并送进 Rime，`accepted=false` 时先 apply 本次状态、仍在组字则 `commit_composition()` 上屏、再把原字符直插，消除旧代码「有 composition 且 Rime 拒收」时的丢字。
- **D9 iOS（输入策略）**：`isSecureTextEntry` 或直通类 `keyboardType` 时调 `set_input_policy(composing:false, learning:false)`，按键完全不进 librime 并清 marked text，转回普通输入框恢复 `composing=true`。

deferred：

- **iOS Rust target 编译（`verify-ios-ime.sh` 最后一步）**：阻塞点在 keytao-core + vendored runtime（1.8.5，缺 `change_page`/`highlight_candidate_on_current_page`），不在本包 ownership；**Swift 侧完全不受影响、已独立验证通过**。**此项后由补充修复包解决**（见 2.11）。
- **ios-15 主 App 侧的 App Group 原子写**：`src-tauri/src/lib.rs` 对 `theme.yaml`/`ios_ime.json` 仍是先截断后写的非原子写，需 temp+rename，属 App 侧 ownership；扩展侧已做防御（解析为空时保留上一次成功的 config/theme）。**此项后由补充修复包解决**（见 2.11）。
- **ios-13 扩展内存基线实测**：降级钩子（`didReceiveMemoryWarning` + `releaseCaches`）已就位，但键盘扩展 jetsam 上限是 Apple 未公开数值，键道大词库在 appex 内真实常驻内存必须真机用 Instruments 量一次，本环境只有模拟器 mock runtime。
- **ios-6 冷启动耗时实测**：异步化已完成，但 20-60ms 还是秒级需真机 + 真实 librime runtime 才能测。
- **ios-3 的 `autocapitalizationType` `.sentences` / `.words` 完整语义**：完整实现需每次上屏后按 Unicode 断句判定词/句边界，属英文体验细节，本键盘主场景是中文；当前只实现「新上下文预置一次性 Shift」+ `allCharacters` 锁 Shift，且只在英文模式生效。
- **cross-14 iOS 部分（不适用 / N/A）**：cross-14 核对结论明确「真正分叉的只有 macOS」「iOS 无对应实现」；iOS 是纯软键盘，无硬件按键 bypass 路径，本项对 iOS 不成立。

### 2.11 补充修复包（编排方在七包之后单独派发，已完成并验证）

- **fixed：2 条**。**deferred：0 条**（仅登记 2 项遗留，见第四节）。

代表性条目：

- **iOS librime 1.8.5 编译门控**（`crates/keytao-core/src/lib.rs`）：vendored iOS librime 为 1.8.5，无 `change_page` 与 `highlight_candidate_on_current_page` 成员（1.9 才加入）。keytao-core 两处使用点做 `cfg(target_os = "ios")` 降级——`highlight_candidate_on_page` 在 iOS 为 no-op，`change_page` 在 iOS 复用既有合成 `'-'`/`'='` 翻页 fallback。**解除了 ffi-jni 与 W1-ios 的 iOS target 编译阻塞**；升级 vendor/librime/ios 到 1.17.x 后可移除门控（遗留事项）。
- **App Group 配置原子写**（`src-tauri/src/lib.rs`）：新增 `write_file_atomic`（同目录 temp + `sync_all` + rename），改造 iOS/Android 扩展会并发读的两处写入点（`theme.yaml` 与 `ios_ime.json`/`android_ime.json`）。**解决 ios-15 主 App 侧非原子写**。

遗留（补充修复包登记，见第四节）：vendor/librime/ios 升级到 1.17.x（需重跑 `scripts/build-ios-librime.sh` 并处理 1.8.x 专用补丁）；iOS/Android target 下 `keytao-core` `src/lib.rs:227` 的 `RimeFindModule` unused import warning。

## 三、验证矩阵

### 3.1 各包验证命令与结果

状态取值：`pass`（已执行且通过）、`fail`（已执行未通过）、`unverified`（未执行或本环境无法执行，需真机/真实桌面/目标 target）。**验证环境为 macOS 开发机**；Windows / Linux 的运行时验收本机不可完成，Linux 编译检查通过 `keytao-linux-check` 容器完成。

| 包 | 验证命令（要点） | 状态 | 说明 |
| --- | --- | --- | --- |
| core（W0-A/B1） | `cargo test -p keytao-core`（host，补充修复包实测） | pass | 39 passed / 0 failed；纯逻辑单测覆盖 key_policy/偏移换算/policy 默认值 |
| core（W0-A/B1） | 新 Engine API（select/change_page/commit/clear/enter） | unverified | 需真实 librime + 已部署用户目录，改为 `examples/*_smoke.rs` 手动冒烟（已在真机 librime 跑通） |
| theme（W0-C） | `cargo check/test -p keytao-theme` | pass | 保留兼容别名，下游零改动可编译 |
| ffi-jni | `cargo check -p keytao-core-ffi` | pass | 零 error 零 warning |
| ffi-jni | `cargo check -p keytao-app`（host） | pass | 13 个 warning 全为 HEAD 既有的 android cfg 双分支未用参数 |
| ffi-jni | `cargo test -p keytao-core-ffi` | pass | 6 passed / 0 failed |
| ffi-jni | `cargo check -p keytao-app --target aarch64-linux-android` | pass | 唯一能编译到 JNI 代码的路径；4 个 warning 为 HEAD 既有 |
| ffi-jni | `cargo check -p keytao-core-ffi --target aarch64-linux-android` | pass | 清 cfg 门控后零 warning |
| ffi-jni | `cargo check -p keytao-core-ffi --target aarch64-apple-ios` | fail → pass | 原失败在依赖 crate keytao-core（vendored iOS librime 1.8.5）；**补充修复包后转 pass** |
| linux-ibus | `docker … keytao-linux-check cargo check -p keytao-linux-ime --all-targets` | pass | Linux target 真实编译，0 error，1 个既有 warning（属 linux-2 包） |
| linux-ibus | `docker … cargo test -p keytao-linux-ime` | pass | 19 passed / 0 failed |
| linux-ibus | `cargo check/test -p keytao-linux-ime`（macOS host） | pass（无覆盖） | host 上后端全部 `#[cfg(target_os="linux")]`，编译 0 行、跑 0 测试，不作为验收依据 |
| linux-ibus | `bash -n verify-linux-bundles.sh` + keytao.xml / tauri.linux.conf.json 校验 | pass | 脚本语法、XML 良构、JSON 合法 |
| linux-ibus | `scripts/build-linux.sh`（docker 全量 deb/rpm + verify） | unverified | 完整 tauri build 时间/网络代价过大，本轮未跑；配置与校验脚本层面已正确 |
| linux-wayland-xim | `docker … cargo check -p keytao-linux-ime` | pass | Linux target 0 warning / 0 error |
| linux-wayland-xim | `docker … cargo test -p keytao-linux-ime` | pass | 33 passed（本包贡献 15 个） |
| linux-wayland-xim | `cargo check/test -p keytao-linux-ime`（macOS host） | pass（无覆盖） | 同上，编译 0 行 |
| windows | `cargo check -p keytao-windows-ime`（macOS host） | pass（无覆盖） | `#![cfg(target_os="windows")]` 令 crate 体为空，实际什么都没检查 |
| windows | `cargo check -p keytao-windows-ime --target x86_64-pc-windows-msvc` | pass | 零 error 零 warning（需 `-ffreestanding`）；本包真正有效的编译检查 |
| windows | `cargo check … --target aarch64-pc-windows-msvc` | pass | ARM64 target 同样零 error / 零 warning |
| windows | `cargo test … --target x86_64-pc-windows-msvc --no-run` | unverified | 类型检查通过、零 warning，链接阶段 `link.exe not found`（macOS 无 MSVC 链接器），12 个单测**从未真正执行** |
| windows | `cargo fmt -p keytao-windows-ime --check` | pass | 格式干净 |
| macos | `swiftc -typecheck Sources/KeyTaoIME/*.swift` | pass | 零 error 零 warning |
| macos | `swift build`（SwiftPM，带 FFI dylib -Xlinker） | pass | Build complete |
| macos | `build.sh --release --skip-pkg` | pass | cargo + swiftc + 签名全通过；bundle 版本实测 1.2.1-alpha.42 / 1.2.1.42 |
| macos | `scripts/build-macos.sh` | pass | 首跑因仓库迁移遗留失败，`cargo clean -p tauri --release` 后重跑通过 |
| macos | `scripts/verify-macos-pkg.sh` | pass | 含新增两条断言（pkg 版本 = IME bundle 版本；postinstall 不含 killall cfprefsd） |
| macos | `smoke.sh`（真实 librime 1.17 + 已部署用户目录副本，26 断言） | pass | text→keysym / Enter 键集 / UTF-16 偏移 / stamp / 候选面板模型（含 orientation=vertical）/ bypass 四态 |
| android | `./gradlew :app:compileArm64DebugKotlin` | pass | BUILD SUCCESSFUL，无 Kotlin warning |
| android | `./gradlew :app:testArm64DebugUnitTest` | pass | 7 个测试类共 64 个用例，failures=0 errors=0 |
| android | `./gradlew :app:compileArm64DebugAndroidTestKotlin` | pass | 仪表测试源码编译通过；**测试本身需真机 + fixtureRoot，未运行** |
| android | `processArm64DebugManifest` / `mergeResources` / `processResources` | pass | 合并 Manifest 已确认三条存储权限与 requestLegacyExternalStorage 消失 |
| android | `cargo check -p keytao-app`（host / aarch64-linux-android） | pass | 13 / 4 个 warning 均 HEAD 既有 |
| ios | `swiftc -typecheck`（iphonesimulator / iphoneos target） | pass | 两个 target 全量类型检查通过，零 error 零 warning |
| ios | `bash scripts/verify-ios-ime.sh` | fail → pass | 前四步通过，卡在末步 iOS Rust target 编译（vendored 1.8.5）；**补充修复包后全绿** |
| ios | `swift build`（SwiftPM） | unverified | Package 声明 `.iOS(.v15)`、源码 import UIKit，macOS host target 无法编译；已用 `swiftc -typecheck` 覆盖 |
| 补充修复 | `cargo check -p keytao-core --target aarch64-apple-ios` | pass | 原 2 个 E0609 消失 |
| 补充修复 | `cargo check -p keytao-core-ffi --target aarch64-apple-ios` | pass | — |
| 补充修复 | `bash scripts/verify-ios-ime.sh` | pass | 全绿 |

### 3.2 待人工验证清单（unverified 的真机 / 真实桌面联调项）

以下为本机无法执行、必须进入对应真机 / 真实桌面 / 目标 target 才能完成的验证，可直接照单执行。

**Windows（真实 Windows 宿主）**

1. `cargo build -p keytao-windows-ime --target x86_64-pc-windows-msvc --release`：确认 TSF DLL 完整链接。
2. `cargo test -p keytao-windows-ime`：本轮全部 12 个单测从未被执行（macOS 无 MSVC 链接器）。
3. 中文模式无 preedit 直接按 `,` `.` `/` `;` → 应上屏 `，` `。` `、` `；`（cross-3）。
4. 有 preedit 时 Ctrl+C / Ctrl+A / Ctrl+V → 宿主必须收到、composition 不变（`should_consume_processed_state` 改严的回归点，本轮风险最高）。
5. 单击 Shift 抬起 → 中英切换 + 模式提示（windows-1）。
6. CapsLock 开启后打字母 → 走 schema 的 caps_lock 策略（windows-8）。
7. Chrome/Edge 密码框 → 不弹候选窗、不组字、按键原样进宿主（windows-3）。
8. 系统 Ctrl+Space 关闭输入法 → 透传；输入指示器切中/英 → Rime 同步（windows-5）。
9. Word/Chrome 滚动页面 → 候选窗跟随；慢排版宿主首次打字 → 候选窗不再出现在屏幕左上角 (100,100)（windows-6）。
10. 鼠标点击候选项/翻页按钮 → 选中/翻页，宿主焦点不丢（windows-11）。
11. Win+Space 切走输入法时正在组字 → 宿主文档不残留带下划线的 preedit（windows-4）。
12. 打包路径与注册长路径：确认 IME runtime 位于 `keytao-windows-ime-runtime/x64`、`DllRegisterServer` 写入完整 DLL 路径无 260 截断；反复 focus/blur 与切换输入法无随机卸载/崩溃。

**Linux（真实 GNOME / KDE / X11 桌面）**

1. `scripts/build-linux.sh`（docker 全量 deb/rpm）+ `verify-linux-bundles.sh`：端到端确认 `/usr/share/ibus/component/keytao.xml` 确实进了 deb/rpm。
2. GNOME(ibus-daemon)：`UpdatePreeditText(vubu)` 生效、`ibus list-engine` 见 keytao、密码框直通、面板指示器跟随中英（linux-1/linux-14/D9/linux-18）。
3. KDE/KWin（input-method-v1）：context 销毁、key serial 配对、`content_type` 下发、按键重复（linux-7/linux-16/linux-17/linux-3）。
4. wlroots Wayland（input-method-v2）：`apply_state_to_input_method` 顺序、焦点抖动不丢字、reload 广播清 UI（linux-9/linux-15/linux-19）。
5. XIM / XWayland：`XIMPreeditPosition` over-the-spot 光标跟随、KeyRelease 订阅 solo Shift 切中英、reload epoch 不重画旧候选（linux-13/cross-7/linux-19）。
6. DBus 地址：非 uid 1000 用户 / 远程会话 / 自定义 `XDG_RUNTIME_DIR` 下地址文件写入当前 session bus。

**macOS（真机 IMKit，安装 pkg + 注销重登）**

1. 失焦提交：TextEdit/Safari/Chrome/Electron 输入框中普通 Return、`commitComposition`、`deactivateServer` 三条路径都能正确提交/丢弃、宿主不残留 marked text。
2. Cmd+Shift 系热键：不误触发中英切换（macos-2）。
3. 分段 marked text：`selStart`/`selEnd` 下的三段下划线（已转换/选中/未转换）正确（macos-12）。
4. 候选窗层级：多屏 / WebView / Electron / 全屏下候选窗层级与定位正确（macos-3 windowLevel + collectionBehavior 待实测）。
5. 部署后自动 reload：主 App 部署方案后 DispatchSource 监听触发，激活中的 controller 清 marked text、刷新状态。

**iOS（真机 / 真实 librime runtime）**

1. `swift build`（SwiftPM，iOS target）与真机端到端键盘交互（地球键切换、marked text、密码框直通、拒收上屏 fallback）。
2. ios-13：键盘扩展在真机用 Instruments 量键道大词库常驻内存与 jetsam 余量。
3. ios-6：真机 + 真实 librime 冷启动耗时实测（决定 20-60ms 还是秒级）。

**Android（真机）**

1. 仪表测试 `KeytaoImeEngineInstrumentedTest`（需真机 + fixtureRoot 参数）。
2. android-1 旧目录迁移在真机上的行为（删掉 `MANAGE_EXTERNAL_STORAGE` 后 `migrateLegacyRootIfNeeded` 多数设备读不到旧目录，用户可能需重装方案）。
3. D8 preedit cursor 在各家编辑器（尤其 WebView）内的落点（当前落末尾）。

## 四、遗留事项

### 4.1 vendor/librime/ios 版本落后（1.8.5，需升级至 1.17.x）

- vendor 里 iOS 的 librime 是源码 commit `08dd95f5`（1.8.5 时代）构建的，桌面（macOS）用的是 1.17.0，两者 `RimeApi` 结构体成员不一致——iOS 缺 `change_page` 与 `highlight_candidate_on_current_page`（1.9 才加入）。
- **当前过渡措施**：补充修复包已在 `keytao-core` 对这两处用 `cfg(target_os = "ios")` 降级门控（`highlight_candidate_on_page` 在 iOS 为 no-op；`change_page` 在 iOS 复用合成 `'-'`/`'='` 翻页 fallback），iOS Rust target 现已可编译。
- **含义与后续**：这不只是编译问题——它意味着 iOS 上跑的 librime 与其它平台不是同一代，D4「翻页/高亮走官方 API」在 iOS 上要到 runtime 升级后才真正生效。升级 vendor/librime/ios 到 1.17.x 需重跑 `scripts/build-ios-librime.sh`，且该脚本固定在 LibrimeKit v0.1.0 的 ref、带 1.8.x 专用的 C++14 + Boost 1.76 兼容补丁（含 librime-lua filesystem 补丁），盲改 ref 会连带改 Boost 版本、C++ 标准与 lua 插件补丁，需由 runtime/构建负责方处理并在真机验证。升级后可移除上述 cfg 门控。

### 4.2 iOS / Android target 的 RimeFindModule unused import warning

- 在 iOS / Android target 下，`keytao-core` `crates/keytao-core/src/lib.rs:227` 有 `unused import: RimeFindModule` 的 warning（HEAD 起就有，非本轮引入）。属既有编译噪声，未在本轮改动范围内，登记待清理。

### 4.3 仓库无 test CI

- 仓库只有 `.github/workflows/release.yml` 一个 tag 触发的发版工作流，没有任何 test/CI 测试 job。
- 影响最大的是 `keytao-linux-ime`：其后端代码全部 `#[cfg(target_os = "linux")]`，在 macOS/Windows 开发机上 `cargo test -p keytao-linux-ime` 会**静默跳过全部后端代码**（本轮实测：host 上 0 passed，Linux 容器里 19 / 33 passed）。这正是 cross-15 那条守门测试长期没在任何 target 编译过的原因。
- **建议**：补一条 Linux runner 上的 `cargo test -p keytao-linux-ime`，以及 Windows runner 上的 `cargo test -p keytao-windows-ime`（本轮 12 个单测从未真正执行）。新建 CI 工作流会影响全仓门禁策略，属主会话决策事项，各 Linux 包已把偏离与容器跑法写入 IMPL.md。

### 4.4 各包 deferred 的后续建议（汇总）

以下按包汇总本轮 `deferred` 项及后续处理方向（与第二节一一对应，供排期参考）：

- **core-keyapi**：`InputContextPolicy.learning` 独立强制需 schema 侧加 `no_memorize` switch 或改 librime；新 Engine API 的自动化单测需 CI 提供真实 librime + 已部署目录。
- **ffi-jni**：`src-tauri/src/rime.rs` 与 App 进程内裸 Engine 建议单独派活迁到 `ImeRuntime`（否则不受 reload 保护）；JSON 路径系统配色探测 fallback 建议由各桌面平台改吃 `keytao_set_system_color_scheme`。
- **linux-ibus**：CI 增补见 4.3；`IBUS_ENGINE_PREEDIT_COMMIT(1)` 分支待将来引入「失焦提交」策略时再按上下文选 mode。
- **linux-wayland-xim**：`reload_bus` 接入 IBus shim / GNOME engine 需把广播改成 `tokio::sync::watch` 或 `AsyncFd`（跨 W1-linux-1 ownership）；linux-5 kimpanel 光标矩形待接入 text-input-v2 `TextInputRectangle` 或 KWin 扩展后补；linux-13/15/17 协议层回归留给后续 Linux 桌面 golden 测试。
- **windows**：windows-9 `IMMERSIVESUPPORT` 待打包器满足 AppContainer 前提后再注册；windows-10 context→session 映射为长期方案；`sel_start`/`sel_end` 分段显示需新增第二个 display attribute GUID。**windows-7 归属待核**（见 2.7）。
- **macos**：macos-3 `collectionBehavior`、macos-7 恢复鼠标点击提交需真机实测回调可达；`macos_ime_status` 返回 IME bundle 版本属 W0-B2 ownership、建议编排方决定是否另派；候选窗 hover 高亮 FFI 入口已具备、待重设计时序。
- **android**：android-17 Direct Boot 需 device-protected 存储改造；D8 preedit cursor 受 `BaseInputConnection` 表达能力限制，建议接入 `requestCursorUpdates`/`getSurroundingText`(API34) 后再做；android-8 repeat 期间缓存需先有可信失效信号。D9 `NO_SUGGESTIONS` 偏离已在 IMPL.md 与通用层记一笔。
- **ios**：iOS librime 升级见 4.1；ios-13/ios-6 真机实测见 3.2；ios-3 sentences/words 完整语义属英文体验细节；cross-14 iOS 部分不适用。
- **补充修复包**：仅登记 4.1 / 4.2 两项遗留，无 deferred finding。

### 4.5 其它环境与工程遗留（非 finding，登记备忘）

- **仓库迁移遗留**：仓库已从 `/Users/rea/code/keytao-app` 迁到 `/Users/rea/code/keytao-org/keytao-app`，`target/release/build/tauri-*/out` 里的产物仍指向旧路径，跑 `scripts/build-macos.sh` 等 Tauri 构建会失败，需 `cargo clean -p tauri --release` 后重跑。凡跑 Tauri 构建的包都会撞上。
- **Android 交叉编译环境变量**：`vendor/librime/android/*/env.sh` 只导出大写目标名 `CC_AARCH64_LINUX_ANDROID`，当前版本 cc-rs 只读小写 `CC_aarch64_linux_android`，直接 source 后跑会在 bzip2-sys 上失败，需额外 `unset CC CXX AR` 并导出小写目标名。属 vendor 脚本既有问题，建议归口维护者修。

---

本报告数字与条目均以 design.md / upstream-w0.md / doc-notes-all.md 为准；凡材料未明确处（如 windows-7 归属、severity 逐条标注）已显式标注「待核」，未作编造。`deferred` 与 `unverified` 已在第二、三节显性呈现，不淡化、不省略。

> 上面这段是第一至四节（整改处置阶段）的收尾说明。整改之后另有交叉审查与修复回环两个阶段，补录于下方第五节；第五节对第 2.4 / 2.9 / 3.1 / 4.2 / 4.4 若干条目有更正与关闭，以第五节为准。

## 五、交叉审查与修复回环

审查日期：2026-07-27；修复与最终门禁：2026-07-28。

第四节之后又发生了两个阶段，本节补录：**(1)** 由 Codex(GPT) 作为独立审查方对整改后的全量工作树 diff 做交叉审查，产出 20 条编号发现，结论是「**不能原样合并**」；**(2)** 7 个 Opus 修复包 + 1 个 iOS 能力跟进包对全部 20 条完成处置，最终验证门禁全绿。审查者与实现者不是同一模型，这是本轮唯一一次真正的外部复核。

### 5.1 审查方法与结论摘要

- **审查对象**：基于 `main@b543d40c` 的未提交工作树，实际检查 `git status --short`、完整 `git diff` 与全部 untracked 代码文件；排除 `docs/**` 与 `**/IMPL.md` 后共 **70 个文件、约 +9669/-2874**。代码范围 `git diff --check` 通过。
- **审查方的环境限制（已如实声明）**：沙箱只读，`cargo check` 在创建 target `.cargo-lock` 时 `Operation not permitted`；Linux Docker 验证因无权连接 Colima socket 失败。**因此审查方没有把实现者先前自报的通过结果当作本轮独立通过**，全部 finding 按静态阅读当前工作树重新核对，未用历史结论替代本轮证据。
- **审查方核实为属实的自报项**（第二、三节的这些结论经外部确认成立）：
  - `Engine::Drop` 确实在锁内通过 `ManuallyDrop` 销毁；
  - 未发现遗漏锁保护的直接 `rime_get_api` 调用点，也未发现 re-entrant 深度计数错误；
  - raw session pointer 的解引用与 C 字符串读取全部位于 guard 闭包内。
- **审查方纠正的自报计数（第 2.4 节数字据此更正）**：
  - C 导出**实际 57 个**、全部进 `guard`，自报的「56 个」只是计数过时（本轮修复后为 60 个，仍全部进 guard）；
  - JNI **实际 32 个导出、31 个进 `android_jni_guard`**，自报的「33 个全部 guarded」不实——缺口就是 finding #7 的 `nativeEngineAvailable`。
- **严重度分布**：

| 严重度 | 条数 | 编号 |
| --- | ---: | --- |
| P1 | 8 | #1、#2、#5、#6、#11、#12、#18、#19 |
| P2 | 9 | #3、#4、#8、#9、#13、#14、#15、#16、#17 |
| P3 | 3 | #7、#10、#20 |
| 合计 | 20 | — |

- **原判结论**：不能合并。最低合并门槛是修好全部 P1/P2（#1–#6、#8–#9、#11–#19）；P3 不单独阻断合并，但 JNI guard 缺口与重复状态机适合随对应 P1/P2 一并收口。
- **处置总量**：**23 项修复、0 项反驳**（20 条发现无一被驳回；条数大于 20 是因为 #15/#16/#19 跨包拆分处置）。另新登记 **8 项 deferred**，全部是衍生项或边缘场景，不是对这 20 条本身的搁置。

### 5.2 20 条发现的处置表

| # | 严重度 | 一句话缺陷 | 处置 | 修复要点 |
| --- | --- | --- | --- | --- |
| 1 | P1 | reload barrier 与 session registry 是每个 `ImeRuntime` 私有的，但它们保护的是进程级 librime 状态 | fixed（core） | 收敛为进程级 `ProcessRimeState`（initialized/generation/reload_barrier/registry）；`create_session` 改为在持 initialized 互斥锁的前提下取 barrier、建 Engine、注册，目录不匹配返回 `Err`；补完并收紧两个双 runtime 并发测试 |
| 2 | P1 | `process_key_result` 与 `set_input_policy` 分两次加锁，policy 存在 TOCTOU 窗口 | fixed（core） | 按键走 `with_engine_and_policy` 的单一临界区（barrier→session inner→librime）；`set_input_policy` 改为在同一临界区内**无条件先落 policy** 再 refresh + clear composition（原来引擎建不出来会把敏感 policy 静默丢掉）；policy 形参由 `&mut` 收成按值 |
| 3 | P2 | deploy 系列在持 `RIME_API_LOCK` 时做路径处理、YAML 解析、字典准备与部署后校验 | fixed（core） | 已由 `DEPLOY_LOCK` 串行化部署、`RIME_API_LOCK` 只包真正的 rime_api 调用；本轮逐个复核 29 处取锁点确认慢 IO 全在锁外。唯一长时间持锁的 `full_deploy_and_wait` 属 D2 允许的不可拆 native 事务 |
| 4 | P2 | legacy singleton 持 `GLOBAL` 调可能 panic 的 core 代码，一次中毒即永久失效 | fixed（ffi） | `lock_ignore_poison` / `read_ignore_poison` / `write_ignore_poison` 成为唯一取锁入口（全文已无裸 `.lock()/.read()/.write()`）；所有 `GLOBAL` 临界区只 clone/take 后放锁再调 core，旧 runtime 在锁外 drop |
| 5 | P1 | `keytao_init` 无条件替换全局 runtime，不回收旧 runtime 发出的 opaque session handle | fixed（ffi） | `INIT_LOCK` 串行化 init、同目录重复 init 直接返回 true；`SESSION_EPOCH` 给每个 handle 打 epoch，切目录时写守卫下 +1 并 `clear_global`，旧 handle 立即退役且不可能在 finalize 时正在飞行中；本轮补掉 `reload_now()` 未持 epoch 的窗口，并给「librime 跑在别的数据目录」这种纯竞态加了**只重试一次**的 `create_session_retrying_a_switch` |
| 6 | P1 | guard 捕获 panic 后在 `catch_unwind` 外 `eprintln!`，日志本身可能二次 panic 越过 ABI | fixed（ffi） | `log_error` 整个报告体自带第二层 `catch_unwind`，内部用 `stderr().lock()` + 显式丢弃 `io::Result`；全文 `eprintln!/println!` 归零；`format_args!` 惰性求值也发生在该 catch_unwind 内，Display 实现 panic 同样逃不出去 |
| 7 | P3 | `nativeEngineAvailable` 是唯一没进 `android_jni_guard` 的 JNI 导出 | fixed（src-tauri） | 该导出已包进 guard，32 个 `Java_*` 全部覆盖；真正的增量是**守卫测试原本会漏检**——旧实现用 `strip_prefix("pub extern \"system\" fn Java_")` 做行首匹配，缩进的、`unsafe extern` 的、同行带属性的导出会被静默跳过。改为行内查找 + 扫描器自测 + 必须扫到三个已知导出的兜底 |
| 8 | P2 | Linux watcher 在 reload 成功**前**就调消费式 `has_changed()` 推进签名，失败即永久不重试 | fixed（linux） | 换成 peek/commit 的 `ReloadStampGate`：`pending()` 只 stat 比签名不推进基线，`commit()` 只在 `reload_without_deploy()` 返回 `Ok` 后调用，且推进到的是**本次加载前读到的签名**（reload 期间新写的 stamp 不被吞）。本轮另把 gate 的构造从子线程提前到 init 刚返回处，收敛播种窗口 |
| 9 | P2 | Tauri test-input 持有不在 registry 内的裸 `Engine`，部署后先建新 Engine 再覆盖旧的 | fixed（src-tauri） | `RimeEngine` 改持 `Option<ImeRuntime> + Option<ImeRuntimeSession>`，新增 `suspend_for_deploy()`（部署**前**先关 session 交还 runtime）与 `resume()`；`rime_setup` / `rime_deploy_default` 都改为 suspend → `runtime.reload()` → resume。src-tauri 全仓已无 `Engine::new` |
| 10 | P3 | 中断续写留下两份近乎逐行相同的 IBus 按键状态机 | fixed（linux） | 按键决策已收进 `src/ibus_shared.rs`；本轮补进审查点名但上次遗漏的 `policy`（`ContentTypeState`，能回答「这次是不是刚跨进密码框」）、模式指示器发布门禁（`ModeIndicator::update()` 用一次 swap 做原子 test-and-set，顺带消掉两个前端各自的竞态）与 select/change_page/navigation 三个面板动作。两个前端只剩各自协议的发布代码 |
| 11 | P1 | Android 对 `NO_SUGGESTIONS` / `NO_PERSONALIZED_LEARNING` 保持 `composing=true`，`learning=false` 实际不生效 | fixed（android） | `InputPrivacyMode.allowsComposing` 改为 `!password && !noLearning && !noSuggestions`，`allowsLearning` 直接等于它，三类编辑器一律直通。**这条同时撤销了第 2.9 节登记的「D9 Android 唯一一处偏离」**——那处偏离（`textNoSuggestions` 保留组字能力）正是本 finding 的对象，现已不再成立 |
| 12 | P1 | Windows 把同步 edit-session / selection / property / `GetInputScopes` 的**所有**失败都解释成「不是密码框」 | fixed（windows） | 三态 `ContextProbe{Restricted/Clear/Unknown}`，Unknown 即 fail closed 直通并挂 250ms 节流重试（放在 `OnTestKeyDown` 判定之前，因为 TSF 不会为被测试回调拒绝的键调 `OnKeyDown`）；`GetSelection` 同时检查 HRESULT 与 fetched count。本轮另补两处真实缺口：敏感 scope 从只认 `IS_PASSWORD` 扩到 PIN 四类 + `IS_PRIVATE`（与 Linux 的 purpose PASSWORD/PIN + hint PRIVATE 对齐）；敏感 policy 此前**从未真正下到 session**（`sync_input_policy` 的调用点在 `input_is_blocked` 之后，敏感上下文永远走不到），改在 `poll_engine_builds()` 补推 |
| 13 | P2 | Windows 未订阅当前 context 的 `KEYBOARD_DISABLED`/`EMPTYCONTEXT`，动态 blocked 时不清 composition | fixed（windows） | 两个 compartment sink 随焦点 context advise/unadvise（按 COM identity 去重，换 context 时先装新再退旧，Deactivate 收尾）；`OnChange` 分派到 `apply_context_compartment_change()`，只重读两个 compartment 并保住上一次的 input-scope 答案，「由不敏感变敏感」时用排队 write session 结束 composition + 隐藏候选 UI。本轮加固：`refresh_input_context()` 敏感分支由 `hide_ime_windows()` 改为 `reset_input_for_focus_change()`，不再依赖「调用方恰好都先跑过 reset」这个脆弱不变量 |
| 14 | P2 | iOS 仍按 `currentState.asciiMode` 直接提交文本，绕过 D6 | fixed（ios） | 移除 asciiMode 旁路，只保留 D9 的 `hostTraits.bypassesRime`；新增 `applyRejectedKeyFallback`（先 apply 本次结果保住它带回的 commit → 有组字则 `commit_composition()` → 再插宿主）。本轮补掉同类里最后一处私货：`handleSpace` 原来无组字时 `commitDirect(" ")` 直接吞空格，导致 schema 的 `full_shape` 与 key_binder 绑的空格在 iOS 上永不生效，现在一律先送 `XK_Space` |
| 15 | P2 | Rust 已输出 `cursor/selStart/selEnd`，Kotlin 丢弃 selection 且恒用 `setComposingText(preedit, 1)` 把光标放末尾 | fixed（android） | `KeytaoImeState` 解析三字段（Unicode 标量语义与 core 一致）；新增 `composingRegionStart` 由 `onUpdateSelection` 的 `candidatesStart` 播种、随自发 commit 推进、`composing=false` 时作废；`applyState()` 写完 `setComposingText` 后经纯函数 `resolveComposingCaret()` 判定光标是否真在 preedit 内部，再经 JNI `nativeUtf16OffsetFromChars` 换算后 `setSelection()`；起点未知时退回末尾而不是写错位置。**有意偏离**：`selStart/selEnd` 只解析不转成真实选区（librime 的 sel 区间是「当前未转换段」，普通组字时覆盖整个 preedit，用选区表达会让编辑器每敲一键就弹选择手柄），Android 上正确做法是给 composing 文本挂 span，属视觉工作 |
| 16 | P2 | 旧 ABI fallback 用候选数字和 `-`/`=` 合成按键，iOS 1.8.5 翻页无条件走 synthetic path | fixed（core + ffi + ios，**iOS UI 部分由跟进包收口**） | **core**：能力查询原本直接读 `(*api).change_page` 等字段，而绑定按构建机头文件生成，运行时 librime 更旧时那是它从未写过的内存——**读函数指针本身就是 UB，能力查询会给出错误答案**。新增 `api_has_member()`（librime `RIME_STRUCT_HAS_MEMBER` 的 Rust 等价，按 `data_size` 判定）与 `rime_api_member!` 宏，`engine_capabilities()` 与全部可选入口读取统一走守卫。**ffi**：7 个 `KEYTAO_CAP_*` 位常量 + `keytao_engine_capabilities()`（纯 ABI 探测，init 之前即可调）+ `keytao_session_capabilities()`（null/已退役 handle 返回 0，即全部禁用，是安全方向）。**ios**：无原生翻页时在布局解析阶段丢掉翻页键（整行丢空则去行；全丢则保留原布局以免抹掉 Apple 强制的 🌐 键），`changeCandidatePage` 与 `didSelectCandidate` 按位加 guard。**跟进包**再补 capabilities 缓存 + `refreshCapabilities()`（有 session 用 session 版、无 session 回落 ABI 版，挂在 install/init/reload/reloadIfNeeded/close 五个点）、`isSelectable()` 按位分派选词、不可选时不触发触感且 accessibility 降级为 `staticText`。**注意：vendored iOS librime 仍是 1.8.5，合成路径本身没有删除，只是变成了可探测、可禁用**（升级见 4.1） |
| 17 | P2 | 共享 layout 的 landscape 默认 0.62 / 最低 0.45，iOS 却对所有方向统一 clamp 到 ≥0.70 | fixed（ios） | 抽出不引 UIKit 的 `KeyTaoIOSFloatingLayout.swift`，按方向取地板：portrait ≥0.70、landscape ≥0.45、上限 1.0、默认 0.88/0.62，逐条对齐 `keytao-theme::mobile_layout::sanitize()`；交互式拖拽上限 0.94 保留为 iOS 独有限制。本轮补掉 `KeyTaoIOSKeyboardLayoutStateStore.save(isLandscape:state:)` 用 state 自带 orientation 归一化的漏洞（把横屏 0.45 写进竖屏槽会绕过竖屏地板），改为按写入槽位重定 orientation 再 clamp |
| 18 | P1 | Android 在 `accepted=false` 且无 preedit 时直接 fallback，未先应用同一结果携带的 `committed` | fixed（android） | 新增 `KeytaoRimeInput.applyRejectedResult()`：被拒结果只要带 `committed` 或宿主还留着 composing region，就先 `applyState()` 再把按键交给宿主，顺序对齐 iOS 的 `applyRejectedKeyFallback`。已接入 `onKeyDown` / `handleTextInput` / solo Shift release，本轮补上被漏掉的第四个回落点 `handleBackspace()` 的 else 分支（原本会丢掉该结果的 committed，还带着陈旧 region 去 `deleteSurroundingText`） |
| 19 | P1 | Android 与 iOS 的多字符 `rimeInput` 循环只保存并应用最后一次结果，中间 commit 被覆盖 | fixed（android + ios） | **Android**：循环抽成 `feedSequence()`，逐 codePoint 送 Rime、每次结果立刻按序 `applyState()`；keysym 缺失或中途被拒时先 `applyRejectedResult()` 再 reset + 提交 fallback。**iOS**：逐次 apply 之外补掉两个洞——(a) 中途映射不出 keysym 时原代码会把**整串**字面量再插一遍造成重复出字，现在改为**送出任何一键之前**先整串预映射，有一个映射不出就整键回退（Rime 完全不参与）；(b) 「被拒但仍有组字」的字符原本被静默丢弃，现在走 `applyRejectedKeyFallback`，插入文本取「Rime 没消费掉的剩余子串」，只有 `consumed == 0` 时才用该键声明的 fallbackValue |
| 20 | P3 | `candidate_api_smoke` 声称证明官方 API 与无 synthetic fallback，但没有确定性 fixture | fixed（core） | 用确定性 fixture（无 menu/select_keys；digits 与 `-`/`=` 都在 speller alphabet 内，故合成键只会改编码；无 key_binder/bindings；`page_size=12` > `DEFAULT_SELECT_KEYS` 的 10；20 个同码单字 + `sort: original` 强制两页且顺序固定）写成集成测试 `tests/candidate_api.rs`，跑在 `cargo test` 里而不是靠人手跑 example；example 本身改为如实声明「只走一遍真实部署上的路径、无法判定走的是官方入口还是 fallback」，并打印本次运行为何不具决定性 |

### 5.3 修复轮亮点

- **进程级 `ProcessRimeState` 收敛 + 变异验证的并发测试（#1）**：`tests/process_reload.rs` 两个用例——`independent_runtimes_share_one_librime`（两个独立 `ImeRuntime` 各持活跃 session，任一方 reload 后双方仍能组字）与 `concurrent_reloads_from_two_runtimes_keep_sessions_alive`（2 打字线程 + 2 reload 线程跑 3 秒）。**变异验证**：把 reload 改成不 drain 别人的 engine、不 bump 全局 generation（等价于每 runtime 私有 registry），两个用例都失败；改回即通过。
- **policy 单临界区（#2）**：`a_sensitive_context_never_composes_under_concurrent_typing` 用一个线程持续敲码、主线程 300 轮 sensitive↔default 切换，断言 sensitive 期间 `state()` 永不出现 preedit/候选，并额外断言打字线程确实组过字（否则测试是空的）。把 policy 读取挪回临界区外并加宽窗口后该用例稳定失败。
- **能力探测的 `data_size` 守卫（#16）**：这是本轮唯一一处**消除未定义行为**的修复——原实现直接读 `rime_api_t` 里构建机头文件才有的字段，运行时 librime 更旧时那块内存 librime 从未写过，读函数指针即 UB，能力查询会撒谎。新增单测 `members_past_data_size_are_reported_as_missing`：满 `data_size` 时 `change_page` 可见；把 `data_size` 截到 `setup` 之后时 `setup` 仍可见而 `change_page` 判缺失；空指针判假。
- **FFI epoch 退役机制（#5）**：`SESSION_EPOCH: RwLock<u64>` 给每个 opaque handle 打 epoch，切目录时在写守卫下 +1；`session_handle()` 持读守卫贯穿整个 core 调用，所以旧 handle 既不会答话，也不可能在 librime 被 finalize 时正在飞行中。锁序固定为 `INIT_LOCK → SESSION_EPOCH → GLOBAL → keytao-core`。诚实说明：`init_never_deadlocks_against_the_session_exports`（4 init 线程 + 4 session 线程 + 2 reload 线程）只是锁序/活性回归测试，**不能证明 reload 与切目录真的互斥**，那需要真实部署的 librime。
- **Android commit 保序（#18/#19）**：`KeytaoRimeInputTest` 7 条覆盖逐次按序应用、中途被拒先 flush commit 再 fallback、首键无 keysym 直接回落，并新增 `two commits inside one code both reach the editor` 直接钉住「一个编码里两次 commit 都必须到达编辑器」这个回归点。
- **确定性 candidate fixture 取代手动冒烟（#20）**：`selection_and_paging_go_through_librime` 断言 change_page 前后 preedit 不变而 page 变（合成 `=` 会让 preedit 变成 `aa=` 且 page 不动）、选中页内第 12 项（根本没有对应 select key）必须提交对应词、选中第 1 项必须提交首词（合成 `1` 会把编码改成 `aa1` 而不提交）。**变异验证**：强制 change_page 走合成键 → page 断言失败；强制 select 走 `send_select_key` → committed 断言失败。这条把第 3.1 节里 core 那条长期 `unverified` 的「新 Engine API 需真实 librime」转成了自动化用例。
- **JNI 守卫测试从「看着像已完成」变成真能抓漏（#7）**：`jni_export_scanner_rejects_unguarded_shapes` 对扫描器自身做单测，覆盖旧前缀匹配漏掉的形状（缩进在 `mod` 内、`pub unsafe extern "system"`）；变异验证把 `nativeEngineAvailable` 的 guard 换回裸 `1`，测试失败并精确报出 `lib.rs: nativeEngineAvailable`。
- **`ModeIndicator::update()` 的原子 test-and-set（#10）**：把原来 load→publish→store 的非原子发布门禁换成一次 swap，顺带消掉两个 IBus 前端各自的竞态；`the_mode_indicator_only_speaks_when_the_mode_moves` 在 swap 退化成 load 时即失败。
- **#9 首次获得编译验证**：`mod rime` 与 `rime_deploy_default` 的 Linux 分支是 `cfg(target_os = "linux")`，macOS host check 与 android target check 都碰不到，**此前从未被任何路径编译过**。修复包临时把三处 cfg 翻成 `cfg(macos)` 并临时加 `sysinfo`/`enigo` 依赖，做了一次真实类型检查（0 error），随后逐条精确回滚（`git diff -- src-tauri/Cargo.toml` 为空，lib.rs 与基线逐字节相同）。

### 5.4 修复后验证矩阵（编排方 2026-07-28 实测）

| 范围 | 命令 | 状态 | 说明 |
| --- | --- | --- | --- |
| host check | `cargo check`：keytao-core / keytao-core-ffi / keytao-theme / keytao-linux-ime / keytao-windows-ime / keytao-app | pass | keytao-app 13 条 warning 全为既有 cfg 相关 unused；**linux/windows 两包在 host 上是空跑**，见下方说明 |
| host test | `cargo test -p keytao-core` | pass | **44 通过 0 失败**：lib 单测 40（含新增 `api_member_tests`）+ `tests/candidate_api` 2 + `tests/process_reload` 2 |
| host test | `cargo test -p keytao-core-ffi` | pass | 16 通过 0 失败（改动前 15） |
| host test | `cargo test -p keytao-theme` | pass | 16 通过 |
| host test | `cargo test -p keytao-app --lib` | pass | 32 通过，含 `every_jni_export_is_panic_guarded` 与新增的扫描器自测 |
| Linux target | `docker … keytao-linux-check cargo check -p keytao-linux-ime --all-targets` | pass | 零 warning |
| Linux target | `docker … cargo test -p keytao-linux-ime` | pass | **44 通过 0 失败**（改动前 42，新增 2） |
| Linux target | `docker … cargo clippy -p keytao-linux-ime --all-targets` | pass | 11 条 warning 全在本轮未动的 panel.rs / x11_backend.rs / wayland_backend.rs 等既有代码；`ibus_shared.rs` 与本轮改动行零 warning |
| Windows target | `cargo check -p keytao-windows-ime --target x86_64-pc-windows-msvc --all-targets`（需 `BINDGEN_EXTRA_CLANG_ARGS="--target=arm64-apple-darwin"`） | pass | 零 error 零 warning，含 `#[cfg(test)]` 目标 |
| Windows target | `cargo clippy … --target x86_64-pc-windows-msvc --all-targets` | pass | 只剩 4 条既有 warning，全在 panel.rs |
| iOS target | `cargo check -p keytao-core / -p keytao-core-ffi --target aarch64-apple-ios` | pass | 无 warning；两个 target 生成的 `include/keytao_core.h` 逐字节相同，iOS/macOS Swift 可共用一份头文件 |
| iOS | `bash scripts/verify-ios-ime.sh` | pass | 全绿，含 plist/entitlement lint、Swift 全量类型检查、🌐 键注入的 5 条 grep 断言、librime-lua 符号检查、`cargo check -p keytao-core-ffi --target aarch64-apple-ios-sim` |
| iOS | `bash crates/keytao-ios-ime/test-floating-layout.sh` | pass | #17 的按方向 clamp 与共享层默认 JSON 解码断言 |
| Android target | `cargo check -p keytao-app --target aarch64-linux-android` | pass | 需额外 `TARGET_CC/TARGET_CXX/TARGET_AR` 并 `unset CC CXX`，见 4.5 |
| Android | `./gradlew :app:compileArm64DebugKotlin`、`:app:testArm64DebugUnitTest` | pass | exit 0；8 个测试类共 **77 条**（改动前 64），failures=0 errors=0；`compileArm64DebugAndroidTestKotlin` 亦通过 |
| 全包 | `cargo fmt -- --check` | pass | 各包分别执行，无 diff |

**验证矩阵的诚实说明（两条对第 3.1 节的更正）**：

1. **`cargo check/test -p keytao-linux-ime` 与 `-p keytao-windows-ime` 在 macOS host 上是空跑**，两包都在修复轮独立复现并确认：keytao-linux-ime 的 src/ 下每个 mod 都带 `#[cfg(target_os = "linux")]`（host 上 0.4 秒 Finished、跑 0 个用例）；keytao-windows-ime 的 `src/lib.rs` 根部是 `#![cfg(target_os = "windows")]`（crate 体为空，任何语法/类型错误都查不出来）。design.md「windows/linux 在 macOS host target 下可做编译检查」这句对这两个 crate 不成立，**建议更正 design.md 与本报告 3.1 的相应表述**；可用的替代命令见上表。
2. **keytao-windows-ime 的 32 个单测（含本轮新增 2 个）仍从未真正执行**，只做到了按 Windows target 编译通过。需要 Windows 宿主或 Windows CI，仍在 3.2 的待人工验证清单里。

### 5.5 仍开放的事项

#### 5.5.1 本轮新登记的 8 项 deferred

**keytao-core-ffi（2 项，final-gate 单独点名）**

1. **`keytao_init` 切目录失败时会连带退役所有存活 handle**：core 的 `init_without_deploy` 是先校验（schema 是否安装/是否部署、Windows repair）再 `shutdown_for_dir_change`。校验阶段失败时 librime 其实**完全没被动过**，但 FFI 已先 bump epoch 并 `clear_global`，于是一次目录写错的 init 会白白报废一个正在工作的输入法（需重新 `keytao_init` 恢复）。要区分这两种失败必须让 keytao-core 暴露「librime 当前跑在哪对目录上」的查询（`PROCESS_RIME.initialized` 目前私有）。现状是 fail-closed 的安全方向且已写进 `keytao_init` 的文档注释。**建议**：core 补一个 `pub fn running_data_dirs() -> Option<(PathBuf, String)>` 后再收口。
2. **`keytao_reload()` 失败后 `keytao_is_initialized()` 仍返回 true**：core 的 `reload_without_deploy` 失败时会把 `PROCESS_RIME.initialized` 置 `None`（librime 已 finalize），FFI 侧 `GLOBAL.initialized` 却仍是 true。实际后果有限且可自愈——存活 session 的 `with_engine` 会建不出 Engine 而返回 `None`（各导出返回 null，平台降级为直通），下一次 `keytao_create_session` 会由 core 把 librime 重新拉起来。同样需要上面那个查询才能准确判断。

**src-tauri（2 项，均为 #9 衍生）**

3. `rime_setup` 在部署失败时不重开 test-input session（与 `rime_deploy_default` 的「成功失败都 resume」不一致）。不是回归（HEAD 版本同样只在成功后才装 Engine），但补上会让失败路径在 async executor 线程上内联跑 `init_without_deploy`→`setup_only`；`tauri::State` 不是 `'static`、丢不进 `spawn_blocking`，要修得先把托管状态 Arc 化。
4. `resume()` 内联在 async 命令里做 librime 调用。成功路径只是 `RimeCreateSession` + schema 定位，很快；失败路径可能退化成 `setup_only`。与上一条同源、同一处结构限制。

**keytao-windows-ime（2 项）**

5. **宿主始终拒绝同步 read session 时的死锁风险**：fail closed 的代价是探测失败即 Unknown 即敏感即 `OnTestKeyDown` 返回 false，而 TSF 不会为被拒绝的键调 `OnKeyDown`，只有 `OnTestKeyDown` 里那次重试能自救。评估后不做异步回填：该 crate 的正常上屏路径走的是同步 **write** session（比 read 更难拿），一个永不给同步 read 的宿主本来就跑不起这个 TIP；且异步回填要引入新的 pending 状态与回调期 `RefCell` 重入面，在无 Windows 宿主可测时风险大于收益。
6. **同一 context 内 input scope 中途变化**（如网页把普通输入框改成 password）不会被重新探测：只有焦点/context 变化与 Unknown 重试会重跑探测。TSF 对这种变化通常会走 `OnSetFocus`/context 变化，且宿主一般同时置 `KEYBOARD_DISABLED`（那条有 sink 兜），实际暴露面小；彻底覆盖需订阅 `GUID_PROP_INPUTSCOPE` 的变更通知。

**keytao-ios-ime（2 项）**

7. **`test-floating-layout.sh` 未接进 `scripts/verify-ios-ime.sh`**：本次任务书把 ownership 收窄到 `crates/keytao-ios-ime/**`，未改脚本。现状是 #17 的解码测试只能手动跑。**成本是一行**，建议在 verify 脚本的 Swift 类型检查之后补上调用。
8. **符号层按键（`layerMode.id.isSymbolLayer`）仍走 directInput 直接上屏**：严格按 D6 字面「任何模式下按键都先进 Rime」这也算旁路，但它不是 #14 指控的对象；语义上符号层是「我要的就是这个字形」的显式挑选面板（同 Emoji 面板），送进 Rime 会被 schema 的 punctuator 改写成别的字符。留给架构侧裁决是否要把「层」也纳入 D6。

#### 5.5.2 iOS showMessage 提示可见性（跟进包顺带发现，未修）

能力不足时（如无原生翻页）`showMessage` 给出的提示，在候选可见时会被 `drawCandidateBar` 的渲染条件吞掉，用户看不到「此功能不可用」的反馈。要修需给候选栏加一条独立的瞬时提示通道，属小改动，跟进包已留档未做。

#### 5.5.3 与第四节遗留项的关系

- **4.1 vendor/librime/ios 版本落后（1.8.5 → 1.17.x）：仍开放，且这是 #16 未能彻底收口的根因。** 本轮做到的是让合成路径**可探测、可禁用**（`data_size` 守卫 + `KEYTAO_CAP_*` + iOS 按能力移除翻页键），但合成路径本身没有删除，D4「翻页/高亮走官方 API」在 iOS 上仍要等 runtime 升级后才真正生效。升级仍需按 4.1 的方式由 runtime/构建负责方处理。
- **4.2 iOS / Android target 的 `RimeFindModule` unused import warning：已在本修复轮顺带解决。** keytao-core 修复包把该 import 按平台 `cfg` 化（它只在 macOS/Linux/Windows 用），iOS 与 Android 两个 target 的 `cargo check` 现均无 warning（见该包 verification 备注）。**4.2 可以关闭。**
- **4.3 仓库无 test CI：仍开放，且本轮被两个包各自独立再次点名**——见 5.4 的诚实说明。另外 #9 那段 Linux-only 代码此前从未被任何路径编译过，也是同一个缺口的表现。建议除 4.3 已提的 Linux/Windows runner 外，再加一条 Linux 容器的 `cargo check -p keytao-app`。
- **4.4 各包 deferred 汇总**：第 2.9 节 android 项下的「D9 `NO_SUGGESTIONS` 偏离」条目**已由 #11 撤销**，不再是偏离；`src-tauri/src/rime.rs` 裸 Engine 那条建议**已由 #9 落地**，可从 ffi-jni 的待办中移除。其余各项状态不变。
- **4.5 Android 交叉编译环境变量**：修复轮再次踩到并给出了确切修法——`vendor/librime/android/*/env.sh` 只导出 cargo 风格的大写 `CC_AARCH64_LINUX_ANDROID`，cc-rs 不认，会回落到 shell profile 的 `CC=/usr/bin/cc`，bzip2-sys 用 host clang 编 android 目标直接 `stdlib.h not found`。可用做法是补 `TARGET_CC`/`TARGET_CXX`/`TARGET_AR` 并 `unset CC CXX`。建议把这三行直接补进 env.sh。

---

本节数字与条目均以审查报告原文、7 个修复包的结构化结果与编排方的最终门禁实测为准。20 条发现无一被反驳，也无一被搁置；8 项 deferred 全部为衍生项或边缘场景，理由已逐条列出。审查方与修复方均已声明各自的验证边界（只读沙箱、无 Windows 宿主、无真机），凡未真正执行的验证都标注在 5.4 与 3.2，未按「编译通过」冒充「测试通过」。
