# 输入法通用层实现规范

本文定义 KeyTao 系统输入法前端的通用层边界。后续新增或重构平台输入法时，应先对齐本文，再进入各平台 `IMPL.md`。

目标是让平台实现只处理平台差异，把稳定行为收敛到同一套 runtime、core、state、key event、UI model 和 reload 规则里。

## 当前结论

现在直接操作 librime 的代码在 `crates/keytao-core/src/lib.rs`：

- `deploy()` 负责 `setup()`、`initialize()`、`full_deploy_and_wait()`。
- `Engine::process_key_result()` 负责调用 `session.process_key(KeyEvent::new(...))`。
- `extract_state()` 负责把 librime context/menu/status 转成统一 `ImeState`。

平台层不应该直接操作 librime。Linux 之前有一份自己的 `CoreEngine/ImeSession` 调度，现在已经收敛为 `keytao-core::ImeRuntime` / `ImeRuntimeSession`；Linux 只在 `crates/keytao-linux-ime/src/engine.rs` re-export。macOS 通过 `keytao-core-ffi` 创建 per-session 时，也走同一套 `ImeRuntimeSession`。Windows TSF 当前也已改为持有 `ImeRuntimeSession`，不再直接持有 `Engine`。

除 librime 调度外，下面这些此前分散在平台层的规则现在也只有通用层一份实现，平台必须调用而不是复制：

- reload stamp 的路径、签名与变更检测（`ReloadStamp` / `ReloadStampWatcher`，见《Reload 规范》）。
- 候选选择、高亮、删除、翻页、提交、清空、Enter（走 librime 官方 API，见《候选交互规范》）。
- 文本到 keysym 的映射、Enter 判定、空 composition bypass 判定、CapsLock 归一（`key_policy`，见《按键事件规范》）。
- 光标与选区的偏移单位换算（`char_offset_from_utf8` / `utf16_offset_from_chars`，见《统一状态模型》）。
- 密码/敏感输入上下文策略（`InputContextPolicy`，见《敏感输入上下文》）。

还没有完全收敛的是系统协议本身、原生按键到 keysym 的转换、UI 绘制和各平台的 reload 触发时机（文件监听 / 定时器 / 生命周期回调各不相同）。这些属于平台差异，可以继续留在平台层，但不得在其中重新实现上面任何一条通用规则。

## 架构优点

这套架构的核心价值是把“输入法业务状态”和“平台系统协议”拆开：

1. librime 只由 `keytao-core` 操作，避免 Linux、macOS、Windows 各自再实现 deploy、session、candidate、reset、ascii mode 等细节。
2. 系统输入法统一使用 `init_without_deploy()` / `reload_without_deploy()`，部署后只递增 generation，已有 session 在下一次访问时懒刷新，减少“词库已部署但 IME 还在旧状态”的问题。
3. 平台层变薄：新增平台只要实现 key event 归一化、commit/preedit/candidate adapter 和 UI renderer，不需要理解 librime context/menu/status。
4. `ImeState` 成为唯一 UI/提交输入，顶功、commit + new preedit、候选 label、分页和中英状态可以按同一顺序处理。
5. `theme.yaml` 已落在共享主题语义和 UI model 上，平台 renderer 只负责把 model 映射到 AppKit、Wayland SHM、X11 overlay、TSF candidate window 或系统 lookup table。
6. 测试面更稳定：key map、modifier mask、reload generation、candidate actions 可以在通用层或薄 adapter 层分别测试，不需要每个平台重复造一套业务状态。

这也意味着平台层不要把“为了某个平台方便”而新增的字段直接塞进 `ImeState` 或 `theme.yaml`。平台差异应落在 adapter 能力声明和 renderer fallback 里。

## librime 通信模型

librime 是进程内 native library，不是独立服务。通用层和 librime 的通信发生在 `keytao-core` 内部：

```text
platform key event
  -> X11 keysym + Rime modifier mask
  -> ImeRuntimeSession::process_key_result()
  -> Engine::process_key_result()
  -> rime_api::Session::process_key(KeyEvent)
  -> KeyStatus + Rime context/menu/status
  -> extract_state()
  -> KeyProcessResult { accepted, state: ImeState }
  -> platform commit/preedit/candidate adapter
```

部署通信：

```text
App installs schema/dict/lua/opencc files
  -> keytao_core::deploy(user_data_dir, shared_data_dir)
  -> rime_api setup/initialize on first call
  -> full_deploy_and_wait()
  -> App writes keytao-ime.reload
  -> running IME calls ImeRuntime::reload_without_deploy()
  -> generation += 1
  -> ImeRuntimeSession refreshes internal Engine lazily
```

按键通信里只有两种输入从平台进入通用层：

- `keycode`：统一使用 X11 keysym，例如 Return 是 `0xff0d`，Escape 是 `0xff1b`。
- `mask`：统一使用 Rime modifier mask，保留 Shift、Control、Alt、Super/Hyper/Meta 和 Release；Lock 位平台可以照传，通用层会把它折进 keysym 后剥掉；NumLock/鼠标状态等噪声在通用层过滤。

通用层只向平台返回两类输出：

- `accepted`：librime 是否接受该按键，平台据此决定是否吞掉原生事件。
- `ImeState`：提交文本、preedit、cursor、选区、候选、highlight、page、select keys、ascii mode。

候选选择、翻页、清空、提交、Enter 和 ascii mode 也走 session API。平台 UI 不直接读写 librime 状态，只把用户动作转成 `select_candidate_on_page()`、`highlight_candidate_on_page()`、`change_page()`、`clear_composition()`、`commit_composition()`、`process_enter()` 或 `set_ascii_mode()`。

## 线程模型

librime 的 C API 没有线程安全承诺：`Service`、`ConfigComponent`、`DictionaryComponent` 都是进程级单例，内部缓存不加锁。`keytao-core` 因此持有一把进程级可重入锁 `RIME_API_LOCK`，把**全部** `rime_api` 调用序列化：

- `Engine` 的所有方法，以及 `Engine` 的 `Drop`（destroy session 也在锁内完成）。
- `create_session`、`setup`、`initialize`、`reinitialize`、`deploy` 系列、`full_deploy_and_wait`、`validate_deployed_schemas`、运行时版本查询。

`Engine: Send + Sync` 的依据是“由全局锁序列化”，不是 librime 自身线程安全，SAFETY 注释按此描述。

线程模型不变量：

- 平台层不得绕过 `keytao-core` 直接调用 librime。
- 多后端（Linux daemon 的三条后端线程、Windows 的后台引擎线程、Android 的 IME 线程）共享同一个 `ImeRuntime` 即可，不需要各自再加锁。
- 锁只覆盖单次 librime 调用或一个逻辑事务（`process_key` + `extract_state`），不跨慢 IO 持有。
- reload 会用读写屏障短暂阻塞该 runtime 上的全部 session 调用，因此 reload 的**检测与触发都必须在按键同步路径之外**。
- `keytao-core` 内不再有 `lock().unwrap()`；中毒锁一律 `PoisonError::into_inner()` 后继续使用，避免 panic 穿过 FFI 边界导致进程 abort。

## 代码分层

当前系统输入法由四层组成：

| 层级 | 当前位置 | 职责 | 不应承担 |
| --- | --- | --- | --- |
| IME runtime + Rime wrapper | `crates/keytao-core` | librime setup/deploy/session、`ImeRuntime`、reload generation、modifier mask、`ImeState` 抽取、用户目录/共享目录、配置合并工具 | 平台协议、窗口绘制、系统安装 |
| C FFI | `crates/keytao-core-ffi` | 给 Swift/C/其它语言提供 per-session C ABI，并复用 `ImeRuntimeSession` | 平台策略、UI 样式、按键猜测 |
| Platform frontend | `crates/keytao-linux-ime`、`crates/keytao-macos-ime`、`crates/keytao-windows-ime`、`src-tauri/gen/android/app` | 系统输入法协议、按键转换、提交文本、更新 preedit、候选窗/候选服务、日志诊断 | Rime 业务状态、配置合并、跨平台视觉语义 |
| App integration | `src-tauri/src/lib.rs` 和 React UI | 下载安装方案、触发部署、状态展示、通过 `keytao_core::ReloadStamp::write()` 写 reload stamp、种子写入用户可编辑的 `theme.yaml` / `keyboard.yaml`；Linux 可在启动时拉起 fallback daemon | 直接接管系统按键热路径、自己拼 reload stamp 签名、在正式 UI 暴露系统输入法安装/卸载/重启按钮 |

