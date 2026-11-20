# 方案包隔离设计（per-package isolation）

说明：所有行号都是 `git show main:<path>` 里的行号。[U] 表示这条依据我对 librime 1.17 源码的了解，本地没有 .cc 源码可以核对，未验证。热点文件：keytao-core/src/lib.rs 共 10379 行，app-core/src/packages.rs 2289 行，package_activation.rs 1225 行。

## 1. 目标模型

**目录布局**（全平台一样，`<data>` 就是现在的 app data root）
- `<data>/scheme-packages/{cloud|local}/<key>/files/`：包的源文件，只读，现有结构不变（app-core package_store.rs:116,1118-1133）。
- `<data>/runtime/<key>/`：这个包自己的 Rime user_data_dir。放 KeyTao 生成的 `*.custom.yaml`、`default.custom.yaml`、rime.lua、`lua/`、user.yaml、installation.yaml、userdb。
- `<data>/runtime/<key>/build/`：staging_dir，也就是这个包的构建缓存。
- 内置方案也按"包"处理（`builtin/<key>`），shared 目录指向现在的捆绑目录。这样 macOS、iOS、Android 现有的 shared_data_dir 用法不变（lib.rs:551-558）。

**librime trait 指向**（RimeTraits 字段见 rime_api.h:87-116，KeyTao 设置它们的地方是 lib.rs:2271-2276）
- shared_data_dir = 包的 `files/`
- user_data_dir = `runtime/<key>`
- staging_dir = `runtime/<key>/build`
- prebuilt_data_dir = `files/build`（包自带 build 时才用）
- log_dir 保持全局。它只认第一次 setup 的值（lib.rs:843-851）。

**为什么 user 目录也要分包**：librime 查源文件时 user 目录优先于 shared 目录 [U]。共享用户目录里任何一个多余的 yaml（比如 symbols.yaml、另一个包的 default.custom.yaml）都会遮住所有包，这正是现在的 bug。

**保持全局的部分**
- 主题、app 设置、云同步状态、KeyTao 自定义词源数据：都在 app 层。
- 自定义词改为在各包 runtime 里按 owned/generated 文件重新生成，复用 managed_custom.rs:9-10,202 的生成段机制。
- sync：每个包的 installation.yaml 写入相同的 installation_id 和共享的 `sync_dir: <data>/sync`（key 名 [U]）。
- userdb 默认分包，见第 5 节的决策 D1。

**构建缓存与失效**
- 每个包一份 receipt：`runtime/<key>/build/.keytao-desktop-inputs.json`，复用 desktop_inputs::snapshot / desktop_deploy_plan（desktop_inputs.rs:11,162,256；lib.rs:1729）。
- schema_inputs 的 receipt 同样放到包内（schema_inputs.rs:10,87）。
- 输入哈希要覆盖：包文件、生成的 custom、所选 schema、add-on 叠加、内置公共文件的版本。
- 哈希一致就跳过部署，只做 `start_maintenance(full_check=false)`。这一步按 mtime 比较，只是一次 stat 扫描 [U]（调用点 lib.rs:2908-2911）。
- build_prune 只在本包的 build 内运行（build_prune.rs:29-60），所以不会再删掉别的包的缓存。

**active 包与切换**
- active 指针沿用 `active-manifest.json`（keytao-core package_store.rs:7），新增字段 `active_package`。
- 切换流程：
  1. 如果目标包的 receipt 过期，先部署这个包。
  2. 写 active 指针。
  3. 写 ReloadStamp，内容带上 package key（app-core ime_paths.rs:24-90）。
- 各 IME 进程看到 stamp 变了：丢掉 session，走 `shutdown_for_dir_change`（generation 加一，再 finalize，lib.rs:5562-5580），然后用新的 traits 调 initialize。同一个进程不能同时跑两个包（Service 是单例 [U]）。
- 现有的重载入口不用改：macOS `keytao_reload_if_stamp_changed`（EngineInit.swift:211-216），Windows server reload（server.rs:398-417），Linux reload_bus（reload_bus.rs:62），Android `reloadIfNeeded`（KeytaoImeEngine.kt:346,381），iOS `reloadIfNeeded`（KeyTaoKeyboardViewController.swift:267,348）。
- 包内切换方案仍然走 `select_schema`（lib.rs:3554,3641），记忆写在本包的 user.yaml。每个包各自记住上次用的方案，这是想要的行为。

