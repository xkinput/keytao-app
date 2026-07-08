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
3. 调用 `ITfCategoryMgr::RegisterCategory` 标记为 keyboard TIP，并声明 immersive support / UI element enabled / systray support 能力。
4. 调用 `ITfInputProcessorProfiles::Register` 注册 text service。
5. 优先调用 `ITfInputProcessorProfileMgr::RegisterProfile` 注册 `KeyTao` language profile，并传入 DLL 同级 `keytao.ico` 作为 profile icon；在现代 profile manager 不可用时才退回 `AddLanguageProfile`。
6. 调用 `EnableLanguageProfile` / `EnableLanguageProfileByDefault` 启用当前用户和默认语言 profile。
7. 通过 `ITfInputProcessorProfileMgr::GetProfile` 检查 `TF_IPP_FLAG_ENABLED`；profile 未启用时注册失败，而不是只凭 CLSID 写入报告成功。
8. 调用 `InstallLayoutOrTip`，用 `0x0804:{CLSID}{ProfileGUID};` 把 KeyTao 加入当前用户启用的输入法列表，确保安装后可被输入切换器真正选中。

只写入 CLSID / InprocServer32 代表 COM server 已注册，但不代表用户语言 profile 已启用，也不代表当前用户的输入法列表已经包含该 TIP；Windows 输入切换器只会切到已启用且加入用户输入法列表的 TSF language profile。

`DllUnregisterServer` 会先用 `InstallLayoutOrTip` 的 uninstall flag 从当前用户输入法列表移除 KeyTao，再反向移除 TSF profile、category 和 CLSID 注册表树。

Tauri 主 App 在启动后会先渲染界面，再通过后台任务检查 TSF 状态；若安装包完整但当前 DLL/profile 尚未注册，会异步触发提升权限 PowerShell 调用 `DllRegisterServer`，并通过 `windows-ime-status` 事件把“正在注册 / 已注册 / 注册失败”回传到界面。界面只保留刷新入口，不提供手动重装 TSF 或卸载按钮，避免注册流程阻塞首屏显示。
主 App 状态检查同样分开检查 DLL path 和 TSF profile enabled 状态：`registered=true` 必须同时满足已注册 DLL 路径匹配当前 runtime，并且 TSF profile 已启用。这样不会再把“COM DLL 注册表存在”误报成“输入法可切换”。

## TSF 官方契约对齐点

实现按 Microsoft TSF 文档保持这些边界：

- text service 除标准 COM in-proc server 注册外，还要通过 `ITfInputProcessorProfiles::Register`、`RegisterProfile` / `AddLanguageProfile` 和 `ITfCategoryMgr::RegisterCategory` 注册到 TSF。
- IME profile icon 必须是随 runtime 安装的 `.ico` 文件；KeyTao 使用 DLL 同级 `keytao.ico`，而不是把 DLL 自身当成 icon 文件。
- 安装后要调用 `InstallLayoutOrTip`，把 `<LangID>:{CLSID}{ProfileGUID};` 加进当前用户输入法列表；只启用 language profile 仍可能导致输入切换器里可见但不能真正切换。
- 现代 Windows 可能查询 `ITfTextInputProcessorEx`；KeyTao 同时实现 `ITfTextInputProcessor` 和 `ITfTextInputProcessorEx`，`ActivateEx` 复用同一个轻量 `Activate` 路径。
- `ITfKeyEventSink` 由 text service 实现，并通过 `ITfKeystrokeMgr::AdviseKeyEventSink` 安装到当前 `ITfThreadMgr`。
- `ITfThreadMgrEventSink` 由 text service 在 `Activate` 时通过 `ITfSource::AdviseSink(IID_ITfThreadMgrEventSink, ...)` 安装，并在 `Deactivate` 时用 cookie 反注册；焦点、document manager 和 context 生命周期通知只做轻量状态清理和候选窗隐藏。
- `OnTestKeyDown` / `OnTestKeyUp` 只声明当前按键是否会被处理；真正的状态更新、commit 和 composition 操作在 `OnKeyDown` / `OnKeyUp` 中完成。
- composition 必须在 TSF edit session 里用 `ITfContextComposition::StartComposition` 创建，用 `ITfRange::SetText` 更新 range，用 `ITfComposition::EndComposition` 结束；没有 active composition 的直接提交使用 `ITfInsertAtSelection::InsertTextAtSelection`。
- 候选窗必须使用当前 `ITfContextView::GetWnd()` 返回的窗口作为 owned window；如果 TSF context 没有窗口，则退回 `GetFocus()`。候选窗显示、隐藏、位置/大小变化分别通过 `NotifyWinEvent(EVENT_OBJECT_IME_SHOW/HIDE/CHANGE, ...)` 通知 Windows light-dismiss/accessibility 管线。
- 触摸键盘或 `SendInput` 产生的 Unicode 输入会以 `VK_PACKET` 进入 `ITfKeyEventSink`；KeyTao 使用 `GetKeyboardState` + `ToUnicode(VK_PACKET, ...)` 提取字符，并把触摸键盘候选翻页的 `0xF003` / `0xF004` 映射到 Rime 的 PageDown / PageUp keysym。