平台前端之间可以使用完全不同的系统协议，但必须共享同一套输入输出语义：

```text
native key event
  -> X11 keysym + Rime modifier mask
  -> keytao-core ImeRuntimeSession
  -> ImeState + accepted
  -> platform commit/preedit/candidate adapter
```

## Runtime/Core 契约

`keytao-core` 是平台无关核心。所有桌面平台都应通过它进入 librime，并通过 `ImeRuntime` 管理 IME session 调度。

### `deploy(user_data_dir, shared_data_dir)`

- 必须在创建 session 前成功执行。
- 进程内 `setup()` + `initialize()` 只在第一次 deploy 时执行；后续 deploy 只重新 `full_deploy_and_wait()`。
- `user_data_dir` 是 KeyTao 自有用户目录，不默认复用其它输入法的用户目录。
- `shared_data_dir` 必须包含基础 Rime 数据，至少要有 `default.yaml`。
- 调用方应把 deploy 放在后台线程或平台允许阻塞的位置。

### `Engine`

- 一个 `Engine` 对应一个 librime session。
- 每个输入上下文必须独立创建 session，不能多个客户端共享一个 session（已登记的平台偏离见《不变量》）。
- `Engine` / `ImeRuntimeSession` 的公开操作是唯一的 session 状态入口：

| 分类 | 操作 |
| --- | --- |
| 按键 | `process_key_result(keycode, mask)`、`process_enter()` |
| 只读 | `state()`、`raw_input()`、`is_ascii_mode()`、`current_schema_name()`、`input_policy()`、`all_candidates_limited(max)` |
| 候选 | `select_candidate_on_page(index)`（`select_candidate` 是其别名）、`highlight_candidate_on_page(index)`、`delete_candidate_on_page(index)`、`change_page(backward)` |
| 组字收尾 | `commit_composition()`、`clear_composition()`（`reset()` 是其别名）、`commit_raw_input()` |
| 模式与策略 | `set_ascii_mode(enabled)`、`set_input_policy(policy)` |

候选、翻页、清空、提交一律调用 librime 官方 API（`RimeSelectCandidateOnCurrentPage`、`RimeHighlightCandidateOnCurrentPage`、`RimeDeleteCandidateOnCurrentPage`、`RimeChangePage`、`RimeCommitComposition`、`RimeClearComposition`、`RimeGetInput`），不再合成按键；只有旧 ABI 缺对应函数指针时才退回合成按键。这意味着通用层不再依赖 `default.yaml` 里 `-`/`=` 的 `key_binder` 绑定、也不依赖 Escape 绑定或 `select_keys` 的长度。

**iOS 的 vendored librime 降级路径**：`vendor/librime/ios` 目前从 librime 1.8.5（源码 commit `08dd95f5`）构建，其 `RimeApi` 结构体尚无 `change_page` 与 `highlight_candidate_on_current_page`（二者是 librime 1.9 才加入的成员）——这在 iOS target 上是**编译期**字段缺失，不是运行期函数指针判空。因此 `keytao-core` 对这两处做 `#[cfg(target_os = "ios")]` 门控降级：`highlight_candidate_on_page` 在 iOS 上是 no-op，`change_page` 在 iOS 上退回合成 `-`/`=` 翻页键；其余平台（macOS/Windows/Linux/Android 用 librime 1.17.x）一律走官方 API。这意味着 D4「走官方 API 而非合成按键」在 iOS runtime 升级到 1.9+ 之前无法在 iOS 上真正生效。升级 `vendor/librime/ios` 到 1.17.x 后应移除该门控（见《当前已知差异与收敛点》与 `docs/ime-convention-compliance-report.md` 的遗留事项）。

平台前端不应直接访问 librime context/menu/status，也不应绕过 core 自己解析候选。

### `ImeRuntime`

`ImeRuntime` 是输入法通用运行时，负责把“什么时候部署、什么时候重载、session 什么时候刷新”从平台层收回来。

当前职责：

- `ImeRuntime::new()`：使用平台默认用户目录和共享数据目录。
- `ImeRuntime::with_dirs(user_data_dir, shared_data_dir)`：用于 macOS/Windows/测试等明确指定目录的场景。
- `init()`：首次部署并初始化 librime。
- `reload()`：重新部署词库并递增 generation。
- `init_without_deploy()`：只加载 App 已部署的方案；未安装或未部署时拒绝初始化。
- `reload_without_deploy()`：只重载现有编译产物并递增 generation，不在输入法进程中部署。
- `reload()`：`reload_without_deploy()` 的语义再加一次部署。
- `create_session()`：为输入上下文创建 `ImeRuntimeSession`，并把它登记进 runtime 的 session 注册表。

`reload_without_deploy()` 的完整语义是：

```text
持全局锁 + reload 写屏障
  -> 遍历 session 注册表，丢弃每个存活 session 的 Engine（记住它的 ascii_mode）
  -> RimeFinalize
  -> 重新 setup / initialize
  -> generation += 1
  -> 各 session 下次访问时懒重建 Engine，并写回重建前的 ascii_mode
```

必须走到 finalize/initialize 的原因：librime 的 `ConfigComponent` / `DictionaryComponent` 用 `weak_ptr` 缓存编译产物，只要进程内还有一个存活 session，新建 session 就会拿到旧产物；只递增 generation 并重建 session 不能让新词库生效。代价是 macOS 上实测约 20–60ms（三线程并发时），且 reload 期间该 runtime 的 session 调用会被短暂阻塞。

`ImeRuntimeSession` 当前职责：

- 在每个公开操作前检查 generation，发现变化时自动重建内部 `Engine`，让新词库实时生效。
- 重建时迁移重建前的 `ascii_mode`（不再强制复位为中文）。
- 在 `process_key_result()` 中先跑 `key_policy::normalize_key_for_modifiers`（折叠 Shift/CapsLock 的大小写并剥掉 Lock 位），再过 `rime_modifier_mask`。
- 持有 `InputContextPolicy`，`composing == false` 时按键完全不进 librime（见《敏感输入上下文》）。策略跨 Engine 重建存活。

平台层只需要持有 runtime/session，不需要自己维护 generation、reload 后重建 session、modifier mask 过滤或大小写补偿。

## FFI 契约

非 Rust 平台前端应优先使用 `keytao-core-ffi` 的 per-session API。FFI per-session 已经复用 `ImeRuntimeSession`，所以 Swift/C 层不需要直接管理 librime session。Android 的 Kotlin 侧走 `src-tauri` 里对称的 `Java_..._KeytaoNativeBridge_native*` JNI 导出，语义与同名 C 入口一致。

### panic 边界

- `keytao-core-ffi` 的**全部** `extern "C"` 导出（含 `keytao_free_string` / `keytao_free_state`）与 `src-tauri` 的**全部** `Java_*` JNI 导出都在 `catch_unwind(AssertUnwindSafe(..))` 内。
- panic 时返回 null / false / 0，并记一条 error 日志：C 侧写 stderr，Android 侧写 logcat（TAG=`KeytaoNative`）。日志只含 panic 消息，不含按键与提交内容。
- **不变量**：平台拿到 null / false 必须按“本次操作没有发生”处理，保留上一次的 UI 状态，不能当成崩溃或据此销毁 session。

### 返回值所有权

1. 每个返回的 `KeytaoState*` 必须调用 `keytao_free_state()`。
2. 每个返回的 `char *`（含所有 `*_json` 入口）必须调用 `keytao_free_string()`。
3. `KeytaoState` 内所有字符串都是 UTF-8 C string；`committed` 和 `select_keys` 用空字符串表示无值。
4. 平台前端持有 session handle 时，应在输入上下文销毁、失焦彻底结束或进程退出时 `keytao_destroy_session()`。

