# Windows IME engine server

Status: Batch 3 已实现，Windows 默认 Pipe；实机验收待完成。Scope: `crates/keytao-windows-ime` (TSF DLL)、server 和 Windows App deploy 路径。

Batch 3 fixes: no-learning fallback reuses a stamped per-user temp directory; explicit `EngineMode=0` keeps startup in-process and allows in-process deploy only when the server pipe is absent; App startup seeds DWORD `EngineMode=1` only when missing.

## 1. 目标 / 非目标

### 目标

| 问题 | 现状 | 目标 |
|---|---|---|
| userdb 多写者 | App（deploy）和每个桌面 TSF 宿主都在同一个 `user_dir` 打开 LevelDB userdb（state.rs:425-428）。LOCK 是跨进程独占的（leveldb env_windows.cc:567-581），所以第二个进程 Open 会失败，接着 `userdb_recovery_task` 会对别人正在用的库做 `RepairDB`，甚至 rename/remove 它（user_db_recovery_task.cc:23-55）。`EngineInit` mutex 只串行化 init（state.rs:419）。 | 每个用户会话只有一个进程打开 userdb。 |
| 宿主崩溃 | librime/Lua 在宿主进程里运行，engine 崩溃会连带宿主（Explorer、Office、浏览器）。 | engine 崩溃只影响 server，宿主保留，按键放行。 |
| 每宿主内存 | 每个宿主各自加载 rime.dll、schema、字典。 | 字典只在 server 加载一份，DLL 只保留 UI。 |
| AppContainer 数据副本 | 沙箱宿主从 `appcontainer-schemas` 复制一份到私有 LocalAppData，然后自己 deploy（appcontainer.rs:78-237, state.rs:435-439）。 | pipe 可用时不再复制，也不在沙箱里 deploy。 |
| 热更新 | engine 升级要等宿主重启后重新加载 DLL。 | server 可以独立重启或升级，客户端自动重连，DLL 不动。 |

保持不变：每键延迟。TSF STA 上的 `OnKeyDown` 仍然同步拿结果。本机 pipe 往返约 20-100µs，engine 本身耗时不到 1ms，所以感知预算 <~100ms 不受影响。

### 非目标

- theme 和 UI 设置不迁移。`THEME_CACHE`/`ThemeResolver`（state.rs:64, panel.rs:750）属于 presentation，留在 DLL。
- 不做 Windows Service，也不用 COM LocalServer32 或 ALPC。
- 不改 Linux 和 macOS 前端。
- 不改 librime，不打 patch。
- 不在 TSF 侧新增 schema 和 option API。DLL 今天就没有 `select_schema`/`set_option`，schema 切换继续走文件请求，由 server 在 reload 时应用。

## 2. 架构

```
宿主进程 (TSF STA)                          keytao-windows-ime-server.exe (medium IL, 每用户每会话一个)
┌───────────────────────────────┐          ┌──────────────────────────────────────────┐
│ keytao-windows-ime.dll        │  named   │ 唯一 ImeRuntime (desktop / AC 共用 userdb)│
│  UI / theme / TSF / key_map   │  pipe    │ librime 状态进程全局                     │
│  ImeEngineBackend:            │◄────────►│ 每连接一个 ImeRuntimeSession              │
│   InProc | Pipe               │ len+json │ deploy / reload / status file / log      │
└───────────────────────────────┘          └──────────────────────────────────────────┘
```

### 2.1 引擎边界（trait）

DLL 对 engine 的全部调用都经过 `TsfState` 持有的 `ImeRuntimeSession`。边界由 8 个 session 调用、1 次 build 和 1 次 reload 检查组成（state.rs:416-461, 846, 878; key_event_sink.rs:1936, 1967-1969, 2244, 2246）：

