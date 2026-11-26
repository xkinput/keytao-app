# KeyTao 移动端自定义键盘规范（v2）

本文档定义 KeyTao Android / iOS 软键盘的声明式配置格式、助记绑定机制、方案推荐绑定、内置键盘清单与导入导出协议。相关实现位于 `crates/keytao-theme/src/mobile_layout.rs` 与 `crates/keytao-theme/src/keyboard_store.rs`。

---

## 1. 架构与文件职责

与 `docs/theme-spec.md` 的主题源/编译分离原则一致：

| 路径 | 职责 | 写入方 | 参与分享 |
|---|---|---|---|
| `crates/keytao-theme/keyboards/*.yaml` | 6 套内置键盘源（随应用编译内嵌） | 仓库维护 | 可导出 |
| `<userRoot>/keyboards/<id>.yaml` | 用户导入或自定义的键盘源文件（YAML v2） | App / 导入器 | 是 |
| `<userRoot>/keyboard.yaml` | 当前生效键盘的编译落盘文件（含 `keyboardId`、`keyboardHash`） | `keyboard_store::apply_keyboard` | 否 |
| `<userRoot>/android_ime.json` / `ios_ime.json` | 本机交互与显示开关（如 `mnemonicHintsEnabled`、`numberRowEnabled`） | App / IME 面板 | 否 |

- **共享解析**：Android（JNI `resolveKeyboardJson`）、iOS（C FFI `keytao_resolve_keyboard_json`）与 Flutter 预览（`ime_settings`）全部消费 `keytao-theme::mobile_layout` 的统一解析 JSON。
- **继承默认附属层**：当 `inheritsDefaultLayers` 为 `true`（默认）时，未显式覆盖的 `numberRows`、`symbolRows`、`layers` 与几何参数自动继承 `default-keyboard.yaml`，使自定义键盘源与分享串保持精简（通常 2–8 KB）。
- **无损迁移**：若升级前用户手改过 `keyboard.yaml`（无 `keyboardId`/`keyboardHash` 且与历史默认哈希不同），首次读取库时自动迁移为 `keyboards/my.custom.yaml`，不丢失用户布局。
- **残留清理保护**：`package_orphans.rs` 与 `package_migration.rs` 将 `themes/`、`keyboards/`、`theme.yaml`、`keyboard.yaml` 及系统元文件（`.DS_Store`、`Thumbs.db`、`desktop.ini`）排除在残留文件之外。

---

## 2. 键盘配置格式（version: 2）

### 2.1 顶层字段

```yaml
version: 2

meta:
  id: "keytao.keydo30"            # 唯一 ID；内置使用 keytao.*，用户/导入建议使用 <namespace>.<name> 或 my.*
  name: "键道·我流"                # 界面展示名称（必填，1–40 字符）
  author: "KeyTao"                # 可选作者
  description: "Colemak-DH 30键，含我流音形助记"  # 可选说明
  recommendedSchemas: ["keydo"]   # 可选：推荐绑定此键盘的 Rime 方案 ID 列表

inheritsDefaultLayers: true       # 默认 true：未声明的 numberRows/symbolRows/layers/几何继承默认布局
numberRow: auto                   # auto（跟随常驻数字行开关）| embedded（键盘自带顶部数字行）| hidden（隐藏开关）

# 几何尺寸（可选，省略则继承默认；所有数值经 sanitize clamp）
height: 266                       # 160–420
candidateBarHeight: 52            # 36–96
bottomInset: 0                    # 0–80
horizontalGap: 4                  # 0–24
verticalGap: 5                    # 0–24
outerInset: 5                     # 0–32
maxKeyHeight: 54                  # 36–84
symbolAutoReturn: false
phrases: ["谢谢", "好的", "收到", "稍等", "我在路上"]

# 键盘专属颜色分组（可选，支持明暗两套覆盖；未设字段继承当前主题）
keyGroups:
  row1:
    light: { background: "#F5DEB3", foreground: "#263442", hintForeground: "#6B5A3E", borderColor: "#E0C48D" }
    dark:  { background: "#4A3F2B", foreground: "#F5E9D3", hintForeground: "#C9B48C", borderColor: "#63543A" }

# 主键盘行（必填，非空二维按键数组）
rows:
  - [ ... ]

# 可选覆盖
numberRows: ...
symbolRows: ...
layers:
  custom_layer:
    - [ ... ]
```

### 2.2 按键定义（`MobileKey`）