### C ABI 分组

| 分组 | 入口 |
| --- | --- |
| 生命周期 | `keytao_init(user_dir, shared_dir)`、`keytao_is_initialized()`、`keytao_reload()`、`keytao_create_session()`、`keytao_destroy_session(session)` |
| 按键 | `keytao_session_process_key(session, keyval, modifiers)`、`keytao_session_process_enter(session)` |
| 候选 | `keytao_session_select_candidate(session, index)`、`keytao_session_highlight_candidate(session, index)`、`keytao_session_delete_candidate(session, index)`、`keytao_session_change_page(session, backward)`、`keytao_session_all_candidates_json(session, limit)` |
| 组字收尾 | `keytao_session_commit_composition(session)`、`keytao_session_clear_composition(session)`、`keytao_session_reset(session)` |
| 状态与模式 | `keytao_session_state(session)`、`keytao_session_get_ascii_mode(session)`、`keytao_session_set_ascii_mode(session, enabled)` |
| 输入策略 | `keytao_session_set_input_policy(session, composing, learning)`、`keytao_session_input_policy_composing(session)`、`keytao_session_input_policy_learning(session)` |
| key policy | `keytao_text_to_keysym(utf8)`、`keytao_key_policy_is_enter(keyval)`、`keytao_key_policy_should_bypass(session, keyval, modifiers)`、`keytao_utf16_offset_from_chars(text, char_offset)` |
| reload stamp | `keytao_reload_stamp_path()` / `_signature()`、`keytao_reload_stamp_path_at(user_dir)` / `_signature_at(user_dir)`、`keytao_reload_stamp_changed()`、`keytao_reload_if_stamp_changed()` |
| 进程级注入 | `keytao_set_ui_capabilities(...)`、`keytao_set_theme_paths(default, user)`、`keytao_set_system_color_scheme("dark"/"light"/null)` |
| 主题 | `keytao_resolve_theme_json(...)`、`keytao_resolve_keyboard_json(...)`、`keytao_default_keyboard_yaml()` |
| 释放 | `keytao_free_state(state)`、`keytao_free_string(ptr)` |

上表中除只读查询与释放外，绝大多数入口都另有一个 `*_json` 版本，返回带 `CandidatePanelModel` / `ModeHintModel` 的完整 UI JSON，供自绘前端使用。

### 进程级注入必须在第一次取 UI model 之前完成

JSON 状态路径的三项进程级设置默认值是为软键盘候选条准备的，桌面前端不注入会静默拿到错误结果：

- `keytao_set_ui_capabilities(supports_custom_colors, supports_vertical, supports_hover, supports_shadow, supports_separator, system_lookup_table_only)`：未声明时 `supports_vertical = false`，`theme.yaml` 里的 `orientation: vertical` 会**静默失效**。自绘竖排候选窗的前端（macOS、Linux overlay、Windows）必须先声明。
- `keytao_set_theme_paths(default_theme_path, user_theme_path)`：不注入时 UI model 用的是内置默认主题，而不是用户的 `theme.yaml`。
- `keytao_set_system_color_scheme("dark" / "light")`：不注入时 `keytao-theme` 会自己探测系统外观，macOS/Linux 上是 1 秒节流的 `defaults` / `gsettings` 子进程——输入法进程内应当避免。传 null 可退回自动探测。

### JNI 侧对应关系

Kotlin 用到的入口与 C 侧一一对应，命名去掉 `keytao_` 前缀改为 `native` 驼峰：`nativeProcessKey` / `nativeProcessEnter` / `nativeSelectCandidate` / `nativeHighlightCandidate` / `nativeDeleteCandidate` / `nativeChangePage` / `nativeCommitComposition` / `nativeClearComposition` / `nativeReset` / `nativeSetInputPolicy` / `nativeInputPolicyComposing` / `nativeInputPolicyLearning` / `nativeTextToKeysym` / `nativeIsEnterKey` / `nativeShouldBypassKey` / `nativeUtf16OffsetFromChars` / `nativeReloadStampPath(userDir)` / `nativeReloadStampSignature(userDir)` / `nativeAllCandidates(session, limit)`。

### 旧 singleton 入口

FFI 里仍保留旧的 module-level singleton API（`keytao_process_key()`、`keytao_select_candidate()`、`keytao_change_page()`、`keytao_reset()`）；它们内部也复用一个 `ImeRuntimeSession`。新平台前端不要使用这些旧入口，除非是单上下文工具。

## 统一状态模型

`ImeState` 是所有平台前端的唯一 UI/提交输入：

| 字段 | 含义 | 平台应用规则 |
| --- | --- | --- |
| `committed` | 本次操作产生的提交文本 | 非空时先清旧 preedit，再通过平台原生提交接口写入客户端 |
| `preedit` | 当前预编辑文本 | 用平台 composition/marked text/preedit API 更新；为空时清除当前 preedit |
| `cursor` | preedit 光标位置，**单位是 Unicode 标量（char）偏移** | 由通用层从 librime 的 UTF-8 字节偏移换算而来；平台自行换到原生单位，见下表 |
| `sel_start` / `sel_end` | preedit 内已转换/选中段的范围，同样是 Unicode 标量偏移 | 无选中时两者相等；macOS 据此做 IMK 惯例的分段 marked text 下划线 |
| `candidates` | 当前页候选 | 显示候选文本和 comment；空则隐藏候选窗或 lookup table |
| `highlighted_candidate_index` | 当前高亮候选 | 映射为高亮、lookup table cursor 或默认空格选择目标 |
| `page` | 当前候选页 | 用于上一页按钮/状态 |
| `is_last_page` | 是否末页 | 用于下一页按钮/状态 |
| `select_keys` | Rime 候选选择键 | **只用于候选 label**；为空时 label 兜底到 `1234567890`（常量 `DEFAULT_SELECT_KEYS`）。按键拦截**不得**兜底，见《候选交互规范》 |
| `ascii_mode` | 当前中英模式 | 只驱动状态显示和模式提示，不替代 Rime 状态，也不是 bypass 判据 |

`all_candidates` 字段已从 `ImeState`、C 结构体、FFI JSON 与 Android JSON 中彻底移除（它长期恒空，各平台据此写了走不到的分支）。需要完整候选列表时按需拉取：`all_candidates_limited(max)` / `keytao_session_all_candidates_json(session, limit)` / `nativeAllCandidates(session, limit)`。

### `committed` 的交付契约

- 只读查询 `state()` 不再调用 `RimeGetCommit`，其返回的 `committed` 恒为 `None`。
- 提交文本只出现在 `process_key_result` / `process_enter` / `select_candidate_on_page` / `select_candidate_global` / `change_page` / `commit_composition` / `clear_composition` / `commit_raw_input` / `reset` / `set_ascii_mode` 的返回值中，且只出现一次。
- 平台必须在这些返回值上应用 `committed`；把它们的结果丢掉再去 `state()` 补查会丢字。

### 偏移单位换算

| 平台 | 原生单位 | 换算方式 |
| --- | --- | --- |
| Linux IBus | Unicode 标量 | 直接使用 `cursor`，只需 clamp 到 `preedit.chars().count()` |
| Linux Wayland text-input v1/v2 | UTF-8 字节 | 后端的 `preedit_cursor_bytes()` |
| macOS / Windows / Android / iOS | UTF-16 code unit | `keytao_utf16_offset_from_chars` / `nativeUtf16OffsetFromChars` |

平台**不得**自己写换算。Rust 侧直接用 `keytao_core::{char_offset_from_utf8, utf16_offset_from_chars}`。

### 状态应用顺序

必须固定：

1. 如果 `committed` 非空，并且平台有旧 composition/preedit，先清掉旧 preedit。
2. 提交 `committed`。
3. 设置新的 `preedit`、`cursor` 和选区。
4. 更新 `ascii_mode`。
5. 根据 `candidates` 显示或隐藏候选 UI。
6. 返回或记录 `accepted`，决定原生按键是否继续传给客户端。