**两级 F4**
- 第二级：librime 原生 switcher。它天然只列 active 包的 default.yaml schema_list（addon-default.yaml:5-6；write_effective_default_schema_list 在 lib.rs:7832）。
- 第一级：在 keytao-core 新建 `switcher_model.rs`，提供 `packages() -> [{key,name,active,ready}]` 和 `schemas(key)`。所有前端和 Flutter 都从这里拿列表，保证顺序和文案一致。
- 第一级怎么呈现，见第 5 节的决策 D2。

**app 首页**
- `_CurrentSchemeHero`（input_page.dart:161,185-258）改成两级：已安装包，然后是 `switcher_model.schemas(pkg)`。
- 新 bridge 接口 `switch_package(key)`，替代 `activate_scheme` 里复制文件的那条路（app-bridge core.rs:483）。`select_schema` 继续只在包内使用（core.rs:1069）。

## 2. 部署

- deploy_child 子进程（deploy_child.rs:343,386）对过期的包逐个处理：用这个包的 traits 调 initialize，跑 maintenance，等它结束，再 finalize。active 包排第一个，用户能尽早打字。
- 进度协议：`Message::Progress` 扩成 `{package, name, index, total, step}`，step 取值为 `materialize|compile|validate|prune|skip`。
- `CoreEvent::DeployProgress(String)`（app-core deploy.rs:380,465,686）改成结构体，经 bridge 传到 Flutter 的 `_deployStep`（controller.dart:2024-2034），每个包一行。
- IME 的提示改成"正在部署 <包> (i/n)"：macOS deploymentHint（EngineInit.swift:92,222），Android 通知（KeytaoRimeDeployService.kt:137），Windows `windowsRedeployImeAction` 的结果也带上分包步骤。
- 失败隔离：每个包单独记录 `{ok|failed, error}`。失败的包保留它上一次能用的 build，不删别的包的任何东西。如果 active 包失败，就留在旧 build 上并报错。validate_deployed_schemas 也按包运行。
- Windows 分阶段部署：复制范围改成 `runtime/<key>/`，交换 `runtime/<key>/build` 和 `previous-build`（staged_deploy.rs:35-56,137-200）。只有 active 包需要走这条路（它的 bin 被 server 锁着），非 active 包直接部署进自己的 build。
- Android：`:rime_deployer` 的 deploy_android_staged/schema（lib.rs:1526,1955）接收目标包的目录。`:ime` 只在 active 包的 build 完成后才认 stamp。

## 3. 迁移（一次性，可回退）

- 旧的扁平 root 原样保留，作为 legacy，不移动也不删除。回退方式：把 `layout=flat` 写回 manifest。
- 对每个已激活包：
  - 建 `runtime/<key>`。
  - 生成 custom、rime.lua，把它的 `.keytao-package-lua/<key>` 挪进 runtime 的 `lua/`（packages.rs:360-420）。
  - 用户自己写的、属于本包 schema 的 `<schema>.custom.yaml` 复制进来，按 schema_id 归属判断。
- userdb 不复制正在运行的 leveldb。做法是先在 legacy root 上跑 sync_user_data，导出 `sync/<id>/*.userdb.txt`；各包 runtime 用相同的 id 和 sync_dir，第一次同步时合并进来 [U]。
- 孤儿文件：legacy root 里既不属于任何包、也不在保护名单（install.rs:13-25）里的文件，用 package_orphans（package_orphans.rs:172）列给用户看，不自动删除。
- 迁移在 app 进程执行，执行前写 stamp 让 IME 暂停。这是数据布局变更，需要人工确认（human gate）。

## 4. 冲突

- 包和包之间的同名文件冲突不会再出现，因为每个包有自己的 shared 目录和 user 目录。
- 剩下的冲突只有两种：包内同名文件（包自己的问题），以及 add-on 叠加进主包时的冲突。
- `check_package_file_conflicts`（install.rs:532-590）改成以 `runtime/<key>` 为 root，只用来拦截 add-on。

## 5. 风险与待决策

- **D1 userdb**：
  - 默认按包隔离，同名词库通过共享 sync_dir 合并，代价是同步较贵。
  - 备选：用 junction/symlink 把每个包的 `<dict>.userdb` 指到一份全局 userdb。同一进程同时只开一个包，锁的情况和现在一样，但 Windows 的 junction 和 Android 两个进程的 LOCK 需要实测。