| 字段 | 类型 | 说明 |
|---|---|---|
| `label` | `string` | 主显示文本 |
| `value` | `string?` | 默认输入值（省略 `action` 时自动生成 `input` 命令） |
| `asciiLabel` / `asciiValue` | `string?` | 英文 / ASCII 模式下的标签与输入值 |
| `rimeValue` | `string?` | 发往 Rime 引擎的编码串（如三拼主键发送 `q`–`b`、符号键发送 `;k`） |
| `weight` | `f32?` | 相对键宽（clamp `0.25..=8.0`，默认 `1.0`） |
| `rowSpan` | `u8?` | 纵向跨行数（`1..=8`），与 `stack` 配合实现数字键盘或三拼左侧纵向符号列 |
| `stack` | `MobileKeyStackItem[]` | 纵向堆叠子键列表 |
| `style` | `string?` | 兼容 v1 `"accent"` |
| `group` | `string?` | 引用顶层 `keyGroups.<name>` 或内置语义组 |
| `hint` | `string?` | 兼容 v1 右上角小字，等价于 `slots.topRight` |
| `slots` | `MobileKeySlots?` | 常规键面多区域小字（见 2.3） |
| `mnemonic` | `MobileKeyMnemonic?` | 键盘助记数据（见 2.4） |
| `action` / `asciiAction` | `MobileCommand?` | 点击命令 |
| `swipeUp` / `swipeDown` | `MobileCommand?` | 上滑 / 下滑命令 |
| `swipeLeft` / `swipeRight` | `MobileCommand?` | 左滑 / 右滑命令（可选） |
| `longPress` / `asciiLongPress` | `MobileCommand?` | 长按命令 |
| `alternates` / `asciiAlternates` | `MobileKeyAlternate[]` | 长按候选弹窗 |

### 2.3 常规键面多槽位（`slots`）

```yaml
slots:
  topLeft: "Q"                  # 左上角小字
  topCenter: "ˉ"                # 顶部居中
  topRight: "↑"                 # 右上角小字（未填时回退 key.hint）
  labelSub: "z"                 # 主标签右侧次/加粗标记（如 "zh" + "z"）
  bottomLeft: ""                # 左下小字
  bottomCenter: "a,an,ang 一"   # 底部单行提示
  bottomRight: ""               # 右下小字
  bottomLines: ["line1", "line2"] # 底部多行提示（优先于 bottomCenter）
```

### 2.4 键盘助记（`mnemonic`）

助记不再作为系统级硬编码功能绑定 `keytao` 方案 ID，而是直接声明在键盘配置的按键上：

```yaml
mnemonic:
  top: "zh"                  # 主标右上角声母/笔画标记（如 zh、ch、sh、丿、丨、丶、㇕、一、~、∅）
  topBadge: "月"             # 可选：紧邻 top 的代表字根（如我流 U 键「丿 月」、A 键「㇕ 氵」）
  bottomLines:               # 底部 1–2 行韵母或字根提示
    - "iu ua"
  tone: outer                # outer | special | brand | guide
  group: stroke              # 可选：开启助记时覆盖使用的 keyGroups 分组名
  hideRegularSlots: true     # 默认 true：开启助记时隐藏常规 hint/slots
```

- **自动检测 `hasMnemonics`**：解析器遍历全部按键，只要存在任意非空 `mnemonic`，输出 JSON 中 `hasMnemonics = true`，否则为 `false`。
- **设置项显隐**：Flutter 键盘设置页与 Android/iOS 键盘内设置面板**仅在 `hasMnemonics == true` 时显示「开启键盘助记」开关**；否则不显示。
- **默认值**：`mnemonicHintsEnabled` 默认为 `false`（即助记版键盘默认不显示助记，需用户打开开关后展示）。
- **渲染条件（Android / iOS 一致）**：`mnemonicHintsEnabled && layout.hasMnemonics && layer == "letters" && !isEnglishMode && !asciiMode`。

### 2.5 命令类型（`MobileCommand`）

支持以下标准命令类型（`{ type, value?, fallbackValue? }`）：

- **输入类**：`input`、`rimeInput`、`directInput`
- **基础功能键**：`backspace`、`enter`、`space`、`shift`、`reset`
- **层与模式**：`keyboardMode`（`letters`、`numbers`、`symbols:last`、`emoji:last`、`editor`、`back` 或自定义层名）、`mode`（`toggle`/`chinese`/`english`）
- **光标与编辑（`edit`）**：`selectAll`、`cut`、`copy`、`paste`、`tab`、`lineStart`、`lineEnd`、`cursorLeft`、`cursorRight`、`cursorUp`、`cursorDown`、`undo`、`redo`、`deleteWord`、`clearAll`、`forwardDelete`、`repeatCommit`
- **布局与系统**：`oneHanded`、`floating`、`keyboardPicker`、`nextInputMethod`、`openPage`、`panel`、`setting`、`keyboardProfile`
- **Rime 交互**：`rimeMenu`、`rimeSchema`、`rimeOption`、`nextPage`、`previousPage`