参考：

- [Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)
- [ITfInputProcessorProfileMgr::RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)
- [ITfInputProcessorProfileMgr::GetProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-getprofile)
- [InstallLayoutOrTip](https://learn.microsoft.com/en-us/windows/win32/tsf/installlayoutortip)
- [IME requirements](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)
- [ITfTextInputProcessor::Activate](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itftextinputprocessor-activate)
- [ITfKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfkeyeventsink)
- [ITfKeystrokeMgr::AdviseKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink)
- [ITfSource::AdviseSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfsource-advisesink)
- [ITfThreadMgrEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfthreadmgreventsink)
- [ITfContextComposition::StartComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontextcomposition-startcomposition)
- [ITfComposition::EndComposition](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcomposition-endcomposition)
- [ITfInsertAtSelection::InsertTextAtSelection](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinsertatselection-inserttextatselection)

## 初始化

文件：

- `src/lib.rs`
- `src/text_service.rs`
- `src/state.rs`

流程：

1. TSF 调用 `DllGetClassObject`。
2. `ClassFactory::CreateInstance` 创建 `TextService`。
3. `TextService::Activate` 保存 `ITfThreadMgr` / `client_id`，注册 foreground `ITfKeyEventSink` 和 `ITfThreadMgrEventSink`，然后只启动后台 engine warmup，不等待 librime。
4. 后台 warmup 在不持有 `TsfState` mutex 的情况下创建 `keytao_core::ImeRuntime`，调用 `init()` 并创建 `ImeRuntimeSession`，完成后只用短锁安装到共享状态中。
5. warmup 未完成或 reload 正在运行时，`OnTestKeyDown` 仍按 KeyTao 会处理的键声明接管，确保 TSF 继续把真实 key event 送到 `OnKeyDown`；`OnKeyDown` 不同步等待 librime，只触发后台 warmup/reload 并在未 ready 时放行。
6. 后续按键通过 `KeyEventSink::OnKeyDown` 进入通用 runtime，再由 `keytao-core` 操作 librime。

`Activate` 只做轻量 TSF 接线和后台预热调度，避免 Windows 在输入法切换、枚举或加载 TIP DLL 时被 librime 初始化阻塞。初始化失败时按键会放行并记录错误，不会让 TSF 激活本身失败。

切换输入法时必须避免在 TSF 回调线程里做重活或持锁调用可重入 API：

- `DllMain` 只保存 DLL instance 并调用 `DisableThreadLibraryCalls`；日志订阅器等会分配、读环境变量或拿锁的初始化延后到 `DllGetClassObject` / 注册导出函数里，避免 Windows loader lock 下卡死宿主进程。
- `keytao_windows_ime.dll` 在 MSVC 构建中把 `rime.dll` 配置为 delay-load，并在第一次真正初始化引擎前用 DLL 同级绝对路径预加载 `rime.dll`；这样 Windows 切换输入法载入 TSF DLL 时不会同步加载整条 librime 依赖链，也不会从宿主进程目录错误解析同名 DLL。
- `TextService::Activate` / `Deactivate` 不在持有 `TsfState` mutex 时调用 `AdviseKeyEventSink`、`AdviseSink`、`UnadviseKeyEventSink` 或 `UnadviseSink`，避免 TSF 同步回调 `OnSetFocus` 时自锁。
- `CandidateWindow::new()` 不创建 HWND、不读取字体；候选窗和字体在首次显示候选或模式提示时懒加载，避免 TSF 创建 text service 时卡住输入线程。
- `apply_ime_state()` 不在持有 `TsfState` mutex 时调用 `StartComposition` / `SetText` / `EndComposition`，并在同步 edit session 返回后才刷新候选窗，避免 `OnCompositionTerminated` 或宿主窗口消息重入时再次拿锁造成死锁。
- librime 初始化和 reload 都通过后台线程执行；TSF 回调线程只检查状态、启动任务或使用已经 ready 的 session，锁只用于安装已创建的 runtime/session 或更新轻量状态。
- `OnTestKeyDown` 只读取缓存 `ImeState`，不调用 `ImeRuntimeSession::state()`，避免 TSF 按键探测阶段进入 librime 或触发 generation refresh。
- `ImeRuntimeSession::state()` / `process_key_result()` / `select_candidate()` / `reset()` 先 clone session 句柄再执行，避免在 `TsfState` mutex 内进入 librime 或触发 generation refresh。
- 候选窗 Win32 show/hide 会临时从 `TsfState` 中取出窗口对象后再执行，避免 `UpdateLayeredWindow`、字体加载或窗口销毁和 TSF 状态锁相互耦合。

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
   - 若存在 TSF composition，替换 composition range 并结束 composition。
   - 否则用 `ITfInsertAtSelection` 直接插入。
10. 有 `preedit` 时：
   - 若存在 composition，更新 composition range。
   - 否则从当前 caret 开始 `StartComposition`。
11. preedit 清空且无 commit 时，结束 composition。
12. 候选窗口用 `CandidateWindow` 绘制并按 caret screen position 定位。

`OnKeyUp` 只处理 solo Shift release：发送 `Shift_L` 或 `Shift_R` keysym，mask 为 `RIME_RELEASE_MASK`。Shift key down 本身不送 Rime；如果 Shift 按下后又出现其它 keyDown，pending flag 会被清除。

Windows 的 composition 生命周期比较显式，所以 commit 与新 preedit 同时出现时，可以先结束旧 composition，再创建或更新新 composition。

## 候选窗与主题

文件：

- `src/candidate_win.rs`
- `src/panel.rs`

候选窗和中英模式提示都是 Win32 layered popup window，位置跟随 TSF caret screen position。`candidate_win.rs` 只负责窗口生命周期、屏幕边界、mode hint timer 和 `UpdateLayeredWindow` 上传；`panel.rs` 负责把 UI model 渲染成 BGRA buffer。

主题接入方式与 Linux 自绘通道一致：

1. `keytao-theme::ThemeResolver` 从默认主题和用户主题解析 `ResolvedImeTheme`。
2. `panel.rs` 把 `ImeState` 转成 `CandidatePanelModel`，统一 select key label、highlight、翻页和横竖排语义。
3. `panel.rs` 把 `ascii_mode` 转成 `ModeHintModel`，统一 `中`/`英` 文案、背景色、前景色、字号、尺寸、圆角和显示时长。
4. Windows renderer 把 `ResolvedImeTheme + CandidatePanelModel / ModeHintModel` 渲染到 layered window 像素 buffer。

`theme.yaml` v2 支持 `ui.colorScheme: auto | light | dark`、`ui.accentColor` 和 `light:` / `dark:` 模式变体；`auto` 跟随系统主题，Windows 自绘候选窗会消费解析后的最终主题。

用户主题路径跟随 `keytao_core::default_user_data_dir()`，即 `%APPDATA%/keytao/theme.yaml`；开发覆盖可用 `KEYTAO_IME_THEME_PATH`。

## key map

`src/key_map.rs` 维护 VK 到 X11 keysym 的转换：

- 字母按 Shift 状态传小写或大写 ASCII keysym，同时保留 Shift modifier mask，语义与 Linux/macOS 的“keysym 表示实际字符、mask 表示修饰键”一致。
- 数字、小键盘、常见标点映射到 ASCII keysym。
- Backspace、Tab、Return、Escape、Space、Delete、方向键等映射到 XK 值。
- `VK_F4` 映射为 `XK_F4` / `0xffc1`，用于打开 Rime schema / options 菜单。
- `VK_PACKET` 使用 `GetKeyboardState` + `ToUnicode` 解包 Unicode 字符；触摸键盘发送的 `0xF003` / `0xF004` 会映射到 `XK_Page_Down` / `XK_Page_Up`。
- `Shift_L` / `Shift_R` 只在 solo key up 时以 release mask 送入 Rime。
- 其它 function key、媒体键等返回 `None`，输入法不拦截。

## 重载

Windows 用户目录为 `%APPDATA%/keytao`。TSF 前端只在真正处理按键的 `OnKeyDown` 和 solo `OnKeyUp` 前检查：

```text
%APPDATA%/keytao/keytao-ime.reload
```

stamp 的 mtime/size 签名变化后，`start_reload_if_needed()` 只在 `TsfState` 里标记 reload running 并启动后台线程调用 `ImeRuntime::reload()`；reload 期间按键放行。后台 reload 成功后，已有 `ImeRuntimeSession` 会通过 core generation 懒刷新；当前 TSF context 则在下一次 edit session 中应用空 `ImeState`，清掉旧 preedit、composition 和候选窗，随后继续处理按键。

## 排查入口

- `windows_ime_status` 的 `packaged`、`registered`、`runtime_dir`、`dll_path`、`registered_path`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`。
- `registered_dll` 只表示 `HKCR\CLSID\{...}\InprocServer32` 指向当前 DLL；`profile_enabled` 才表示 TSF language profile 已启用。`registered` 必须同时满足两者。
- 注册表：`HKCR\CLSID\{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}\InprocServer32`。
- TSF profile：`langid=0x0804`，profile GUID `{1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}`，`TF_IPP_FLAG_ENABLED` 必须为 true。
- DLL 同级是否有 `rime.dll`。
- 运行时目录是否有 `rime-data/default.yaml`。
- `%APPDATA%/keytao/keytao-ime.reload` 的 mtime/size 是否随部署变化。
- UAC 是否允许注册脚本写入注册表。