这个顺序对顶功很重要：一次按键可能同时返回 `committed` 和新的 `preedit`。平台实现不能先设置新 preedit 再提交旧文本。

另一条同源不变量：**提交 `committed` 之后必须在同一次协议提交里写回新的 `preedit` 与 `cursor`**，不能只发提交、把新 preedit 留到下一次按键。Linux 的 input-method-v2 曾因绕过这条把顶功后的新 preedit 丢掉，现在三条路径统一收敛到一个 `apply_state_to_input_method()`（commit_string → set_preedit_string → commit(serial)）。

## 按键事件规范

所有平台前端应尽量向 librime 发送同一形状的事件：

```text
keycode = X11 keysym
mask = Rime modifier mask
```

当前 modifier mask（值与 librime `key_table.h` 一致）：

| 名称 | 值 | 含义 |
| --- | --- | --- |
| `Shift` | `0x0001` | Shift pressed |
| `Lock` | `0x0002` | CapsLock toggle。平台可以照传，通用层的 `key_policy::normalize_key_for_modifiers` 会把它折进 keysym 后剥掉，**不会发给 Rime** |
| `Control` | `0x0004` | Control pressed |
| `Alt` / `Mod1` | `0x0008` | Alt/Option pressed |
| `Super` | `1 << 26` | macOS 的 Command、Windows 的 Win 键统一映射到这里 |
| `Hyper` | `1 << 27` | X11 惯例保留 |
| `Meta` | `1 << 28` | X11 惯例保留 |
| `Release` | `1 << 30` | key release event，主要用于 Shift release |

特殊键应使用 X11 keysym：

| 键 | keysym |
| --- | --- |
| Return | `0xff0d` |
| Backspace | `0xff08` |
| Delete | `0xffff` |
| Escape | `0xff1b` |
| Space | `0x0020` |
| Tab | `0xff09` |
| Left / Up / Right / Down | `0xff51` / `0xff52` / `0xff53` / `0xff54` |
| Home / End | `0xff50` / `0xff57` |
| PageUp / PageDown | `0xff55` / `0xff56` |
| Shift_L / Shift_R | `0xffe1` / `0xffe2` |

### printable key 规则

- 平台能拿到布局后的 printable 字符时，应优先传实际字符的 keysym。
- Shift+a 在 macOS/Linux 当前目标行为是传 `0x41` 加 Shift mask，让 librime 的 ASCII composer 自己决定行为。
- Windows TSF 也应对 Shift+字母传大写 ASCII keysym，同时保留 Shift modifier mask；后续如果接入完整键盘布局转换，应继续保持“keysym 表示实际字符、mask 表示修饰键”的语义。
- NumLock 不进 Rime modifier mask。CapsLock 由平台把 `Lock` 位放进 mask 即可，通用层的 `key_policy::normalize_key_for_modifiers` 会按 caps XOR shift 折出实际字母并剥掉 Lock 位；平台不要再写自己的大小写补偿。
- 文本到 keysym 的映射只有一份实现：`key_policy::keysym_for_char` / `keysym_for_text`（经 FFI 是 `keytao_text_to_keysym`，经 JNI 是 `nativeTextToKeysym`）。ASCII 与 Latin-1 直通，其余用 `0x01000000 | codepoint`，控制字符返回 0/`None`。
  - **返回 0 表示“不要送 Rime，直接上屏”**。
  - 非 ASCII 现在会得到合法 keysym 并被送进 Rime，Rime 多半不接受，**平台必须在 `accepted == false` 时把原文直接上屏**，否则字符会丢。
  - 这条同时消除了 Android 曾出现的错误：`（`（U+FF08）被当成 `XK_BackSpace`（0xff08）。

### bypass 规则

统一契约：**任何模式下按键都先进 Rime，Rime 未接受的可打印键再由平台按原生路径放行**。判定只有一份实现 `key_policy::should_bypass_empty_composition`（经 FFI 是 `keytao_key_policy_should_bypass`，经 JNI 是 `nativeShouldBypassKey`）。

没有 active composition 时提前放行的只有两类：

- 导航/编辑类 nonstarter 键：Space、Return、Backspace、Delete、Tab、Escape、Home..Begin（`0xff50..=0xff58`）、方向键、翻页键。
- 系统保留修饰键组合：含 Super / Hyper / Meta（Cmd 系、Win 系）的组合，由 `key_policy::is_system_reserved_modifier` 判定。

明确不再放行、必须先送 Rime 的：

- **Ctrl / Alt / Option 组合键**。Rime 自己的热键（`Ctrl+grave` 切方案、F4、key_binder 里的 emacs 编辑键）此前永远进不了 librime，现在一律先送、按 `accepted` 决定吞否。
- **`ascii_mode` 不是 bypass 判据**。英文模式下按键仍需进 `ascii_composer`，否则 Rime 侧的开关键与标点规则失效。macOS 与 Android 的“ascii 模式整段绕过 core”私货已删除。

有 active composition 或 candidates 时，上述按键交给 Rime 或平台候选交互处理。

平台侧的两点必要补充：

- Windows TSF 的 `OnTestKeyDown` 是**刻意放宽的超集**：所有可打印键与所有非系统保留的修饰组合都声明拦截，真正的放行发生在 `OnKeyDown`，由“`accepted` 或产生 commit 或 preedit/候选/高亮/页码确实变化”决定。原因是 TSF 的语义是 `TestKeyDown && fEaten` 才会调 `KeyDown`，测试回调保守会**永久性**丢功能（标点、key_binder 绑定都到不了 Rime）。任何有“测试回调”的平台协议都应照此处理。
- 未映射的键（如小键盘 Enter）在**有 composition 时不得直接放行**：先补上正确的 keysym（keypad Enter 是 `0xff8d`）交给 Rime，仍未接受再按 Enter 的 fallback 处理。

### Shift release

当前中英切换基线：

1. Shift key down 不切换模式。
2. 只有没有混入其它 keyDown 的 solo Shift release 才送入 Rime。
3. 送入 `Shift_L` 或 `Shift_R` keysym，mask 为 `Release`。
4. 如果 Rime 不接受，而平台需要兜底，可以调用 `set_ascii_mode(!ascii_mode)`。

平台前端必须区分 solo Shift 和 Shift+letter/number/symbol。Shift+其它按键必须走普通按键路径。

## 候选交互规范

候选行为由 core 统一，平台只负责把用户动作转成 session 调用：

- **Enter**：唯一语义是 `process_enter()`。它把 `XK_Return` 交给 Rime，只有 Rime 未接受且确实在组字时才由 core fallback 到 `commit_raw_input()`（取 `RimeGetInput` 提交），并保留 Rime 本次已产出的 commit 不丢字。平台**不得**自己拼“提交 preedit 字面量”，也不得自己合成 `XK_Return`。
- **数字/选择键**：平台**不得**本地拦截数字键做选词，一律作为普通按键送 Rime，由 schema 的 `select_keys` / `key_binder` 决定。`select_keys` 为空时数字属于编码，通用层不做 `1234567890` 兜底（兜底只属于候选 label）。
- **空格**：有 candidates 时可选择 `highlighted_candidate_index`；建议同样交给 Rime 的 selector。
- **上一页/下一页**：调用 `change_page(backward)`，不要在平台层伪造 page state，也不要依赖 `-`/`=` 的 key_binder 绑定。
- **Escape/cancel**：调用 `clear_composition()`，清 preedit，隐藏候选。
- **鼠标点击候选**：调用 `select_candidate_on_page(index)`，再按 `ImeState` 应用结果。五端中 Linux 自绘面板仍未实现点选，属遗留。
- **hover 高亮**：如需回写 Rime 高亮，用 `highlight_candidate_on_page(index)`；只做视觉态时不要调，否则会改变后续 Enter/空格选中的是哪一项。

如果平台候选服务只支持结构化 lookup table，不支持自绘样式，也仍应保持 label、candidate、highlight 和 page 语义一致。

### 失焦与上下文结束的处置矩阵

结束当前组字只有两条路径，**不得伪造 `XK_Return` / `XK_Escape`**：