### 2.6 移除「输入 `=` 替换第一行为数字键」

- 移除 Android 与 iOS 中 `shouldUseInlineNumberRow()`（输入编码包含 `=` 时把第一行字母临时替换为数字行）的行为。
- 保留用户可配置的「常驻数字行」（`numberRowEnabled`）：
  - 当键盘声明 `numberRow: embedded`（自带顶部数字行）或 `hidden` 时不再重复插入。
  - 当 `numberRow: auto` 且开关打开时，插入独立 10 键数字行（`1..0`，提示 `!@#$%^&*()`），不再按第一行列数截断。

---

## 3. 内置 6 套键盘方案

| 编号 | ID | 名称 | 布局结构 | 含助记 | 推荐绑定方案 |
|---|---|---|---|---|---|
| 1 | `keytao.qwerty26` | 标准 26 键（默认） | 10 / 9 / 9 / 5（原默认布局） | 否 | 拼音方案（薄荷、万象、明月、双拼等） |
| 2 | `keytao.keytao26` | 键道6 助记键盘 | 标准 26 键 + 键道6 音形助记（默认关闭） | 是 | — |
| 3 | `keytao.keytao30` | 键道6 键盘 | 10 / 10 / 9 / 6（含 `; , . /`，底行无「键」键、「数/符」居左）+ 键道6 30 键助记 | 是 | `keytao`、`xmjd`、`xmjd-feiding`、`txjx`、`snow_jiandao` |
| 4 | `keytao.keydo30` | 键道·我流 | Colemak-DH 30 键（10 / 10 / 9 / 6：`QWFPBJLUY;` / `ARSTGMNEIO` / `⇧ZXCDVKH⌫` / `数 / 空格 , . ↵`）+ 我流音形助记 | 是 | `keydo` |
| 5 | `keytao.lssp_3x5` | 李氏三拼 3×5 | `numberRow: embedded`（顶部 1–0 行）+ 左侧 `rowSpan: 3` 常用符号栈列（`，。？！`）+ 3×5 声韵调主键区（黄/蓝/粉三行色组，发往 Rime `qwert/asdfg/zxcvb`）+ 右侧 `⌫/←/→` + 底行 | 否 | `triple_pinyin_lssp` |
| 6 | `keytao.lssp_4x4` | 李氏三拼 4×4 | 左侧 `rowSpan: 3` 常用符号栈列 + 3×4 主键区 + 第 4 行 `!?#` / `zh z` / `ch c`(w2) / `sh s` / `,` + 底行 `中/En` / `李氏三拼`(w4) / `Enter` | 否 | — |

---

## 4. 方案推荐与移动端切换询问

1. **推荐匹配**：根据当前激活的 Rime 方案 ID（或方案 key），在键盘库中匹配 `recommendedSchemas`（未知拼音方案回退至 `keytao.qwerty26`）。
2. **设置页展示**：
   - 若当前方案存在推荐键盘，在键盘设置页顶部提示「当前方案推荐使用 XX 键盘」，未启用时提供一键切换按钮。
   - 键盘方案列表以 **2 列卡片网格** 展示：卡片上方为键盘实时预览，下方为键盘名称与「使用中 / 推荐 / 助记」标识。
3. **切换方案后弹窗询问（移动端）**：
   - 在 Android / iOS 端切换输入方案成功后，若目标方案有推荐键盘且与当前生效键盘不同，弹出确认弹窗。
   - 弹窗内直接展示**推荐键盘的完整预览效果**，用户可选择「切换」或「暂不切换」。

---

## 5. 导入、导出与分享

复用 `crates/keytao-theme/src/share.rs` 的规范化与压缩协议：

- **分享串**：`keytao-keyboard:2:<base64url_nopad(zlib(canonical_json(source)))>`
- **导出文件**：`<id>.keytao-keyboard.json`（RFC 8785 canonical JSON）
- **文本清洗**：导入前剥离所有空白与零宽字符（保留纯 JSON 字符串内空格），支持粘贴分享串或以 `{` 开头的 JSON，以及从文件导入。
- **体积上限**：
  - 分享串超过 `4000` 字符时提示改用文件分享。
  - 解压后源 JSON 上限 `256 KiB`（支持重度全层自定义）；清洗后输入文本上限 `384 KiB`。
- **冲突处理**：导入前调用 `preview_keyboard_import`，若 ID 与内置或已有自定义键盘冲突，提供「覆盖（非内置）」或「另存为副本（自动分配 `my.<slug>-N`）」选项。