- **D2 F4 第一级**：
  - X：各前端在菜单里加"方案包"子菜单。macOS 是 InputController.menu（InputController.swift:843-852），Windows 是语言栏（language_bar.rs:43-46），Linux 是 IBusPropList 和 tray（gnome_ibus_engine.rs:305-308，tray.rs:15-28），Android 是 RIME_SCHEMA 命令（KeytaoInputMethodService.kt:2165-2184），iOS 在 selectRimeSchema 上面加一层（KeyTaoKeyboardViewController.swift:622）。
  - Y：switcher 打开时再按一次 F4，由 core 合成包列表作为候选。只需改 core 一处，前提是确认所有前端都只渲染 core 给的候选（未核实，global_input.rs:25-26 只能说明它们靠 `.default` 判断）。
  - 建议先做 Y，Y 验证不通过再做 X。
- **D3 add-on**（如 Easy English）：在每个主包的 runtime 里各叠加一份 owned 文件，并用 merge_default_custom_content（keytao-core lib.rs:8278-8300）并进该包的 schema_list。用户按主包选择是否启用。
- **D4 公共文件**：shared 目录换成包目录之后，key_bindings、punctuation、symbols、opencc、essay 会找不到。包里缺什么，就从捆绑的 rime-data 复制什么进 runtime，记为 generated，只补缺失的。
- **D5 rime.lua**：每个包一份，放在它自己的 runtime 里。rime_lua::select（rime_lua.rs:259-340）只在包内切换。regenerate_rime_lua_for_ime 和 rime_lua::signature 改成接收 runtime 目录（lib.rs:1310,1627）。
- **风险**：
  - 切包延迟没测过（lua VM 重启加 bin 映射，估计几十 ms [U]）。
  - 磁盘占用乘以包数。
  - local_import.rs:20 和 package_orphans 都假设只有一个 root。
  - 切包后 registry 和 lua 模块状态是否残留 [U]（grammar_tests.rs:602）。

## 6. 分批计划（串行；lib.rs 和 packages.rs 同一时间只给一个批次）

- **B0 纯重构漏斗**
  - 内容：新建 keytao-core/src/runtime_dirs.rs，`RuntimeDirs::resolve(root, shared) -> {user, shared, staging, prebuilt}`，目前返回和今天相同的扁平值。把 rime_build_dirs、initialize/reinitialize、deployed_schemas、部署签名（lib.rs:1625-1653）、upgrade_deploy.rs:53、staged_deploy、ReloadStamp 都改为经过它。
  - 验收：golden test 覆盖各个 cfg，对比新旧目录一致；`pytest`/`cargo test` 原有测试不改动全部通过；`cargo test -- --list` 的用例数不少于基线。
- **B1 runtime 物化**（不动 lib.rs）
  - 内容：app-core 新增 runtime_materialize.rs，复用 smart_install_files，把 root 设成 runtime 目录：生成 custom 和 rime.lua、补公共文件、叠加 add-on。用 flag 关闭，不影响现有行为。
  - 测试：两个包都带 symbols.yaml 时互不影响；add-on 冲突只拒绝当前主包。
- **B2 分包部署和结构化进度**（持有 deploy_child.rs 和 app-core deploy.rs）
  - 内容：分包 receipt，包未变化时跳过，失败隔离；Progress 结构体经 bridge 传到 Flutter。
  - 测试：未变化的包被跳过；一个包失败时其他包的 build 文件 md5 不变；进度事件顺序正确。
- **B3 切换**（持有 lib.rs）
  - 内容：resolver 读 active_package；stamp 带 key；Windows 按包分阶段交换；Android 部署器接收目标包。
  - 测试：stamp 变化后目录随之变化；对已缓存的包调切换，不触发编译。
- **B4 迁移**：在主动启用隔离时迁移，保留旧布局，列出孤儿，通过快照迁移 userdb。测试：迁移可重复执行（幂等）；回退后 legacy 文件 md5 不变。实机迁移需要人工确认。
- **B5 UI（展示层）**：switcher_model.rs，Flutter 首页两级选择，F4 第一级（D2）。每个平台的菜单单独一个批次，只做重新渲染检查。
- **B6 模块边界**：旗标关闭时，已存储方案包的激活走 `legacy_flat`，由它负责共享目录复制、跨包 Lua 快照及共享 `default.custom.yaml` 合并。隔离切包、物化和分包部署走各自的 runtime。旗标仍默认关闭。

---
# Critique