```rust
pub struct EngineHandle { pub schema_id: String }

pub trait ImeEngineBackend: Send + Sync {
    fn build(&self) -> Result<EngineHandle, String>;            // off-thread only
    fn reload_needed(&self) -> bool;                             // caller caches ≤250ms
    fn drop_session(&self);
    fn process_key(&self, keysym: u32, mods: u32) -> Option<KeyProcessResult>;
    fn process_enter(&self) -> Option<KeyProcessResult>;
    fn select_candidate_on_page(&self, index: usize) -> Option<ImeState>;
    fn change_page(&self, backward: bool) -> Option<ImeState>;
    fn reset(&self) -> Option<ImeState>;
    fn state(&self) -> ImeState;
    fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState>;
    fn set_english_mode(&self, enabled: bool) -> Result<ImeState, String>;
}
```

`None` 的语义保持不变，都表示 "busy/reloading"：DLL 隐藏面板并放行按键（key_event_sink.rs:2250-2256）。IPC 的超时和断开也映射成 `None`，因此调用方不需要新增分支。

Shift 释放今天会连续调两次 engine（press + `RIME_RELEASE_MASK`，key_event_sink.rs:2334-2335）。协议用一个 `ProcessKeys(Vec<(keysym,mods)>)` 在一次往返里完成，返回最后一个结果。为此 trait 增加 `process_keys(&[(u32,u32)])`，in-proc 实现内部就是循环调用。

### 2.2 谁启动 server

- 主路径：HKCU `Run` 值 `KeyTaoImeServer`，登录时启动。安装器（per-machine，admin）不方便给每个用户写 HKCU，所以由 App 首次启动时写入。卸载脚本已经会删 HKCU Run 的 `KeyTao`（keytao.nsi:175），再加上新值即可。
- 兜底：App 启动时检测 pipe，不存在就拉起 server。
- 提权 App 使用交互式 shell 的主 token 启动 server：`GetShellWindow` → 同用户、同会话进程 → `OpenProcessToken` / `DuplicateTokenEx(TokenPrimary)` → `CreateProcessWithTokenW`，不继承 App 的管理员 token。shell 本身已提权（内置 Administrator 或关闭 UAC）时保留其 token；无法取得 shell token 时不启动或终止 server，并提示正常启动 KeyTao。创建进程失败不回退到 App token。使用显式 exe 路径和隐藏控制台标志，进程及 token 句柄均及时释放。参见 [CreateProcessWithTokenW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createprocesswithtokenw)。
- 沙箱宿主不能拉起 medium 级 server，因为子进程会继承 AC token。所以 DLL 从不 spawn server。连不上时 DLL 按 2.7 的 fallback 策略处理，并在后台每 5s 重连一次（复用 engine_status.rs:12 的 backoff，上限 60s）。
- 单实例：pipe 用 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建；新 server 遇到旧实例时认证后发送 `Shutdown`，在 3s 内重试接管。

### 2.3 Pipe 名与 ACL