- 需要提交 → `commit_composition()`。
- 需要丢弃 → `clear_composition()`。

| 平台 | 触发信号 | 处置 |
| --- | --- | --- |
| macOS | `deactivateServer:`、`commitComposition:` | 提交 |
| macOS | `cancelComposition` | 丢弃 |
| Windows | `Deactivate`、`OnKillThreadFocus` | 提交；必须用**同步** write session，失败退回 `ITfContextOwnerCompositionServices::TerminateComposition`。普通 focus 切换才用异步 edit session |
| Linux IBus / GNOME | `focus_out`、`reset`、`disable` | 丢弃；引擎自己清，preedit focus mode 只发 `CLEAR(0)`，不发 `COMMIT(1)`，否则客户端会重复上屏 |
| Linux Wayland v1 | `reset` / 失焦 | 把当前 preedit 放进 `preedit_string` 的第三个参数（协议定义的“客户端在 reset 时用来替换预编辑的提交文本”），输入法自己只 `clear_composition()`，提交与否交给合成器 |
| Android | 输入上下文结束 | 丢弃用 `clear_composition()`，需要提交用 `commit_composition()` |
| iOS | `viewWillDisappear`、`textWillChange`、`selectionWillChange` | 一律丢弃；键盘收起时宿主往往已不接受插入。同时必须清掉宿主 marked text |

无论走哪条路径，平台都必须同步清除自己的 preedit / marked text 与候选 UI；只 reset core 而不清客户端 marked text 会留下残留下划线。

Windows 的时序约束值得单列：`Deactivate` 返回后 TSF 立即释放 client id，`OnKillThreadFocus` 之后该线程不再泵 edit session，所以这两处不能用异步 edit session 收尾。这是平台特有的，但对“失焦必须确定性收尾”这个通用要求是必要补充。

### 按键重复

输入法抓取键盘后，**被 IME 消费的按键必须由输入法自己实现重复**——合成器/系统不再替你重复。Linux 的两个 Wayland 后端已按 `repeat_info` 的 rate/delay 加 `xkb_keymap_key_repeats` 实现；KWin 的 grab 键盘是 `wl_keyboard` v1、早于 `repeat_info`(v4)，需要按 xkb/Plasma 默认值（delay 600ms、rate 25/s）兜底。目前只有 Linux 落地，其它平台可据此自查。

## 敏感输入上下文

密码框、PIN 框和声明了隐私意图的输入框不得走中文组字，也不得让内容进入用户词库、剪贴板历史或输入历史。

通用层提供 `InputContextPolicy { composing: bool, learning: bool }`：

- `Default` 是两者皆 true；`InputContextPolicy::sensitive()` 是两者皆 false。
- `set_input_policy(policy)`（FFI `keytao_session_set_input_policy`，JNI `nativeSetInputPolicy`）。从 `composing = true` 切到 false 时，core 会先 `clear_composition()` 并把要应用的状态返回给平台清 UI。
- `composing == false` 时按键**完全不进 librime**，直接返回 `accepted = false` 加一份只读快照（保留 `ascii_mode` / `schema_name` 供模式提示），因此不产生 preedit/候选，也不可能发生用户词学习。
- 只读查询：`input_policy()` / `keytao_session_input_policy_composing` / `_learning` / `nativeInputPolicyComposing` / `nativeInputPolicyLearning`。
- 策略存在 session 内部，跨 Engine 重建（reload）存活。

**已知限制**：librime 没有 per-session 关闭用户词记忆的开关（`memorize` 是 translator 的 schema 级配置）。因此 `learning = false` 只有在 `composing = false` 时被真正强制；单独把 `learning` 置 false 而保持 `composing = true` 时，它只是一个交给平台自律的标志，用于关闭剪贴板记忆、建议和输入历史。

平台检测来源：

| 平台 | 检测来源 | 命中规则 |
| --- | --- | --- |
| Linux IBus | `ContentType` **属性** `(uu)`，走 `org.freedesktop.DBus.Properties.Set` | `purpose == PASSWORD(8) \|\| purpose == PIN(9) \|\| (hints & PRIVATE(1<<11)) != 0` |
| Linux Wayland v2 | `zwp_input_method_v2.content_type`，**text-input-v3 编号** | purpose password=8 / pin=9；hint hidden_text=0x40 / sensitive_data=0x80 |
| Linux KDE v1 | `zwp_input_method_context_v1.content_type`，**text-input-v1 编号** | purpose password=8（v1 没有 pin，9 是 date）；hint 同 0x40 / 0x80，password 简写 0xc0 |
| Linux XIM | 无 | **XIM 协议不表达输入用途，X11/XWayland 路径无法检测密码框**，属能力缺口而非实现缺陷 |
| Windows | `GUID_COMPARTMENT_KEYBOARD_DISABLED`（Chromium/Edge/WebView2/Electron 的密码框主力路径）、`GUID_COMPARTMENT_EMPTYCONTEXT`、`GUID_PROP_INPUTSCOPE` 上的 `IS_PASSWORD` | 任一命中即敏感；焦点文档为空同样视为禁用。InputScope 只在焦点变化时用同步只读 edit session 查，不在按键路径 |
| macOS | 无需检测 | 系统 secure input 下 IMK 自动切走。仍需确认代码无按键内容日志 |
| Android | `inputType` 的四个 password 变体 | 直通（`composing = false, learning = false`） |
| Android | `IME_FLAG_NO_PERSONALIZED_LEARNING`、`TYPE_TEXT_FLAG_NO_SUGGESTIONS` | 只置 `learning = false` 并关闭剪贴板记忆/建议与输入历史，**保留组字能力** |
| iOS | `textDocumentProxy.isSecureTextEntry` | 直通 |
| iOS | `keyboardType` 为 numberPad / decimalPad / phonePad / namePhonePad / asciiCapableNumberPad / numbersAndPunctuation / emailAddress / URL / webSearch | 直通 |

三条容易照抄错的口径：

- **iOS 的 `.asciiCapable` 不属于直通集**。该值只表示键盘可以显示 ASCII，很多宿主在仍需中文的输入框上也会设它。
- **Android 的 `TYPE_TEXT_FLAG_NO_SUGGESTIONS` 不走完全直通**。对中文输入法而言，`textNoSuggestions` 字段（用户名、编号等）大量存在，完全直通等于让用户在这些字段里无法输入中文，危害大于它要解决的隐私问题；AOSP `LatinIME` 的 `InputAttributes` 同样只据此关闭建议与词典学习，不关闭组字。真正的隐私契约（密码框）没有放宽，与 iOS 的 `isSecureTextEntry` 口径一致。
- Wayland 两套 `content_type` 常量表与 IBus 的 `(purpose, hints)` **三者互不通用**，跨端只共享“命中即 `InputContextPolicy::sensitive()`”这个结论。

平台除了 `set_input_policy` 之外还要做两件事：切进敏感上下文时立刻清候选/preedit UI；按键入口第一件事就检查 `input_policy().composing`，为 false 时直接放行且不发任何 UI 信号。剪贴板服务另需尊重 `ClipDescription.EXTRA_IS_SENSITIVE`（Android）与 `hasFullAccess` / `UIPasteboard.hasStrings` 探测（iOS）。

## 数据目录与部署

桌面平台使用独立 KeyTao 用户目录：

| 平台 | 用户目录 |
| --- | --- |
| macOS | `~/Library/keytao` |
| Linux | `$XDG_DATA_HOME/keytao`，通常是 `~/.local/share/keytao` |
| Windows | `%APPDATA%/keytao` |

移动端不写死绝对路径，用应用私有目录下的 `keytao/`：

| 平台 | 用户目录 |
| --- | --- |
| Android | 应用私有目录下的 `keytao/`：外部私有目录优先（`getExternalFilesDir(null)/keytao`，便于用户手工编辑），外部存储不可用时退回 `filesDir/keytao`。App 与 `:ime` 同 UID 共享，全程不需要任何存储权限 |
| iOS | App Group 容器下的 KeyTao 用户目录，主 App 与键盘扩展共享 |