**Blocking（必须改设计）**
1. 必须把"控制根目录"和"Rime 用户目录"分开，B0 漏了这一层。ReloadStamp 的位置固定在 `user_data_dir/keytao-ime.reload`（lib.rs:9053-9055），ReloadStampGate 也按同一目录建（lib.rs:9130-9131）。另外 `apply_switch_request(root)`、`publish_current_schema(root)` 按 root 写（lib.rs:5555-5580），`deploymentHint(root)` 也是按 root 取（state.rs:1592、KeytaoImeEngine.kt:185）。如果 user_dir 改成 `runtime/<key>`，IME 会去盯旧包目录里的 stamp，结果是永远不重载。修法：root 保持不变，作为控制根，由 core 在 initialize 内部把它解析成 runtime/<key>。
2. B0 宣称"所有目录都经过 RuntimeDirs"，但各前端其实是自己算 root 的，B0 没列进去：macOS EngineInit.swift:26,162，主题文件路径 ImeTheme.swift:344；Android `KeytaoAndroidPaths.userRootOrNull`（KeytaoImeEngine.kt:51）；iOS KeyTaoIOSPaths（KeyTaoKeyboardViewController.swift:1832）；Linux engine.rs:17、main.rs:209-219、upgrade_deploy.rs:30；Windows server.rs:374,455,519,614，engine_pipe.rs:85，state.rs:414,611,1562。按第 1 条的做法，这些地方都不用改。
3. Windows 漏了两条路径。第一条是 TSF DLL 的进程内引擎：它在宿主进程里直接用 `default_user_data_dir()` 建 ImeRuntime（engine_backend.rs:266-284），并不只是 server 在用。第二条是 AppContainer 沙箱：它把整个 root 连同 build 复制到私有目录（appcontainer.rs:79-91,256-291,370-405），按 generation 和 librime 版本做 key。这带来两个后果。(a) 每次切包都会整份复制一遍 build，"切换很快"这个说法在沙箱宿主里不成立；沙箱缓存需要按包分 key。(b) 设计说"非 active 包直接部署进自己的 build"，但切走后的那段窗口里，还没重载的宿主进程仍然映射着旧包的 bin，Windows 上覆盖这些文件会失败。所以要么所有包都走 staged swap（staged_deploy.rs:35-56），要么等所有 generation 确认之后再部署。
4. iOS 上缓存这条路不成立。iOS 强制全量部署：快照直接是 None（lib.rs:1360），每次都 deploy（lib.rs:1681），没有 deploy plan（lib.rs:1732），begin 直接走全量（lib.rs:1758）。不改这几处，"哈希一致就跳过"在 iOS 上不会生效。B2 和 B3 都没有列出这些点。
5. sync 的设计会把数据写坏。所有包共用同一个 installation_id 和 sync_dir，各包都写 `sync/<id>/`：配置备份（*.custom.yaml）会互相覆盖，userdb 快照会被反复改写 [U librime UserDbSync/BackupConfigFiles]。sync_dir 是用户在 app 里设的，可能是跨设备同步盘（rime_sync.rs:30-46,169-174），迁移会污染它。另外，"第一次同步时合并"需要每个包各自显式调一次 sync_user_data_and_wait（lib.rs:2937）。没有任何地方会自动触发它，所以迁移完用户词频看起来就是丢了。建议：迁移时把 userdb 导出到私有临时目录，再逐包 import（或者用 D1 的全局 userdb），不要碰用户的 sync_dir。
6. 自定义词的源头漏了。KeyTao 的自定义词在 root 下的 `keytao.user.dict.yaml`，由账号同步读写（account.rs:429,458,811,896,1114），不在 managed_custom 里。account.rs、rime_sync.rs、upload_bundle.rs（upload_bundle.rs:48-80 会校验 manifest 的 root 必须等于上传源）都没出现在任何批次里。
7. "跳过时只做 start_maintenance(full_check=false)"这一条：代码里目前只有 `start_maintenance(1)`（lib.rs:2921-2924），而且是在部署路径里。IME 走的是 init_without_deploy，不跑维护。如果在 IME 进程里调维护，它会写 installation.yaml，shared 目录的 mtime 一变还会触发 workspace_update [U]。建议 receipt 命中时什么都不调，直接 initialize。
8. D2-Y 很可能做不到。librime 的 Switcher 写死了 schema_list_translator 和 switch_translator，不能注入候选 [U switcher.cc]。global_input.rs:25-26 只能说明前端靠 `.default` 判断有没有进入 switcher，说明不了候选来自哪里。要做 Y，core 得自己维护一套菜单状态，所有前端的候选渲染都要接到这套状态上，这不是"只改 core 一处"。建议直接选 X，或者先用一个 spike 证伪 Y。
9. 从 IME 发起切包时，目标包必须已经构建好。Android 的 :ime、Windows 的 DLL 和 AppContainer、iOS 扩展都不能在 IME 进程里部署，AppContainer 也写不了 app data。所以 `ready=false` 的包要交给 app 处理，不能在 IME 内"先部署再切"。B3 没写这个约束。