- 名字：`\\.\pipe\KeyTao.Ime.<UserSID>.<SessionId>.v1`。`\\.\pipe\` 是机器全局的，所以名字必须带 SID 和 session。AppContainer 客户端也用这个全局路径，不用 AC 命名空间。大版本号放在名字里。
- DACL/SACL：`D:(A;;GRGW;;;<UserSID>)(A;;GRGW;;;S-1-15-2-1)S:(ML;;NW;;;LW)`。第一版不加 LPAC（`S-1-15-2-2`），见开放问题。
- 不写 Everyone/Authenticated Users。
- UIPI 不拦 pipe，提权（High IL）的宿主可以正常连接。

### 2.4 认证

server 对每个连接做以下检查：
1. 用 `GetNamedPipeClientProcessId` 拿到客户端 PID，再用 `ProcessIdToSessionId` 确认它和 server 在同一个会话，否则断开。
2. 先读完整 Hello，再用 `ImpersonateNamedPipeClient` / `OpenThreadToken(TOKEN_QUERY, OpenAsSelf=TRUE)` 查询连接 token 的 user SID 和 `TokenIsAppContainer`，随后通过 guard 恢复 server 身份；任何身份读取失败均拒绝连接。这样 medium server 无需打开 elevated App 的进程 token。App、DLL 和替换 server 打开 pipe 时均指定 `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`，只授权身份查询。桌面和 AC 连接都走 server 内唯一的 runtime 和桌面 userdb（见 2.10）。
3. 每个连接独占一个 `ImeRuntimeSession`，不同连接之间不共享 preedit 或候选。

有效首帧的认证、会话或版本拒绝均返回同 seq 的 `HelloErr(reason)`，不再静默关闭；损坏或未完成的首帧无法保证回复。恢复身份失败会终止 server，避免以客户端身份继续工作。

Identification token 的 `TokenIsAppContainer=0` 不单独作为特权授权依据；`Deploy` 和 `Shutdown` 都另验客户端进程映像处于同一 KeyTao 安装树，`Deploy` 还要求 App role。

client 对 server 做以下检查：
- 用 `GetNamedPipeServerProcessId` 拿到 server PID，`QueryFullProcessImageNameW` 取 exe 路径，要求在安装目录 `%ProgramFiles%\KeyTao\` 下。
- AC 宿主仅在进程查询被拒绝时，改查 pipe owner SID 是否等于当前用户 SID；其它失败拒绝连接。日志记录采用的校验。
- 签名校验暂不做，见开放问题。

剩余风险：同用户进程本来就能注入按键、读剪贴板，这个威胁模型和今天进程内 engine 一样。上面的校验只防跨会话、跨用户访问和伪造 server。

### 2.5 协议

- framing：`u32 LE length` + payload，单帧上限 1 MiB，超限就断开。
- 序列化：serde_json。`ImeState`、`KeyProcessResult`、`Candidate`、`InputContextPolicy` 都已经 derive 了 `Serialize/Deserialize`（keytao-core/src/lib.rs:130, 162, 313, 388），不需要新依赖。实测 p99 超过 1ms 再换 bincode。协议与位数无关，x86、x64、ARM64X 宿主都能用，不传指针，也不传 repr(C) 结构。
- 每个请求带 `seq: u64`，响应原样带回。客户端丢弃 seq 不匹配的响应（比如超时后才到的旧响应），并断开重连，避免乱序。每个连接在 server 端串行处理。
- 握手：`Hello{protocol: u32, client_build, is_appcontainer}` → `HelloOk{protocol, server_build}`。server 接受 `protocol ∈ {N, N-1}`，其它版本回 `HelloErr`，客户端就回到 fallback。
- reload 后旧 session 回 `Err(NotReady)`，沿用 busy/not-ready 到 `None` 的映射；客户端标记连接失效，由现有 reload 流程重新 `CreateSession`。

请求与 trait 一一对应：

| Request | Response | trait 方法 | 调用线程 / 频率 |
|---|---|---|---|
| `Hello{..}` | `HelloOk` / `HelloErr` | （连接时） | build 线程 |
| `CreateSession` | `Handle{schema_id}` / `Err(String)` | `build` | warmup/reload 线程 |
| `DropSession` | `Ok` | `drop_session` | reload / deactivate |
| `ProcessKeys{keys}` | `Key(Option<KeyProcessResult>)` | `process_key` / `process_keys` | TSF STA，每键 |
| `ProcessEnter` | `Key(Option<KeyProcessResult>)` | `process_enter` | TSF STA |
| `SelectCandidate{index}` | `State(Option<ImeState>)` | `select_candidate_on_page` | 候选窗 wnd_proc |
| `ChangePage{backward}` | `State(Option<ImeState>)` | `change_page` | 候选窗 wnd_proc |
| `Reset` | `State(Option<ImeState>)` | `reset` | 下一键（lazy） |
| `GetState` | `State(ImeState)` | `state` | install fallback |
| `SetInputPolicy{policy}` | `State(Option<ImeState>)` | `set_input_policy` | 敏感标志变化时 |
| `SetEnglishMode{enabled}` | `English(Result<ImeState,String>)` | `set_english_mode` | 语言栏 / compartment |
| `Generation` | `Generation(u64)` | `reload_needed` | ≤ 每 250ms |
| `Deploy{..}` (App only) | `Deployed(Result<(),String>)` | — | App |

`OnTestKeyDown/Up` 不走 IPC。今天它们就只读缓存的 `st.ime_state` 和 `ascii_mode`，从不调 engine（key_event_sink.rs:2096-2128）。所以每键只有 1 次往返。约束文档里写的"每键 2 次 RTT"（引用 key_event_sink.rs:2061）与这段代码不符，以代码为准，Batch 2 在 Windows 上复核。

### 2.6 超时

- 每键请求在 TSF 线程上用 overlapped I/O 同步等待，超时 **80ms**，做成常量。
- build 超时 30s；App Deploy 超时 120s，都只在后台线程等待。
- 每键请求超时后返回 `None`，按键放行、隐藏面板，同时标记连接失效。下一次 build 触发（焦点或按键，入口同 text_service.rs:243/485 和 key_event_sink.rs:1999）时在后台线程重连并 `CreateSession`。已经存在的"engine 未就绪"路径会接住这段时间。

### 2.7 进程内回退策略

问题在于：server 不可用时如果退回今天的进程内 engine，userdb 多写者问题又回来了。

- Batch 3 默认 `Pipe`。只有 HKCU `Software\KeyTao\Ime` DWORD `EngineMode=0` 使用 InProc，App 部署还需确认无 server pipe；缺失、1、不可读或无效值都使用 Pipe。
- 连接或握手失败时，user_dir 使用每用户固定目录 `%TEMP%\keytao-no-learning`（AC 使用自己的私有 Temp），各宿主共用、按 stamp 复用，从已部署数据复制方案、排除 userdb，以桌面部署数据或 AC 公共快照作为 shared data。临时源和编译配置禁用默认及命名 translator 的用户词典；`learning:false` 在后续策略变更中保持，不打开或修复 desktop userdb。
- 回退写 `keytao-ime-engine.json`（`ok=false, error="server unavailable"`）。后台按 5/10/20/40/60s 重试，连通后标记 reload，下一次 build 切回 Pipe。

### 2.8 server 崩溃 / 重启

- 客户端在读写时发现断开，就把当前调用当作 `None`（放行），然后清掉 `session`、`ime_state`，`input_generation += 1`（沿用 `begin_reload_if_changed` 的清理，state.rs:520-546），再走一次 build。代价是丢掉当前 preedit，DLL 不需要重新加载。
- server 进程内不做自动重启。重新拉起靠 App 的看门狗：App 在运行时每 5s 检测一次 pipe，不在就拉起。App 没运行时就等下次登录（Run 键）。见开放问题 2。

### 2.9 reload / deploy

- 现状：App 在自己的进程里跑 `keytao_core::deploy`（ime_status/windows.rs:1459），宿主 stat stamp 文件来感知（state.rs:52, 557）。这本身就是第二个 userdb 写者。
- Batch 2：deploy 仍由 App 执行；server 监听 InProc 使用的同一个 reload stamp，重新初始化唯一的 runtime 并递增 generation。reload 前创建的 session 随即失效，不能继续处理按键。
- Batch 3：Pipe 模式只由 server 部署，连接失败不转 App 进程内部署；只有显式 `EngineMode=0` 且确认当前用户、当前会话无 server pipe 才保留原路径，pipe 存在、忙碌或探测失败均禁止进程内部署，避免同时写 userdb。App 在写 `Deploy` 之前最多恢复一次：验证 pipe peer 的同会话、安装树、server 文件名并持有进程句柄；旧版本握手成功后请求 `Shutdown`，旧版本握手失败或任何版本在 Hello 回复前静默 EOF 才终止该已验证进程。当前安装版本的超时、`HelloErr`、部分回复均不终止。确认旧进程退出及旧 pipe 释放后，启动/连接当前安装 server 并重试握手；提权 App 必须先取得 shell 启动 token 才允许替换。Shutdown 超时不强杀；一旦开始写 `Deploy`，写入失败或响应失败均不重试。watchdog 与 App 部署/恢复持同一生命周期锁。失败保留原始服务原因，仅 shell token 获取失败或以该 token 创建进程被拒绝时追加管理员提示；无 pipe 且无法安全启动时也报错，不转进程内部署。
- 客户端的 `reload_needed()` 改为向 server 请求 `Generation`，在 250ms 缓存内比较，宿主不再 stat 文件。generation 变了就走现有的 `begin_reload_if_changed` 流程，重建 session。
- `ProfileDeploymentGuard::suspend()`（windows.rs:314）在 Batch 3 里保留，等 Windows 实机确认不需要后再删。
- schema 切换请求文件和 `keytao-ime-engine.json` 由 server 读写，格式不变，所以 App 侧的读取代码不用改。

### 2.10 AppContainer

- pipe 的 DACL 授权了 `S-1-15-2-1`，AC 宿主可以直接连接。AC 与桌面共用 server 内唯一的 `ImeRuntime`，使用桌面的 user_dir 和 userdb，学词也共用；每个连接仍有独立 session，preedit 和候选互不共享。
- 原因：librime / keytao-core 的运行时状态是进程全局的，同一进程创建第二个、使用不同 user_dir 的 `ImeRuntime` 会销毁第一个 runtime 的 session。因此 server 不创建第二个 runtime，也不使用 `%LOCALAPPDATA%\KeyTao\appcontainer-userdb`；`TokenIsAppContainer` 仅用于日志。
- pipe 可用后，DLL 里的 `appcontainer::prepare_data/prepare_at/mark_deployed`、私有 deploy 和 AC 命名空间的 mutex 都不再执行。代码保留在 `InProc` 实现里，作为 fallback，Batch 3 之后再评估删除。
- `%LOCALAPPDATA%\KeyTao\appcontainer-schemas` 的发布（windows_public_schemas.rs）在 fallback 还存在时继续保留。

### 2.11 日志

- server 调 `runtime_log::init(user_dir, "windows-ime-server")`，engine 侧的 `rt_log!` 和 diagnostic 都写到这里。
- DLL 保留自己的 `runtime_log`（"windows-ime"），记录 TSF 和 UI 事件、IPC 超时和断开次数，在 deactivate 时随 `drain_key_stats()` 一起输出（text_service.rs:392）。
- 日志不走 IPC。
- 握手拒绝查 `ipc/hello_rejected` 的 `msg`、`client_pid`；监听失败查 `accept_failed`、`create_next_pipe_failed`、`client_thread_failed`，失败后保留监听继续接收。runtime logger 目前只有 Info/Verbose，因此警告通过 `severity="warn"` 标记；握手拒绝同时输出 stderr WARN。App 恢复查 `recovering IME server before Deploy`，包含 server PID、image 和握手状态。

## 3. 分批

本地 macOS 的能力边界：`keytao-windows-ime` 的依赖全部在 `cfg(target_os="windows")` 下，所以 macOS 只能做 `cargo zigbuild`/`cargo check --target x86_64-pc-windows-msvc`（需要 xwin SDK），跑不了 TSF 测试。现有 CI 的 `ime-regressions.yml` 没有 Windows job（只有 ubuntu 和 macos），Windows 只在 `release.yml` 里构建。所以每个 Batch 的 Windows 验收都需要 Windows 实机，或者先新增 Windows CI job（见开放问题 4）。

### Batch 1：纯重构（trait + InProc，无行为变化）

目标：`TsfState` 不再直接持有 `ImeRuntimeSession`，所有 engine 调用都走 `ImeEngineBackend`，唯一实现是 `InProcEngine`（逻辑从 state.rs 原样搬过去）。

文件白名单：
- 新增 `crates/keytao-windows-ime/src/engine_backend.rs`，放 trait、`EngineHandle`，以及 `InProcEngine`。`InProcEngine` 持有 `ImeRuntime`、`ImeRuntimeSession`、`rime_dll`、`reload_stamp_*`，内部包含 `try_build_engine` 的 7 步（AppContainer seed、init mutex、preload dll、init/deploy、create_session、stamp）以及 `reload_needed`。
- `crates/keytao-windows-ime/src/lib.rs`：增加 `mod engine_backend;`。
- `crates/keytao-windows-ime/src/state.rs`：`EngineBundle`、`try_build_engine`、`install_engine`、`reload_needed`、`begin_reload_if_changed`、`session()`、reset 和 input policy 的调用点（约 116-117, 235, 416-546, 557-618, 846, 878, 1402-1427）。
- `crates/keytao-windows-ime/src/key_event_sink.rs`：`runtime_session()`（1831），以及 1936、1967-1969、2006-2009、2244-2246、2334-2335 处的调用点。Shift 释放改成调用 `process_keys`。
- `crates/keytao-windows-ime/src/language_bar.rs`：只在 323 处的 English mode 调用涉及 session 类型时才改。

字段约束（为了让现有测试一行不改）：测试会直接读 `st.runtime.is_none()` 和 `st.session.is_none()`（shortcut_passthrough_tests.rs:239, 363；language_bar_lifecycle_tests.rs:175）。所以保留这两个字段名，类型改成 `runtime: Option<Arc<dyn ImeEngineBackend>>`（引擎）和 `session: Option<EngineHandle>`（就绪标记），`.is_none()` 的语义不变。`unhandled_key_tests.rs` 直接用 `ImeRuntime`，不受影响。

不做：协议、pipe、新 crate、theme/log、appcontainer.rs 逻辑改动、任何测试文件改动。

验收：
- macOS：`cargo check --target x86_64-pc-windows-msvc -p keytao-windows-ime`（zigbuild/xwin）通过；`git diff --name-only` 只有白名单文件；`grep -n "ImeRuntimeSession" crates/keytao-windows-ime/src/` 只命中 engine_backend.rs 和测试。
- Windows 实机或 CI：`cargo test -p keytao-windows-ime -- --test-threads=1`，测试数量（`--list | wc -l`）与基线相同且全部通过；手工打一句、翻页、点候选、Shift 切换中英、触发一次 App deploy，行为与旧版本一致。

### Batch 2：server + client + pipe（运行时开关，默认 InProc）

新增：
- `crates/keytao-ime-ipc/`（无 Windows 依赖，workspace member）：`Request/Response` enum、`Hello` 和版本协商、`u32 LE` framing 的读写（基于 `std::io::Read/Write`）、seq 匹配。依赖只有 `keytao-core`（类型）和 `serde_json`。
- `crates/keytao-windows-ime-server/`（bin，`cfg(windows)`）：创建 pipe（`FIRST_PIPE_INSTANCE`，按 2.3 的 SDDL，用 `ConvertStringSecurityDescriptorToSecurityDescriptorW`），每个连接一个线程和 session，按 2.4 认证；仅一个桌面 runtime，AC 共用其 userdb；监听 reload stamp、递增 generation，初始化 `runtime_log` 并写 `keytao-ime-engine.json`。
- `crates/keytao-windows-ime/src/engine_pipe.rs`：`PipeEngine: ImeEngineBackend`，包括 overlapped 同步调用、80ms 超时、断开后转成 `None`、server exe 路径校验。

修改：
- `engine_backend.rs`：按开关选择实现。开关是 HKCU `Software\KeyTao\Ime` 下的 DWORD `EngineMode`（0=InProc 默认，1=Pipe），在 build 时读取。注册表读不到或值未知时一律用 InProc（fail closed）。
- `Cargo.toml`（workspace members）、`crates/keytao-windows-ime/Cargo.toml`（加 `keytao-ime-ipc` 依赖，以及 `Win32_System_Pipes`、`Win32_System_IO`、`Win32_Security_Authorization` features）。
- `packaging/windows/keytao.nsi` 和 `nsis-hooks.nsh`：把 server exe 装进安装目录（这一批还不写 Run 键）。

不做：默认切换、deploy 迁移、Run 键、删除 appcontainer 代码。

验收：
- macOS 和 Linux CI：`cargo test -p keytao-ime-ipc` 覆盖 framing 往返、超限帧被拒、半帧和 EOF、seq 不匹配、版本协商（N、N-1、N-2 被拒）。把 `keytao-ime-ipc` 加进 `ime-regressions.yml` 的 test 列表。`cargo check --target x86_64-pc-windows-msvc` 覆盖 server 和 dll。
- Windows 实机：
  - `EngineMode=0` 且无 server pipe 时，行为与 Batch 1 一致；pipe 存在或探测失败时 App 拒绝进程内部署。
  - `EngineMode=1` 加手动启动 server 后，在记事本和 Edge 打字、翻页、点选候选。用 x86 宿主（32 位记事本或 SysWOW64 应用）和 Windows Search/开始菜单（AppContainer）各验证一次。
  - 测每键 p99 往返延迟（server 日志统计），要求 <5ms。
  - kill server 后宿主不崩溃、按键放行，重新拉起后能继续打字。
  - 用另一个用户会话或 PsExec 跨会话连接 pipe，应被拒绝。

### Batch 3：默认 server + deploy 迁入 server + 热更新

- 已实现：Windows App 启动写 HKCU Run `KeyTaoImeServer`，定位 HKLM `WindowsImeRuntimeDir` 下的当前架构 server，隐藏控制台拉起，并每 5s 检查 pipe。运行文件放在 Program Files 下的版本化目录；卸载删除 Run 值并停止 server。
- 两处 App deploy 均优先 RPC，保留 profile suspend；server 使用唯一桌面 runtime 和与 App 相同的已安装 shared data。部署完成后 generation 递增，旧 session 失效。
- 默认 Pipe，显式 0 保留原 InProc，App 仅在确认无 server pipe 时进程内部署；失败按 2.7 使用临时无学习回退和后台重连。
- 协议升为 3，接受 3/2。新增 `Shutdown`，仅接受同用户且 exe 位于同一 `%ProgramFiles%\KeyTao\` 安装树的调用者；新实例握手、请求旧实例退出，再在 3s 内重试创建 pipe。已加载的旧 DLL 可继续使用协议 2。
- Batch 2 server 尚不认识 `Shutdown`；App 部署遇到旧协议握手拒绝时，可以在验证进程后终止旧 server 并恢复一次。正常握手的后续 server 通过 `Shutdown` 热替换。

验收：
- macOS：`cargo test -p keytao-ime-ipc`（Deploy、Shutdown 消息）通过；`cargo check` 覆盖三个 Windows target。
- Windows 实机：
  - 装好后登录，server 自动运行；同时开 3 个宿主连续打字学词，userdb 目录里只有 server 一个 LOCK 持有者（用 `handle.exe LOCK` 查看），日志里没有 `userdb_recovery`/`RepairDB`。
  - App 里 deploy 后，各宿主下一次按键或焦点切换时用上了新 schema。
  - server 不在时能打字，但不产生学词（userdb 文件 mtime 不变）。
  - 升级安装时，旧 DLL 仍映射在宿主里，与新 server 通过 N-1 协议正常工作。

## 4. 开放问题

1. **server 不可用时怎么办**：用无学习的临时 userdb 继续打字（本文默认），还是干脆不提供输入？前者体验更好，但会多保留一套进程内 engine 代码。
2. **App 没运行、server 又崩溃时谁负责拉起**：只靠下次登录的 Run 键加 App 看门狗（本文默认），还是再加一个登录触发的计划任务，让它定时重拉？
3. **AppContainer 学词归属**：已决定并入桌面 userdb；librime 状态进程全局，server 无法同进程跑第二个 runtime，见 2.10。
4. **Windows 验收在哪跑**：先给 `ime-regressions.yml` 加一个 `windows-latest` job，跑 `cargo test -p keytao-windows-ime`，还是每批都在你的 Windows 实机上手工验收？
5. **server 身份校验到什么程度**：只校验 exe 路径在 `%ProgramFiles%\KeyTao\` 下（本文默认，因为 Program Files 只有 admin 能写），还是还要做 Authenticode 签名校验？是否需要支持 LPAC 宿主（`S-1-15-2-2`）？

### 决定（2026-11-15，用户确认按建议）

1. server 不可用：用不学词的临时 userdb 继续打字（Batch 3 已实现）。
2. 拉起：登录 Run 键 + App 运行时看门狗；暂不加计划任务。
3. AppContainer 与桌面共用 server 内唯一的 runtime 和 userdb（2026-11-15 改：librime 状态进程全局，同进程无法跑第二个 runtime）。
4. 新增 Windows CI job，跑 `cargo test -p keytao-windows-ime -- --test-threads=1`。
5. 只校验 server exe 路径在 `%ProgramFiles%\KeyTao\` 下；签名校验等安装包签名后再加；暂不支持 LPAC。
