# Windows IME 实现说明

本文只记录 `crates/keytao-windows-ime` 里的 Windows TSF 输入法实现。

跨平台通用契约见 [输入法通用层实现规范](../../docs/ime-common-layer.md)；本文只补充 Windows TSF TIP 的注册、composition 和候选窗差异。

## 形态

Windows 输入法实现为 TSF TIP DLL：

- DLL：`keytao_windows_ime.dll`
- TextService CLSID：`{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}`
- Language profile GUID：`{1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}`
- 语言：Simplified Chinese，`0x0804`

## 注册

文件：`src/registration.rs`

`DllRegisterServer` 做这些事：

1. 写入 `HKCR\CLSID\{...}\InprocServer32`。
2. 设置 `ThreadingModel=Apartment`。
3. 调用 `ITfCategoryMgr::RegisterCategory` 注册 keyboard TIP、UIElement、system tray 和 display attribute provider；每项都使用 `CLSID -> category -> CLSID` 的标准形态。
4. 优先调用现代 `ITfInputProcessorProfileMgr::RegisterProfile` 一次性注册 text service 与 `KeyTao` language profile，图标文件参数指向 `keytao_windows_ime.dll`，图标索引使用微软 API 规定的 zero-based `0`，对应 DLL 内第一枚图标资源。
5. 只有现代 profile manager 不可用时，才使用 `ITfInputProcessorProfiles::Register` + `AddLanguageProfile` 的旧兼容路径，避免在正常 Windows 10/11 上混用两代注册接口。
6. 调用 `InstallLayoutOrTip`，用 `0x0804:{CLSID}{ProfileGUID}` 把 KeyTao 加入当前用户启用的输入法列表。
7. 在 `InstallLayoutOrTip` 返回后调用 `EnableLanguageProfile(TRUE)`；该顺序不可反转，因为输入列表更新可能重置 profile 的 enabled flag。
8. 同时通过 `IsEnabledLanguageProfile` 和 `ITfInputProcessorProfileMgr::GetProfile` 检查最终启用状态；profile 未启用时注册失败，而不是只凭 CLSID 写入报告成功。

只写入 CLSID / InprocServer32 代表 COM server 已注册，但不代表用户语言 profile 已启用，也不代表当前用户的输入法列表已经包含该 TIP；Windows 输入切换器只会切到已启用且加入用户输入法列表的 TSF language profile。
重复注册采用幂等的 `RegisterProfile` / `AddLanguageProfile` 更新，不会先 `UnregisterProfile` 或 `RemoveLanguageProfile` 再重建。这样安装升级和 x86/x64 双架构注册不会反复移除当前 profile、触发不必要的 TSF 全局刷新或丢失用户启用状态。`input.dll` 只通过 `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)` 从系统目录加载，避免注册阶段受 DLL 搜索路径影响。

`DllUnregisterServer` 会先用 `InstallLayoutOrTip` 的 uninstall flag 从当前用户输入法列表移除 KeyTao，再反向移除 TSF profile、category 和 CLSID 注册表树。

x64 安装包同时携带 `current` x64、`x86` 和 `arm64x` runtime。ARM64X runtime 由 ARM64X forwarder、原生 ARM64 TIP + `rime-arm64.dll`、x64 TIP + `rime.dll` 组成；Windows on ARM 的原生和模拟进程会经同一个 COM 路径加载匹配目标。NSIS 先把完整 runtime 复制到 `%ProgramData%\KeyTao\keytao-windows-ime-runtime\<version>\<unique>`，再先注册 x86、最后注册 native/ARM64X。每次安装使用新目录，避免 `ctfmon`、浏览器或 WebView 已加载旧 TIP 时覆盖 DLL 失败。活动目录写入 `HKLM\Software\KeyTao`，卸载时反注册后用 `/REBOOTOK` 清理已加载旧文件。