用户目录下的固定成员：`rime-data/`（部署产物）、`keytao-ime.reload`（reload stamp）、`theme.yaml`、`keyboard.yaml`，以及各平台的 IME 设置 JSON。

共享数据目录由平台查找：

- 优先显式环境变量：`KEYTAO_RIME_SHARED_DATA_DIR`、`RIME_SHARED_DATA_DIR`、`RIME_DATA_DIR`。
- 再查 App 或 IME bundle/runtime 内的 `rime-data` / `SharedSupport` / `share/rime-data`。
- 最后 fallback 到系统 Rime 数据目录，例如 Linux `/usr/share/rime-data`、macOS Squirrel/Homebrew、Windows Weasel。

App 的方案安装只写文件；部署才调用 `keytao_core::deploy()`。任何平台前端都不应自行合并 `default.custom.yaml` 或 `rime.lua`。

状态判定分为三层：选中的 KeyTao 方案源文件完整才是“已安装”；对应 `build/<schema>.schema.yaml` 全部存在才是“已部署”；只有系统组件与已部署方案都就绪才可输入。孤立的 `build` 残留不能反向证明方案已安装。

桌面部署不能只相信 `full_deploy_and_wait()` 的全局返回值。`keytao-core` 会从当前 schema 的编译产物读取 `schema/dependencies`，递归部署每个依赖 schema，并在完成后验证主 schema 与全部依赖的 `build/*.schema.yaml`；随后创建真实 session、选择目标 schema 并核对 session status。任一依赖缺失、schema 选择失败或实际 schema 不匹配都会让部署失败，不能把“源文件已安装”误报为“方案可输入”。

## 打包规范

桌面发行包必须把“能部署”和“能输入”需要的 runtime 一起打包，不能让主 App 和系统 IME 使用不同能力集：

- `librime` native library。
- OpenCC 数据和运行时依赖。
- `rime-plugins`，尤其是 Lua 插件。
- 基础 `rime-data`，至少包含 `default.yaml`、`key_bindings.yaml`、`punctuation.yaml`、`symbols.yaml`、`essay.txt` 和 OpenCC 数据。

平台约束：

- macOS 只构建 pkg。pkg 同时安装主 App 和 `/Library/Input Methods/KeyTao.app`，并要求安装后注销、重新登录当前用户会话；不构建 dmg，因为 dmg 拖拽安装无法可靠完成系统输入法注册。
- Linux 只构建 deb 和 rpm，不构建 AppImage 或 tarball。deb/rpm 通过 Tauri resource 放入 `runtime/`，并同时包含主 App、系统 IME 和完整 runtime。
- Windows 继续使用 installer 方式，并应保持 `resources/rime-data` 和 runtime DLL 闭包完整。
- Android 通过 `scripts/android-librime-runtime.sh` 导入 ABI 对应的 `librime.so` 闭包，Gradle 同步到 `jniLibs/<abi>`，基础 `rime-data` 同步到 APK assets 后由 `InputMethodService` 解包到用户目录下的 `rime-data`。

macOS release CI 必须执行 `pnpm build:macos` 和 `scripts/verify-macos-pkg.sh target/keytao-macos-pkg/KeyTao.pkg`，再上传 `keytao-app-<version>-macos-<arch>.pkg`。当前脚本按 runner 架构构建，例如 `macos-arm64` 或 `macos-x86_64`；不要让 Tauri 的 `dmg` bundle 重新进入 macOS 发行流程。

`keytao-core` 不关心打包格式；它只要求传入可靠的 shared data dir。平台 App/IME 启动代码必须优先选择包内 runtime，再 fallback 到环境变量或系统目录。

## Reload 规范

输入法通过用户目录下的 reload stamp 感知 App 部署：

```text
<user_data_dir>/keytao-ime.reload      // 常量 keytao_core::RELOAD_STAMP_FILE_NAME
```

### 唯一实现在 `keytao-core`

路径、写入、签名、变更检测都只有一份实现，**平台与 App 都不得再自己拼**：

| 用途 | Rust | C ABI | JNI |
| --- | --- | --- | --- |
| 路径 | `ReloadStamp::path(user_dir)` / `default_path()` | `keytao_reload_stamp_path()` / `_path_at(user_dir)` | `nativeReloadStampPath(userDir)` |
| 写入（只有 App 调） | `ReloadStamp::write(user_dir)` / `write_default()` | — | — |
| 签名 | `ReloadStamp::current_signature(user_dir)` / `signature_at(path)` | `keytao_reload_stamp_signature()` / `_signature_at(user_dir)` | `nativeReloadStampSignature(userDir)` |
| 变更检测 | `ReloadStampWatcher::has_changed()` | `keytao_reload_stamp_changed()` | — |
| 检测并 reload | — | `keytao_reload_if_stamp_changed()` | — |

签名格式固定为 `<len>:<mtime_nanos>:<fnv1a64 hex16>`，同时挡住“粗粒度时间戳”和“长度与 mtime 相同但内容变了”两种漏检。**stamp 缺失不算 reload 请求**（`signature` 返回空，`has_changed()` 返回 false）。

带 `_at(user_dir)` 的入口不依赖 `keytao_init` 是否成功，供前端在初始化之前建立基线、或用于文件系统事件监听。`keytao_reload_if_stamp_changed()` 只在 reload 真的执行成功后才把请求标记为已消费，一次失败的 reload 会在下个 tick 重试而不是被吞掉。

App 侧同理：`ReloadStamp::write()` 是唯一写入口，此前 App 写纳秒、Android 写毫秒、iOS 写纳秒三套格式已合并，App 报给前端的 `reloadStampSignature` 和输入法比较的签名从此是同一个函数算出来的。

### 检测必须在按键路径之外

reload 会持全局锁并短暂阻塞该 runtime 的全部 session 调用（macOS 实测 20–60ms），stamp 检测本身也是一次 `stat`。因此：

| 平台 | 检测方式 |
| --- | --- |
| Linux daemon | 独立线程上的 `ReloadStampWatcher::has_changed()`，每秒一次 |
| macOS | 后台队列上的 `DispatchSource`（文件存在时监听文件、不存在时监听其目录），`activateServer` 异步兜底检查一次；按键路径没有任何文件 I/O |
| Windows | 按键路径的 `stat` 有 250ms 节流，focus / context / thread focus 回调强制失效缓存 |
| Android | `onStartInputView` 触发检测，但执行在后台线程 |
| iOS | `keytao_reload_if_stamp_changed()`，不在按键同步路径 |

`keytao_reload_if_stamp_changed()` 会 stat 一次文件，适合定时器/DispatchSource，**不适合按键路径**。

用文件系统事件监听的前端有一条实测结论：`ReloadStamp::write` 用 `std::fs::write` 原地重写，**监听目录收不到该事件**。必须监听文件本身，只有文件不存在时才退回监听目录，并在事件后重建监听。

### reload 时必须

1. 清除旧 preedit 和候选 UI。core 只保证 session 懒重建，UI 清理必须由平台主动做——Linux daemon 的做法是用 `reload_bus` 的 eventfd 向各后端广播，各后端执行与失焦相同的清理。
2. 让 session 重建（持有 `ImeRuntimeSession` 时自动发生；持有裸 `Engine` 的代码不受 reload 保护）。
3. 重新读取状态。
4. 不在 UI renderer 或候选点击回调里执行 deploy。

## UI 和 `theme.yaml` 边界

主题系统由 `crates/keytao-theme` 提供。平台前端共享主题语义、默认值、校验和 UI model，不共享绘制实现。

当前结构：

```text
theme.yaml
  -> UI color scheme + accent color + mode variant
  -> keytao-theme::ThemeResolver -> ResolvedImeTheme
ImeState-like input + backend capabilities
  -> CandidatePanelModel / ModeHintModel
ResolvedImeTheme + Model
  -> platform renderer
```

### 两个配置文件的职责

