# Windows IME 实现说明

本文只记录 `crates/keytao-windows-ime` 里的 Windows TSF 输入法实现。

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
3. 调用 `ITfInputProcessorProfiles::Register`。
4. 调用 `AddLanguageProfile` 注册 `KeyTao` profile。
5. 调用 `ITfCategoryMgr::RegisterCategory` 标记为 keyboard TIP。
6. 注册 immersive support 和 UI element enabled 能力。

`DllUnregisterServer` 会反向移除 TSF profile、category 和 CLSID 注册表树。

Tauri 主 App 的 Windows 管理按钮会用提升权限 PowerShell 加载 DLL，并调用 `DllRegisterServer` / `DllUnregisterServer`。

## 初始化

文件：

- `src/lib.rs`
- `src/text_service.rs`
- `src/state.rs`

流程：

1. TSF 调用 `DllGetClassObject`。
2. `ClassFactory::CreateInstance` 创建 `TextService`。
3. `TextService::Activate` 初始化 `TsfState`。
4. `TsfState::init_engine()` 调用 `keytao_core::deploy()`。
5. `TextService::Activate` 注册 `ITfKeyEventSink`。
6. 后续按键通过 `KeyEventSink::OnKeyDown` 进入 librime。

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
2. `OnKeyDown` 把 Windows Virtual Key 转成 librime 使用的 X11 keysym。
3. `current_mod_mask()` 读取 Shift、Control、Alt 状态。
4. 调用 `engine.process_key_result(keysym, mods)`。
5. `accepted=false` 时返回不消费。
6. 有 `committed` 时：
   - 若存在 TSF composition，替换 composition range 并结束 composition。
   - 否则用 `ITfInsertAtSelection` 直接插入。
7. 有 `preedit` 时：
   - 若存在 composition，更新 composition range。
   - 否则从当前 caret 开始 `StartComposition`。
8. preedit 清空且无 commit 时，结束 composition。
9. 候选窗口用 `CandidateWindow` 绘制并按 caret screen position 定位。

Windows 的 composition 生命周期比较显式，所以 commit 与新 preedit 同时出现时，可以先结束旧 composition，再创建或更新新 composition。

## key map

`src/key_map.rs` 维护 VK 到 X11 keysym 的转换：

- 字母统一传小写 keysym，Shift 通过 modifier 表示。
- 数字、小键盘、常见标点映射到 ASCII keysym。
- Backspace、Tab、Return、Escape、Space、Delete、方向键等映射到 XK 值。
- Function key、媒体键等返回 `None`，输入法不拦截。

## 排查入口

- `windows_ime_status` 的 `packaged`、`registered`、`dll_path`、`registered_path`。
- 注册表：`HKCR\CLSID\{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}\InprocServer32`。
- DLL 同级是否有 `rime.dll`。
- 运行时目录是否有 `rime-data/default.yaml`。
- UAC 是否允许注册脚本写入注册表。