Tauri 主 App 在启动后会先完成 `windows-ime-status` 事件监听并渲染界面，再通过后台任务检查 TSF 状态。若 COM DLL 已完整注册但当前 profile 未启用，App 会在未提权的当前用户进程先调用 `InstallLayoutOrTip`、再调用 `EnableLanguageProfile` 并验证最终状态；这也覆盖标准用户用另一管理员账户确认 UAC、导致安装器无法修改原用户输入列表的情况。只有 COM/runtime 不完整时才异步触发提升权限 PowerShell 流程。该流程把 native/ARM64X 与 x86 runtime 一起复制到新的 `%ProgramData%` 版本目录，再按位数注册；状态通过 `windows-ime-status` 事件回传到界面。界面只保留刷新入口，不提供手动重装 TSF 或卸载按钮，避免注册流程阻塞首屏显示。
主 App 状态检查通过 Unicode Registry API 和显式 `KEY_WOW64_32KEY` / `KEY_WOW64_64KEY` 分开读取 x64/x86 COM view，再核对 DLL path 和 TSF profile enabled 状态：`registered=true` 必须同时满足两套已打包 DLL 路径匹配注册表，并且 TSF profile 已启用。这样既不会受 `reg.exe` 输出代码页影响，也不会把“某一个 COM DLL 注册表存在”误报成“输入法在所有应用中可切换”。

## TSF 官方契约对齐点

实现按 Microsoft TSF 文档保持这些边界：

- text service 除标准 COM in-proc server 注册外，还要通过现代 `RegisterProfile`（旧系统回退为 `ITfInputProcessorProfiles::Register` + `AddLanguageProfile`）和 `ITfCategoryMgr::RegisterCategory` 注册到 TSF。
- IME branding icon 必须嵌入 DLL/EXE。KeyTao 的 PE 资源 ID 1 使用与 macOS 输入源一致的白色五角星品牌图形，深色底保证其在 Windows 浅色任务栏仍可辨识，并包含 `16/20/24/32/40/48` 六档 32-bit alpha frame；`RegisterProfile` 使用 DLL 路径与负资源 ID，发布验证会以 image resource 模式加载 DLL 并验证该资源，不再依赖外置 `.ico`。资源 ID 2/3 继续独立表示“中/EN”模式，品牌与输入模式不混用。
- 安装后要调用 `InstallLayoutOrTip`，把 `<LangID>:{CLSID}{ProfileGUID}` 加进当前用户输入法列表；只启用 language profile 仍可能导致输入切换器里可见但不能真正切换。
- 现代 Windows 可能查询 `ITfTextInputProcessorEx`；KeyTao 同时实现 `ITfTextInputProcessor` 和 `ITfTextInputProcessorEx`。两条激活路径共用轻量接线逻辑，并通过 `ITfThreadMgrEx::GetActiveFlags` 保存 thread mode。
- `ITfKeyEventSink` 由 text service 实现，并通过 `ITfKeystrokeMgr::AdviseKeyEventSink` 安装到当前 `ITfThreadMgr`。
- `ITfLangBarItemButton` 通过 `ITfLangBarItemMgr::AddItem` 暴露持久的“中/英”状态、图标和左键切换，并同步 `GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION`。
- `ITfThreadMgrEventSink` 和 `ITfThreadFocusSink` 在 `Activate` 时通过 `ITfSource::AdviseSink` 安装，并在 `Deactivate` 时按 cookie 反注册；document/context/thread 焦点变化会异步申请 TSF write edit session，清空未提交 range/display attribute 并真正结束旧 composition，然后复位 Rime session 和候选 UI。终止回调按 COM identity 只清理自己对应的 active composition，迟到的旧回调不能清掉新 Context 已开始的 composition。
- `OnTestKeyDown` / `OnTestKeyUp` 只声明当前按键是否会被处理；真正的状态更新、commit 和 composition 操作在 `OnKeyDown` / `OnKeyUp` 中完成。
- composition 必须在 TSF edit session 里用 `ITfContextComposition::StartComposition` 创建，用 `ITfRange::SetText` 更新 range，用 `ITfComposition::EndComposition` 结束；没有 active composition 的直接提交使用 `ITfInsertAtSelection::InsertTextAtSelection`。
- composition range 会写入 `GUID_PROP_ATTRIBUTE`，对应的 `ITfDisplayAttributeProvider` 提供 input display attribute；更新 preedit 后同步 TSF selection 到 Rime cursor。任何 edit session 失败都会终止残留 composition 并复位输入状态。
- 候选 UI 显示前先调用 `ITfUIElementMgr::BeginUIElement`。宿主允许 TIP 自绘时才显示 layered window；宿主返回 `pbShow=FALSE` 时通过 `ITfCandidateListUIElement` 提供目标 `ITfDocumentMgr`、更新 flags、候选数量、选中项、字符串与分页数据，并调用 `UpdateUIElement` / `EndUIElement`。
- 候选窗必须使用当前 `ITfContextView::GetWnd()` 返回的窗口作为 owned window；如果 TSF context 没有窗口，则退回 `GetFocus()`。候选窗显示、隐藏、位置/大小变化分别通过 `NotifyWinEvent(EVENT_OBJECT_IME_SHOW/HIDE/CHANGE, ...)` 通知 Windows light-dismiss/accessibility 管线。
- 自绘候选窗使用宿主窗口 DPI 缩放，并按 caret 所在显示器的 work area 定位；候选窗销毁后尝试注销 Win32 window class，避免 DLL 重载后保留指向旧 `wnd_proc` 的类注册。
- 触摸键盘或 `SendInput` 产生的 Unicode 输入会以 `VK_PACKET` 进入 `ITfKeyEventSink`；KeyTao 使用 `GetKeyboardState` + `ToUnicode(VK_PACKET, ...)` 提取字符，并把触摸键盘候选翻页的 `0xF003` / `0xF004` 映射到 Rime 的 PageDown / PageUp keysym。