| 文件 | 解析入口 | 产出 | 消费方 |
| --- | --- | --- | --- |
| `theme.yaml` | `resolve_theme_from_paths` / `ThemeResolver` | `ResolvedImeTheme`、`CandidatePanelModel`、`ModeHintModel` | 五平台候选窗与模式提示 |
| `keyboard.yaml` | `keytao_theme::mobile_layout::resolve_mobile_layout_from_paths` | `MobileLayout` | 仅 Android / iOS 软键盘 |

共享的 `ResolvedImeTheme` 及其 FFI JSON **不再包含 `keyboard` 字段**，桌面平台拿到的 theme JSON 不再夹带移动端软键盘布局数据。移动端软键盘的 rows / layers / swipe / longPress 命令属于移动 adapter 配置，由 `keyboard.yaml` 表达，既不进入 `ImeState`，也不进入 `ResolvedImeTheme`。

兼容口径：旧 `theme.yaml` 里的根级 `keyboard:` 段**仍可解析**，把该文件路径传给 `resolve_mobile_layout_from_paths` 即可继续生效（它同时接受 `keyboard.yaml` 文档与带 `keyboard:` 段的 `theme.yaml`）；但 `light:` / `dark:` 变体不再能覆盖键盘布局。

`keyboard.yaml` 的种子写入按分层约定应由 App 负责。当前只有 iOS 键盘扩展在做（且已改为“仅在文件缺失或为空时写、且不在冷启动主线程”），在 App 补上之前扩展侧不能删这段逻辑，否则用户会失去可编辑的布局文件。

`theme.yaml` v2 只表达跨平台可落地语义：

- `ui.colorScheme`：`auto`、`light` 或 `dark`；`auto` 会跟随系统主题，resolved theme 会带上最终 `effectiveColorScheme`。
- `ui.accentColor`：主题强调色，用于派生候选高亮、hover 和模式提示强调色。
- `dark:` / `light:` 模式变体，根级字段仍作为通用配置。
- font family、font size、font weight。
- panel padding、gap、radius、border、shadow、max width、orientation。
- background、foreground、comment、label、highlight、hover、separator、preedit color。
- mode hint size、radius、duration、label、color。

平台映射规则：

- macOS 通过 `keytao-core-ffi` 获取 normalized JSON，再由 AppKit adapter 映射为 `NSColor`、`NSFont`、`NSControl`。
- Linux Wayland/X11/KDE/IBus fallback overlay 通过 `ThemeResolver + CandidatePanelModel / ModeHintModel` 渲染 BGRA buffer。
- Linux IBus/Kimpanel/GNOME 系统候选服务只能映射结构，视觉由桌面环境决定。
- Windows candidate window 和 mode hint window 通过 `ThemeResolver + CandidatePanelModel / ModeHintModel` 渲染 layered window BGRA buffer，但必须尊重 TSF focus/composition 生命周期。
- Android input view 通过 JNI 调用 `keytao-theme` 解析用户目录下的 `theme.yaml`，并消费 `CandidatePanelModel / ModeHintModel`；自绘 `View` 只负责把 resolved theme 和 model 映射到 Canvas。软键盘布局另从 `keyboard.yaml` 取。
- iOS 键盘扩展同样消费 `CandidatePanelModel`，布局取自 `keyboard.yaml`。

### 自绘前端的三条前置条件

FFI 的 JSON 状态路径现在持一个带签名缓存的 `ThemeResolver`，按键路径不再重复解析 YAML。代价是三项进程级设置必须由平台注入（详见《FFI 契约》）：

1. `keytao_set_theme_paths()`——否则 UI model 用的是内置默认主题而不是用户主题。
2. `keytao_set_ui_capabilities()`——否则默认按软键盘候选条形状产模型（`supports_vertical = false`），`orientation: vertical` 静默失效。
3. `keytao_set_system_color_scheme()`——否则 `keytao-theme` 会在输入法进程里 fork `defaults` / `gsettings` 探测系统外观。

### 候选 label 的兜底只属于 label

`select_keys` 用尽或为空时，候选 label 兜底到序号 `1234567890`（常量 `DEFAULT_SELECT_KEYS`）。**按键拦截侧不得兜底**：此时数字属于编码，必须交给 Rime 的 selector。

主题不能控制：

- Rime session 状态。
- 候选选择逻辑。
- 候选数量、分页规则或 select key 来源。
- 光标定位、屏幕边界和输入法窗口生命周期。
- 平台按键转发策略。
- reload/deploy。
- 光标 rect 可信度判断和屏幕边界修正。

## 日志与隐私

输入法进程看得到用户输入的一切，日志必须按“默认什么都不记”设计。

跨平台不变量：

- **release 默认级别（info）下不得出现提交文本、preedit 全文、keysym/keycode 明细**。
- 全文只允许 trace 级；debug 级只允许打字符数与候选数量。
- 日志文件必须在用户私有位置，权限 0700/0600，不得用 `/tmp` 下的固定路径（世界可读且可被抢占）。
- 日志打不开必须降级到 stderr 或系统日志，**不得让输入法启动失败**。
- FFI/JNI 的 panic 日志只含 panic 消息与出错的导出函数名，不含按键与提交内容。

平台落地：

| 平台 | 日志位置 | 说明 |
| --- | --- | --- |
| Linux daemon | `$XDG_STATE_HOME/keytao/log/keytao-ime.log`（默认 `~/.local/state/keytao/log`） | 目录 0700，按天滚动保留 3 天；旧版本的 `/tmp/keytao-ime.log*` 在启动时删除；App 的 `read_debug_logs` 先读状态目录再兼容 `/tmp` |
| macOS | `NSLog` + `~/Library/keytao/log` 下的 librime 日志 | — |
| Windows | 诊断文件需显式打开（`KEYTAO_WINDOWS_IME_DIAGNOSTICS=1`），且按键诊断宏在 release 下展开为 `if false` | release 只写 TSF 生命周期事件 |
| Android | logcat，TAG=`KeytaoNative` 为原生侧 | — |

## 简化后的目标架构

目标是把输入法拆成两类代码：

```text
通用层 keytao-core / keytao-core-ffi
  - librime setup / deploy / reload
  - ImeRuntime / ImeRuntimeSession
  - key event mask normalization
  - ImeState extraction
  - candidate/page/reset/ascii mode operations
  - theme.yaml loader and UI model

平台 IME 层
  - 系统输入法注册和生命周期
  - 原生 key event -> X11 keysym
  - commit / preedit / candidate UI adapter
  - cursor rect and screen bounds
  - platform diagnostics
```

KeyTao App 的理想操作方式：

1. App 安装或更新方案文件。
2. App 调用通用 deploy API 或写入统一 reload request。
3. 正在运行的 IME runtime 收到 reload request。
4. `ImeRuntime::reload_without_deploy()` 加载部署产物并递增 generation。
5. 各平台 session 下一次访问时自动刷新到新词库。
6. 平台层只刷新 UI，不关心 librime 部署细节。

这样 App 可以更灵活地操作 IME：触发部署、查询状态、请求重载、读取诊断；IME 层只负责系统协议、UI 和配置接入。

## 平台接入清单

新增平台前端时按这个顺序实现：

