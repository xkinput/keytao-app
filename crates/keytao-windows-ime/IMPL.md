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
3. 调用 `ITfCategoryMgr::RegisterCategory` 标记为 keyboard TIP，并声明 immersive support / UI element enabled 能力。
4. 调用 `ITfInputProcessorProfiles::Register` 注册 text service。
5. 优先调用 `ITfInputProcessorProfileMgr::RegisterProfile` 注册 `KeyTao` language profile；在现代 profile manager 不可用时才退回 `AddLanguageProfile`。
6. 调用 `EnableLanguageProfile` / `EnableLanguageProfileByDefault` 启用当前用户和默认语言 profile。
7. 通过 `ITfInputProcessorProfileMgr::GetProfile` 检查 `TF_IPP_FLAG_ENABLED`；profile 未启用时注册失败，而不是只凭 CLSID 写入报告成功。

只写入 CLSID / InprocServer32 代表 COM server 已注册，但不代表用户语言 profile 已启用；Windows 输入切换器只会切到已启用的 TSF language profile。

`DllUnregisterServer` 会反向移除 TSF profile、category 和 CLSID 注册表树。

Tauri 主 App 在启动后会先渲染界面，再通过后台任务检查 TSF 状态；若安装包完整但当前 DLL/profile 尚未注册，会异步触发提升权限 PowerShell 调用 `DllRegisterServer`，并通过 `windows-ime-status` 事件把“正在注册 / 已注册 / 注册失败”回传到界面。界面只保留刷新入口，不提供手动重装 TSF 或卸载按钮，避免注册流程阻塞首屏显示。
主 App 状态检查同样分开检查 DLL path 和 TSF profile enabled 状态：`registered=true` 必须同时满足已注册 DLL 路径匹配当前 runtime，并且 TSF profile 已启用。这样不会再把“COM DLL 注册表存在”误报成“输入法可切换”。

## TSF 官方契约对齐点

实现按 Microsoft TSF 文档保持这些边界：

- text service 除标准 COM in-proc server 注册外，还要通过 `ITfInputProcessorProfiles::Register`、`AddLanguageProfile` 和 `ITfCategoryMgr::RegisterCategory` 注册到 TSF。
- `ITfKeyEventSink` 由 text service 实现，并通过 `ITfKeystrokeMgr::AdviseKeyEventSink` 安装到当前 `ITfThreadMgr`。
- `OnTestKeyDown` / `OnTestKeyUp` 只声明当前按键是否会被处理；真正的状态更新、commit 和 composition 操作在 `OnKeyDown` / `OnKeyUp` 中完成。
- composition 必须在 TSF edit session 里用 `ITfContextComposition::StartComposition` 创建，用 `ITfRange::SetText` 更新 range，用 `ITfComposition::EndComposition` 结束；没有 active composition 的直接提交使用 `ITfInsertAtSelection::InsertTextAtSelection`。

参考：

- [Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)
- [ITfInputProcessorProfileMgr::RegisterProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)
- [ITfInputProcessorProfileMgr::GetProfile](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-getprofile)
- [ITfTextInputProcessor::Activate](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itftextinputprocessor-activate)
- [ITfKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfkeyeventsink)
- [ITfKeystrokeMgr::AdviseKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink)
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
3. `TextService::Activate` 保存 `ITfThreadMgr` / `client_id` 并注册 `ITfKeyEventSink`，不在激活阶段同步初始化 librime。
4. 第一次需要处理按键时，`TsfState::ensure_engine()` 创建 `keytao_core::ImeRuntime`，调用 `init()` 并创建 `ImeRuntimeSession`。
5. 后续按键通过 `KeyEventSink::OnKeyDown` 进入通用 runtime，再由 `keytao-core` 操作 librime。

`Activate` 只做轻量 TSF 接线，避免 Windows 在输入法切换、枚举或加载 TIP DLL 时被 librime 初始化阻塞。初始化失败时按键会放行并记录错误，不会让 TSF 激活本身失败。

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
8. `accepted=false` 时返回不消费。
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
- `Shift_L` / `Shift_R` 只在 solo key up 时以 release mask 送入 Rime。
- 其它 function key、媒体键等返回 `None`，输入法不拦截。

## 重载

Windows 用户目录为 `%APPDATA%/keytao`。TSF 前端在 `OnSetFocus`、`OnKeyDown` 和 solo `OnKeyUp` 前检查：

```text
%APPDATA%/keytao/keytao-ime.reload
```

stamp 的 mtime/size 签名变化后，`TsfState::check_reload_stamp()` 调用 `ImeRuntime::reload()`。已有 `ImeRuntimeSession` 会通过 core generation 懒刷新；当前 TSF context 则在下一次 edit session 中应用空 `ImeState`，清掉旧 preedit、composition 和候选窗，随后继续处理当前按键。

## 排查入口

- `windows_ime_status` 的 `packaged`、`registered`、`runtime_dir`、`dll_path`、`registered_path`、`user_data_dir`、`shared_data_dir`、`shared_data_source`、`reload_stamp_path`、`reload_stamp_signature`。
- `registered_dll` 只表示 `HKCR\CLSID\{...}\InprocServer32` 指向当前 DLL；`profile_enabled` 才表示 TSF language profile 已启用。`registered` 必须同时满足两者。
- 注册表：`HKCR\CLSID\{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}\InprocServer32`。
- TSF profile：`langid=0x0804`，profile GUID `{1B2C3D4E-5F60-7A8B-9C0D-1E2F3A4B5C6D}`，`TF_IPP_FLAG_ENABLED` 必须为 true。
- DLL 同级是否有 `rime.dll`。
- 运行时目录是否有 `rime-data/default.yaml`。
- `%APPDATA%/keytao/keytao-ime.reload` 的 mtime/size 是否随部署变化。
- UAC 是否允许注册脚本写入注册表。