**Migration / 数据**
10. 迁移之前必须先跑 `package_activation::recover`（package_activation.rs:793），否则会和没收尾的 activation 事务冲突。现在可以同时激活多个主包（read_all，package_activation.rs:75），所以迁移需要一条确定的规则来选 active_package，比如用当前 schema 所属的包。
11. 只按 schema_id 归属复制 `<schema>.custom.yaml` 会漏东西：default.custom.yaml 生成段以外用户手写的 patch、squirrel/weasel.custom.yaml、custom_phrase.txt、用户自建的 *.dict.yaml，还有依赖 schema 的 custom 文件。这些文件只会出现在孤儿列表里，迁移后就悄悄不生效了。建议默认复制给每个包，标记为 user-owned。

**行号与事实偏差**（main 上实测）
12. lib.rs 实际 10410 行，不是 10379；`shutdown_for_dir_change` 在 5593（设计写 5562）；maintenance 在 2921（设计写 2908）。B0 引用的锚点要用 main 重新生成一遍。
13. macOS 的 shared 目录会回退到 Squirrel 的 SharedSupport（EngineInit.swift:46-49，lib.rs:8809 起）。"builtin 包的 shared 指向捆绑目录"这一条需要说清楚回退顺序。

**改进建议**
14. B1 说"不动 lib.rs"，但 rime.lua 的生成在 lib.rs（regenerate_rime_lua_for_ime，lib.rs:1167,1312），还要改 rime_lua::signature。要么把 B1 改成可以动 lib.rs，要么 B0 先把这两处收口。
15. D4 里每个包都复制一份 essay、opencc，磁盘和编译开销都会乘以包数。哈希要纳入捆绑版本，essay 可以考虑用硬链接。librime 只有 user 和 shared 两级回退 [U]。
16. log：lib.rs:569 取的是 `user_dir/log`，prune 也是按包目录跑，但 glog 只用第一次 setup 的目录（lib.rs:844-851）。log_dir 应该固定在控制根下面。
17. 切包延迟需要在 B3 实测，并定一个阈值。Windows 上要把 N 个宿主进程里的进程内引擎和沙箱复制算进去，"几十 ms"这个估计没有依据。

---
# Owner decisions (2026-11-20)
- D1 userdb: per package (each package its own userdb in runtime/<key>).
- D2 F4 first level: inside the F4 switcher a first level lists packages (display name + folder name); choosing one switches package, then shows that package's schema_list. librime Switcher cannot inject candidates: KeyTao-core keeps the menu state, every frontend renders it (spike first).
- D3 add-ons: installed and switched independently like main packages. Exception: Easy English can be added independently to each package (per-package overlay).
- D4 user custom files: copied into every package runtime at migration, marked user-owned.
- D5 migration never touches the sync dir.

- B0 merged. B1 must address: relative 'build/...' literals (staged_deploy.rs:76-77,194-198; desktop_inputs.rs:11-12; schema_inputs.rs:10; deploy_child.rs:20; build_prune.rs; package_artifacts.rs; upload_bundle.rs; package_orphans.rs:128-134,203; windows_public_schemas.rs:23,669; appcontainer.rs:125) once staging leaves root/build; golden test per-cfg.

- B1 merged. MUST FIX before the flag can be on (B4): package_orphans.rs:122 with flag on lists legacy flat build/* as orphans (cleanup would delete them); runtime_dirs.rs:37 falls back silently to flat root when package files dir missing (fail closed + log).

## B6 交接与默认开启

- B4 已保护旧版 `build/` 免于孤儿清理，并在已选隔离包缺少源文件目录时报错。
- 保留平铺事务恢复和迁移所需的激活记录、备份读取。旧版首装、显式目录导出、包移除继续使用现有流程。Windows 隔离公共缓存仍按单包生成 Lua 快照。
- 默认开启前需把应用内首装接到方案包仓库及分包部署，明确隔离包移除和回退的处理方式，并验证迁移、旧布局回退和各平台运行时切包。迁移实机用户数据仍需负责人确认。
- 负责人决定何时默认启用隔离，以及旧版平铺激活和回退入口保留多久。移除旧版实现前，先确认支持期内的旧布局能迁移及回退。