1. 数据目录：确认 `default_user_data_dir()` 和 shared data 查找规则。
2. 初始化：在平台允许的位置创建 `ImeRuntime` 或调用 `keytao_init()`；初始化不得阻塞系统输入法的同步回调线程。
3. Session：为每个输入上下文创建独立 `ImeRuntimeSession`，并在上下文销毁时释放。
4. Key map：把平台原生 key event 转为 X11 keysym + Rime modifier mask；文本到 keysym 一律调 `keytao_text_to_keysym` / `nativeTextToKeysym`，返回 0 时直接上屏。
5. Bypass：调 `keytao_key_policy_should_bypass` / `nativeShouldBypassKey`，不要自己维护键表；Ctrl/Alt 组合与 ascii 模式都不是放行理由。
6. Process：调用 `ImeRuntimeSession::process_key_result()` 或 `keytao_session_process_key()`；Enter 走 `process_enter()`。
7. Apply state：按固定顺序应用 `committed`、`preedit`、`cursor`、`candidates`、`ascii_mode`；`cursor` / `sel_start` / `sel_end` 用 `keytao_utf16_offset_from_chars` 之类的官方换算。
8. Candidate actions：实现 select candidate on page、highlight、change page、clear/commit composition，包括鼠标点选。
9. Mode switch：实现 solo Shift release，必要时提供 ascii mode fallback。
10. Reload：接入 `ReloadStamp`（或其 FFI/JNI 入口），检测放在定时器/文件事件/生命周期回调上，不放按键路径；reload 后主动清 UI。
11. UI model：先调 `keytao_set_theme_paths` / `keytao_set_ui_capabilities` / `keytao_set_system_color_scheme`，再接入 `CandidatePanelModel` / `ModeHintModel`，不要让平台 UI 直接发明 state 字段。
12. 敏感上下文：接入平台的密码/隐私信号，调 `set_input_policy`，命中时清 UI 并让按键直通。
13. 日志：按《日志与隐私》配置路径与级别，release 不打按键与提交内容。
14. Diagnostics：提供状态检查和日志入口，至少能定位 shared data、user data、session init、key event、commit/preedit。
15. Tests：覆盖 key map、bypass、commit+new-preedit、candidate select、reload 后 session refresh、敏感上下文直通。**测试必须在目标 target 上真正编译并运行**——`keytao-linux-ime` / `keytao-windows-ime` 的后端代码全部在 `#[cfg(target_os = ...)]` 下，在 macOS host 上 `cargo test -p <crate>` 会静默跳过全部后端代码（实测：host 0 passed，Linux 容器内 19 passed）。

## 不变量

这些规则不能因平台差异改变：

- 一个输入上下文一个 session。已登记的偏离：Windows TSF（一线程一 session，见 `crates/keytao-windows-ime/IMPL.md` 的《与通用层的已知偏离》表）、Linux Wayland v2 与 KDE v1（协议不提供可区分应用的稳定标识，进程级单 session，见 `crates/keytao-linux-ime/IMPL.md`）。macOS 每个 `IMKInputController` 一个 session，Linux IBus 每个 engine 一个 session，均不偏离。
- 平台层不得绕过 `keytao-core` 直接调用 librime。
- 平台传给 core 的按键必须是 X11 keysym + Rime modifier mask。
- `ImeState` 是提交、预编辑和候选显示的唯一来源。
- `committed` 必须先于新 `preedit` 应用，且提交后必须在同一次协议提交里写回新 preedit 与 cursor。
- 任何模式下按键都先进 Rime；没有 composition 时不能吞应用快捷键和导航键。
- 结束组字只有 `commit_composition()` 与 `clear_composition()` 两条路径，不伪造 Enter/Escape。
- 密码等敏感上下文必须直通，且不得让内容进入候选、用户词、剪贴板历史或日志。
- FFI/JNI 返回 null/false 表示“本次操作没有发生”，不是崩溃信号。
- UI/theme 不读写 Rime 状态。
- App 负责方案安装和部署；系统输入法负责按键热路径。
- reload 通过用户目录的稳定信号触发，不通过 UI 组件触发；检测与执行都不在按键同步路径上。

## 当前已知差异与收敛点

本轮跨平台规范符合度整改后的现状（逐区处置明细见 `docs/ime-convention-compliance-report.md`）。

已收敛：

- `keytao-core::key_policy` 现在还收敛了 text→keysym（`keysym_for_char` / `keysym_for_text` / `char_for_keysym`，非 Latin-1 用 `0x01000000|cp`）、CapsLock 归一（`normalize_key_for_modifiers`）、Enter 决策（`enter_action`）、系统保留修饰键判定（`is_system_reserved_modifier`）以及空 composition bypass。Swift/Kotlin 侧经 FFI/JNI（`keytao_text_to_keysym` / `keytao_key_policy_should_bypass` / `keytao_key_policy_is_enter` 及 `native*` 对应项）调用，五端不再各写一份。
- reload stamp 的路径、写入、签名与变更检测唯一实现在 `keytao-core::ReloadStamp` / `ReloadStampWatcher`；五平台与 App 的私有签名逻辑（此前 App 纳秒 / Android 毫秒 / iOS 纳秒三套）已全部删除改调该 API。macOS 已改用后台 `DispatchSource` + `keytao_reload_if_stamp_changed()`，不再在 Swift 层每次 handle 前比较 stamp；Windows 按键路径 stat 加 250ms 节流并由 focus/context 回调失效缓存；Android 在 `onStartInputView` 触发但执行在后台线程；iOS / Linux daemon 同步接入。
- macOS `commitComposition` 与普通 Return 路径都使用 `XK_Return`/`0xff0d`；五端 Enter 只有 `ImeRuntimeSession::process_enter()` 一个实现。候选/翻页/清空/提交走 librime 官方 API；数字/选择键不再本地拦截；失焦收尾只有 `commit_composition()` / `clear_composition()`，不再伪造 Enter/Escape。
- macOS 消费 FFI 的 `CandidatePanelModel` / `ModeHintModel`（cross-9），删除 Swift 侧重写的 label/高亮/分页逻辑；theme 层的移动端软键盘布局已从共享 `ResolvedImeTheme` 分家到 `keytao-theme::mobile_layout`（`keyboard.yaml`）。
- Linux daemon 日志已从 `/tmp/keytao-ime.log` 固定路径移到 `$XDG_STATE_HOME/keytao/log`（目录 0700，启动时清理旧 `/tmp` 日志）；全平台 release 默认级别不打按键、preedit 全文与 keysym 明细。
- Linux 旧的 `src-tauri/src/ime/linux.rs` 内嵌 Wayland IME 代码已清理；系统输入法维护以 `crates/keytao-linux-ime` daemon 为准。

仍存在的平台差异与遗留：

- **iOS 的 vendored librime 是 1.8.5**（源码 commit `08dd95f5`），落后于桌面/Android 的 1.17.x，缺 `change_page` 与 `highlight_candidate_on_current_page`。`keytao-core` 已用 `#[cfg(target_os = "ios")]` 门控降级（highlight no-op、change_page 退回合成 `-`/`=`），iOS target 现在可编译；但这两处「官方 API」在 iOS runtime 升级前无法真正生效。升级 `vendor/librime/ios` 到 1.17.x 后移除门控。
- **IBus 协议签名分叉**是真实存在的坑：`org.freedesktop.IBus.Engine.UpdatePreeditText` 是 `(vubu)`（第 4 参 `IBusPreeditFocusMode`），`org.freedesktop.IBus.InputContext.UpdatePreeditText` 是 `(vub)`，两者不可混用。IBus 引擎注册需安装期 `/usr/share/ibus/component/<name>.xml` 加运行期 `RegisterComponent`，且必须连 ibus-daemon 的私有总线（`IBUS_ADDRESS` / `~/.config/ibus/bus/*`）而非 session bus。
- Linux GNOME/IBus/Kimpanel 视觉不能完整受 `theme.yaml` 控制；文档和 UI 设置页需要明确“结构生效，视觉受系统限制”。
- **Linux 自绘候选面板（wlroots / X11 overlay）的鼠标点选候选与翻页按钮仍未实现**，属遗留（Windows 本轮已补齐点选）。
- Windows TSF 已接入 reload stamp、solo Shift release、候选点选、Enter direct commit、密码框直通与 CapsLock 归一；`context→session` 映射按已登记偏离暂缓，仍需真实 Windows 桌面回归测试。
- `src-tauri/src/rime.rs` 与 App 进程内仍有不受 reload 保护的裸 `Engine`（App overlay 输入通道，非 JNI/FFI），建议后续迁到 `ImeRuntime`。

## 文档维护规则

平台实现变化时，同步维护三处文档：

1. 本文：只写跨平台契约、通用状态、统一规范和平台接入清单。
2. 平台 `IMPL.md`：写该平台具体协议、进程、目录、构建、限制和排查。
3. `README.md`：只放用户可理解的入口链接和平台状态。

如果平台实现为了系统限制偏离本文，必须在平台 `IMPL.md` 标出“偏离原因、影响范围、后续收敛方式”。