参考：

- [Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)
- [ITfInputProcessorProfileMgr::RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)
- [ITfInputProcessorProfileMgr::GetProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-getprofile)
- [InstallLayoutOrTip](https://learn.microsoft.com/en-us/windows/win32/tsf/installlayoutortip)
- [IME requirements](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)
- [64-Bit Considerations](https://learn.microsoft.com/en-us/windows/win32/tsf/64-bit-platform-considerations)
- [ITfTextInputProcessor::Activate](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itftextinputprocessor-activate)
- [ITfKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfkeyeventsink)
- [ITfKeystrokeMgr::AdviseKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink)
- [ITfSource::AdviseSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfsource-advisesink)
- [ITfThreadMgrEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfthreadmgreventsink)
- [ITfContextComposition::StartComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextcomposition-startcomposition)
- [ITfComposition::EndComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-endcomposition)
- [ITfInsertAtSelection::InsertTextAtSelection](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinsertatselection-inserttextatselection)
- [UILess Mode Overview](https://learn.microsoft.com/en-us/windows/win32/tsf/uiless-mode-overview)
- [Providing Display Attributes](https://learn.microsoft.com/en-us/windows/win32/tsf/providing-display-attributes)

### 能力声明矩阵

| TSF category | 当前声明 | 实现依据 |
| --- | --- | --- |
| `GUID_TFCAT_TIP_KEYBOARD` | 是 | TSF keyboard TIP + foreground key event sink |
| `GUID_TFCAT_TIPCAP_UIELEMENTENABLED` | 是 | `ITfCandidateListUIElement` + Begin/Update/EndUIElement |
| `GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT` | 是 | DLL 内嵌 branding icon + profile resource index |
| `GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER` | 是 | `ITfDisplayAttributeProvider` + `GUID_PROP_ATTRIBUTE` |
| `GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT` | 否 | 尚未完成 AppContainer 可写 user-data/broker 边界，不虚报 |
| `GUID_TFCAT_TIPCAP_SECUREMODE` | 否 | librime 用户数据和诊断路径不适合 Secure Desktop |
| `GUID_TFCAT_TIPCAP_COMLESS` | 否 | 未实现无 COM 激活路径 |
| `GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT` | 是 | `ITfLangBarItemButton` + `GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION` |

完整 Windows search contract 还需要 `ITfFnSearchCandidateProvider` 和 `ITfIntegratableCandidateListUIElement`；当前只声明并实现基础 UIless candidate contract，不把它描述为完整 search integration。

## 初始化

文件：

- `src/lib.rs`
- `src/text_service.rs`
- `src/state.rs`

流程：

1. TSF 调用 `DllGetClassObject`。
2. `ClassFactory::CreateInstance` 创建 `TextService`。
3. `TextService::Activate` 只保存 `ITfThreadMgr` / `client_id` / active flags，并注册 foreground `ITfKeyEventSink`、`ITfThreadMgrEventSink` 和 `ITfThreadFocusSink`；不在激活时初始化 librime，避免输入法全局切换时多个宿主进程同时争抢 CPU、磁盘和用户数据锁。
4. 文档 focus/context push 完成后检查 engine mailbox 和 reload stamp，并按需启动后台 warmup/reload；如果 profile 激活前宿主已经有焦点文档，则在 `Activate` 完成全部 TSF 接线后只调度后台任务，不在 `Activate`、key focus 或 thread focus 回调的 UI 线程中初始化引擎。后台任务不持有 `TsfState`，只创建 `keytao_core::ImeRuntime`、调用 `init_without_deploy()`、创建 `ImeRuntimeSession` 并把结果写入 engine mailbox。跨进程命名互斥量会串行化同一 Windows session 内的初始化和 reload，避免多个宿主同时争用 Rime 用户数据。按键测试回调也会在尚未启动时兜底调度，但当前按键继续放行；TIP 绝不在宿主应用进程里运行 Rime deployment。
5. warmup 未完成或 reload 正在运行时，`OnTestKeyDown` 与 `OnKeyDown` 都放行按键，不让测试回调和真实回调产生相反的接管状态。首次 warmup 失败后，测试回调按 5 秒退避重新启动后台 warmup；检测到 reload stamp 变化时，测试回调直接启动后台 reload，避免因测试结果为 false 而永远进不了真实按键回调。
6. 后续按键通过 `KeyEventSink::OnKeyDown` 进入通用 runtime，再由 `keytao-core` 操作 librime。

`Activate` 只做轻量 TSF 接线，前台 focus 到来后才调度预热，避免 Windows 在输入法切换、枚举或批量加载 TIP DLL 时触发多进程 librime 初始化。初始化失败时按键会放行并记录错误，不会让 TSF 激活本身失败。

切换输入法时必须避免在 TSF 回调线程里做重活或持锁调用可重入 API：

- `DllMain` 只保存 DLL instance 并调用 `DisableThreadLibraryCalls`。TIP 不在宿主进程安装全局 tracing subscriber；发行构建默认也不做同步文件诊断，只有 debug 构建或 `KEYTAO_WINDOWS_IME_DIAGNOSTICS=1` 时写 `windows-ime.log`。
- `keytao_windows_ime.dll` 在 MSVC 构建中把 `rime.dll` 配置为 delay-load，并在第一次真正初始化引擎前用 DLL 同级绝对路径预加载 `rime.dll`；这样 Windows 切换输入法载入 TSF DLL 时不会同步加载整条 librime 依赖链，也不会从宿主进程目录错误解析同名 DLL。
- x86/x64 使用 librime 官方 Windows SDK 中合并的 `librime-lua`；原生 ARM64 构建读取官方包记录的同一插件 revision，并通过 `RIME_PLUGINS` 合并进 `rime-arm64.dll`。构建和 bundle 校验必须同时验证 feature manifest 与 `lua_translator` / `lua_filter` / `lua_processor` 标记，不能再把缺少 Lua 当作警告放行。
- 若自定义 runtime 使用外置 Lua plugin，加载器只从版本化 runtime、应用资源目录和显式 `KEYTAO_RIME_PLUGIN_DIR` / `RIME_LIB_DIR` 中解析，不遍历宿主进程 `PATH`；加载后以 Rime module registry 判断是否成功，不依赖 Windows DLL 导出 C++ linker helper symbol。
- 候选窗主题解析通过 `RegGetValueW` 直接读取 `AppsUseLightTheme`，禁止在 `OnKeyDown` / panel render 路径启动 `reg.exe` 或其他子进程；现代 WinUI 宿主可能限制子进程并让 `Command::output()` 永久等待管道，进而冻结整个输入应用。
- `Activate` 首先用 `GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_PIN)` 固定 in-process TIP，所有架构的 TIP 同时静态链接 Rust/MSVC CRT。后台预热或迟到回调因此不会在 TSF 卸载模块或 CRT 后继续执行；模块只在宿主进程结束时由系统统一释放。
- `TsfState` 使用 STA 内的 `Rc<RefCell<_>>`，类型系统不允许它跨线程；`TextService::Activate` / `Deactivate` 不在持有 mutable borrow 时调用 `AdviseKeyEventSink`、`AdviseSink`、`UnadviseKeyEventSink` 或 `UnadviseSink`，避免 TSF 同步回调 `OnSetFocus` 时重入冲突。
- `ITfKeyEventSink`、thread manager/focus sink 和 composition sink 只保存 STA 内 `TsfState` 的 `Weak` 引用；状态仍可持有对应 COM interface 以管理生命周期，但不再形成 `state -> COM sink -> state` 强引用环。宿主异常跳过 `Deactivate` 时，状态与 thread manager 仍可释放，迟到回调会直接放行或忽略。
- `CandidateWindow::new()` 不创建 HWND、不读取字体；候选窗和字体在首次显示候选或模式提示时懒加载，避免 TSF 创建 text service 时卡住输入线程。
- `apply_ime_state()` 不在持有 `TsfState` borrow 时调用 `StartComposition` / `SetText` / `EndComposition`，并在同步 edit session 返回后才刷新候选窗，避免 `OnCompositionTerminated` 或宿主窗口消息重入时借用冲突。
- librime setup 和 reload session rebuild 都通过命名后台线程执行；每个任务和每个 COM 对象都持有 `DllActivityGuard`，所以 `DllCanUnloadNow` 不会在线程或 sink 仍可回调时返回 `S_OK`。后台任务只把纯 engine bundle 写入 mailbox，不持有或升级 `TsfState`；下一次 TSF STA 回调领取并安装结果，禁止在 worker 上析构 STA COM/window 对象。
- `OnTestKeyDown` 只读取缓存 `ImeState`，不调用 `ImeRuntimeSession::state()`，避免 TSF 按键探测阶段进入 librime 或触发 generation refresh。
- focus/context 回调清理 TSF composition 与窗口、标记 session 待重置，并通过 `refresh_engine_for_focus()` 领取后台结果或调度 warmup/reload；真正的 `ImeRuntimeSession::reset()` 延迟到下一次真实按键，回调本身不会在宿主 UI 线程同步进入 librime。
- `ImeRuntimeSession::state()` / `process_key_result()` / `select_candidate()` / `reset()` 先 clone session 句柄再执行，避免在 `TsfState` borrow 内进入 librime 或触发 generation refresh。
- 候选窗 Win32 show/hide 会临时从 `TsfState` 中取出窗口对象后再执行，避免 `UpdateLayeredWindow`、字体加载或窗口销毁和 TSF 状态 borrow 相互耦合。

共享数据目录优先从 DLL 相关目录查找：

- `rime-data`
- `resources/rime-data`
- `share/rime-data`

找不到时退回 `keytao_core::default_shared_data_dir()`。

## 按键处理

文件：

- `src/key_event_sink.rs`
- `src/key_map.rs`
- `src/candidate_win.rs`

流程：

1. `OnTestKeyDown` 先调用 `should_eat_key()`，告诉 TSF 是否拦截当前按键。
2. `OnTestKeyUp` 只对 solo Shift release 返回拦截，避免 Shift+字母/数字误触发中英切换。
3. `OnKeyDown` 把 Windows Virtual Key 转成 librime 使用的 X11 keysym。
4. `current_mod_mask()` 读取 Shift、Control、Alt 状态。
5. 没有 composition 时放行 Space、Return、Backspace、Delete、Tab、Escape、导航键和 Ctrl/Alt 组合键。
6. 有 preedit 时，Enter 直接提交当前 preedit 并 reset session；空格或 `select_keys` 命中的选择键调用 `select_candidate(index)`。
7. 普通按键调用 `ImeRuntimeSession::process_key_result(keysym, mods)`。
8. Rime 结果消费判断不只看 `accepted`：只要状态里产生了 preedit、candidate、commit，或需要清空已有 composition，就会同步 TSF 状态和候选窗；只有完全没有 IME 状态变化且 `accepted=false` 时才返回不消费。
9. 有 `committed` 时：
   - 若存在 TSF composition，替换 composition range、结束 composition，并把 TSF selection 折叠到提交文本末尾。
   - 否则用 `ITfInsertAtSelection` 直接插入。
10. 有 `preedit` 时：
   - 若存在 composition，更新 composition range。
   - 否则从当前 caret 开始 `StartComposition`。
11. preedit 清空且无 commit 时，结束 composition。
12. 候选窗口用 `CandidateWindow` 绘制并按 caret screen position 定位。

`OnKeyUp` 只处理 solo Shift release：发送 `Shift_L` 或 `Shift_R` keysym，mask 为 `RIME_RELEASE_MASK`。Shift key down 本身不送 Rime；如果 Shift 按下后又出现其它 keyDown，pending flag 会被清除。

Windows 的 composition 生命周期比较显式，所以 commit 与新 preedit 同时出现时，必须先结束旧 composition，再把 selection 折叠到 commit range 末尾，最后创建新 composition。缺少 selection collapse 时，新 preedit 会插到已提交文字前面，顶功结果会从“`缤` + `c`”错误地显示成“`c缤`”。

## 候选窗与主题

文件：

- `src/candidate_win.rs`
- `src/panel.rs`

候选窗和中英模式提示都是 Win32 layered popup window，位置跟随 TSF caret screen position。`candidate_win.rs` 只负责窗口生命周期、屏幕边界、mode hint timer 和 `UpdateLayeredWindow` 上传；`panel.rs` 负责把 UI model 渲染成 BGRA buffer。

主题接入方式与 Linux 自绘通道一致：

1. `keytao-theme::ThemeResolver` 从默认主题和用户主题解析 `ResolvedImeTheme`。
2. `panel.rs` 把 `ImeState` 转成 `CandidatePanelModel`，统一 select key label、highlight、翻页和横竖排语义。
3. `panel.rs` 把 `ascii_mode` 转成 `ModeHintModel`，统一 `中`/`英` 文案、背景色、前景色、字号、尺寸、圆角和显示时长。
4. Windows renderer 把 `ResolvedImeTheme + CandidatePanelModel / ModeHintModel` 直接按 owner window DPI 栅格化到 layered window 像素 buffer，不再对低分辨率 panel 做二次 bitmap 拉伸。
5. Windows 桌面候选窗在 DPI 比例上应用 `0.82` 的紧凑密度；150% 系统缩放时渲染比例为 `1.23`，保留每显示器 DPI 语义，同时把默认 34 DIP 候选行收紧到约 42 个物理像素。
6. 字体按 glyph 回退：中文优先使用 Microsoft YaHei / SimSun，缺字时依次使用系统自带的 Segoe UI Emoji、Segoe UI Symbol 和 Segoe UI；`FE0E`、`FE0F` 与 ZWJ 作为零宽格式控制符处理，不能显示成方框或额外占位。

`theme.yaml` v2 支持 `ui.colorScheme: auto | light | dark`、`ui.accentColor` 和 `light:` / `dark:` 模式变体；`auto` 跟随系统主题，Windows 自绘候选窗会消费解析后的最终主题。

用户主题路径跟随 `keytao_core::default_user_data_dir()`，即 `%APPDATA%/keytao/theme.yaml`；开发覆盖可用 `KEYTAO_IME_THEME_PATH`。

## key map

`src/key_map.rs` 维护 VK 到 X11 keysym 的转换：

- 字母按 Shift 状态传小写或大写 ASCII keysym，同时保留 Shift modifier mask，语义与 Linux/macOS 的“keysym 表示实际字符、mask 表示修饰键”一致。
- 数字行和 OEM 标点通过当前 HKL 的 `ToUnicodeEx(..., flag=4)` 解析，正确支持非 US 布局和 Shift 符号；dead key 不会被伪装成 US 标点。小键盘操作符映射到对应 ASCII keysym。
- Backspace、Tab、Return、Escape、Space、Delete、方向键等映射到 XK 值。
- `VK_F4` 映射为 `XK_F4` / `0xffc1`，用于打开 Rime schema / options 菜单。
- `VK_PACKET` 使用 `GetKeyboardState` + `ToUnicode` 解包 Unicode 字符；触摸键盘发送的 `0xF003` / `0xF004` 会映射到 `XK_Page_Down` / `XK_Page_Up`。
- `Shift_L` / `Shift_R` 只在 solo key up 时以 release mask 送入 Rime。
- 其它 function key、媒体键等返回 `None`，输入法不拦截。

## 重载

Windows 用户目录为 `%APPDATA%/keytao`。TSF 前端在 key、document/context focus 和 thread focus 回调中检查：

```text
%APPDATA%/keytao/keytao-ime.reload
```

stamp 的 mtime/size 签名变化后，`start_reload_if_needed()` 标记 reload running，先释放旧 runtime/session 及其映射文件，再在后台用 `init_without_deploy()` 创建并安装新的 runtime/session bundle；focus 回调提前启动 reload，避免方案更新后的首键漏到宿主，reload 期间其余按键仍放行。部署只由主 App 完成，避免多个宿主进程并发改写 Rime build 文件。

主 App 的方案安装、手动部署和升级修复与 TSF 后台初始化共用 `Local\KeyTao.WindowsIme.EngineInit` 命名互斥量。重建前会删除 `.keytao-windows-build-repair-v1` 完成标记并写入 reload stamp，使已加载的宿主会话在按键路径上先放行；随后按微软 TSF 生命周期用 `ITfInputProcessorProfileMgr::DeactivateProfile(TF_IPPMF_FORSESSION)` 和 `ReleaseInputProcessor` 释放各宿主映射的 Rime 文件，只失效当前方案及其依赖的 `schema/prism/table/reverse` 产物，由主 App 后台部署，再写 reload stamp 和完成标记，并用新的 profile manager 恢复、校验部署前激活的 KeyTao profile。Windows 对尚在释放的 mapped/shared 文件会做最长 10 秒的有限重试。TSF 收到 reload stamp 后先释放旧 session，且在完成标记缺失时拒绝加载可能已损坏的旧构建产物，不会自行触发 deployment。

## 排查入口

- `windows_ime_status` 的 `packaged`、`registered`、`runtime_dir`、`dll_path`、`registered_path`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`。
- `registered_dll` 只表示 `HKCR\CLSID\{...}\InprocServer32` 指向当前 DLL；`profile_enabled` 才表示 TSF language profile 已启用。`registered` 必须同时满足两者。
- 注册表：`HKCR\CLSID\{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}\InprocServer32`。
- TSF profile：`langid=0x0804`，profile GUID `{1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}`，`TF_IPP_FLAG_ENABLED` 必须为 true。
- DLL 同级是否有 `rime.dll`。
- 运行时目录是否有 `rime-data/default.yaml`。
- `%APPDATA%/keytao/keytao-ime.reload` 的 mtime/size 是否随部署变化。
- UAC 是否允许注册脚本写入注册表。

## 发布门槛

- Windows x64 安装包必须同时通过 x86/x64/ARM64 目标、ARM64X forwarder、delay-load、静态 CRT、三个内嵌 icon resource、版本化 runtime staging 和 NSIS 架构选择检查。
- 微软要求第三方 IME DLL 使用可信代码签名。当前脚本会验证结构和资源，但仓库没有可提交的私钥；正式公开发行必须在 CI 注入代码签名证书并对两个 TIP DLL 及安装包签名。未签名 alpha 只能用于受控测试，Windows 安全策略仍可能阻止加载。
- `IMMERSIVESUPPORT` 只有在 AppContainer 下验证 Program Files shared data、可写 user data ACL/代理进程和真实候选输入后才能注册。
- 自绘候选窗还需要补齐 `IME_Candidate_Window` UIA automation id、menu open/close 与 selection 事件，才能声明完整 Narrator accessibility；当前基础 UIless contract 不等于完整 accessibility/search integration。
