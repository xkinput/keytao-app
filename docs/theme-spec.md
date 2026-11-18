# KeyTao 主题系统设计 · 主题文件规范 v1

状态：草案第 2 版（2026-11-18，已按评审 30 条修订，见附录）。下文锚点都相对仓库根目录。`lib.rs` 指 `crates/keytao-theme/src/lib.rs`。

本规范扩展现有 `keytao-theme`，不替换它。核心决定只有一条：

> **主题源文件（v3）只存在于 `themes/<id>.yaml`。native 端读取的 `theme.yaml` 保持 v2 扁平结构，由 App “编译”生成。**

这样旧版 IME 二进制（例如 Windows 上宿主进程未重启、仍加载旧 TSF DLL 的情况）读到的仍是合法 v2 文件，外观正确；native 读取路径不改。

图例：✓ 已渲染；◐ 部分渲染或借用别的字段；○ 本规范新增，尚未实现；— 不适用/无法控制。
PC 列：M = macOS，W = Windows，Lp = Linux 自绘面板（`UiCapabilities::full_custom()`，`crates/keytao-linux-ime/src/panel.rs:674`），Lk = Linux kimpanel（`UiCapabilities::system_lookup_table()`，`crates/keytao-linux-ime/src/kimpanel.rs:194`，所有样式字段均不生效）。

---

## A. 支持项罗列

所有数值范围**直接取自 `sanitized()`（`lib.rs:1097-1131`）**，那里是唯一来源；新增字段的范围以本表为准，实现时加进 `sanitized()`。超出范围一律 clamp，不拒绝。

### A.1 通用（`common`）

| 项 | 字段 | 类型/取值 | 默认 | PC | Android | iOS | 现状 | 优先级 |
|---|---|---|---|---|---|---|---|---|
| 强调色 | `common.accentColor` | color | 未设 | M W Lp ✓，Lk — | ✓ | ✓ | 已有（`ui.accentColor`） | P0 |
| 全局圆角 | `common.cornerRadius` | 0–28（`lib.rs:243-247`） | 未设 | M W Lp ✓ | ✓ | ✓ | 已有（`ui.cornerRadius`） | P0 |
| 主背景色 | `common.backgroundColor` | color | 未设 | M W Lp ✓ | ✓ | ✓ | 已有（`ui.backgroundColor`），派生规则见 B.6 | P1 |
| 字体族 | `common.font.family` | 逗号分隔字符串（回退链） | 未设（系统字体） | M✓ W○ Lp○ | — | — | 已有，`Option<String>`（`lib.rs:1212`）；macOS 已支持逗号回退链（`crates/keytao-macos-ime/Sources/KeyTaoIME/ImeTheme.swift:459`） | P1 |
| 字重 | `common.font.weight` | ultralight\|thin\|light\|regular\|medium\|semibold\|bold\|heavy\|black | semibold | M✓ W○ Lp○ | ○ | ○ | 已有 9 档（`lib.rs:292-302`，`ImeTheme.swift:9-19`） | P1 |
| 主题身份 | `meta.id`、`name`、`author` | string | 必填 id、name | — | — | — | 新增 | P0 |
| 外观适配 | `meta.appearance` | both\|light\|dark | 见 B.5 推断规则 | ✓ | ✓ | ✓ | 新增 | P0 |
| 明暗变体 | `light:` / `dark:` 块 | 子树覆盖 | 无 | ✓ | ✓ | ✓ | 已有，作用范围扩大到 mobile | P0 |

移动端不支持字体族：Android IME 现在只用 `Typeface.MONOSPACE`（`flutter_app/android/app/src/main/kotlin/ink/rea/keytao_app/KeytaoKeyboardView.kt:3035`），`Typeface.create` 只能解析系统字体族；iOS 键盘扩展无法使用用户安装的字体。需要时放到 v2 的内嵌字体。

### A.2 PC 候选面板（`pc`）

| 项 | 字段 | 类型/取值 | 默认 | PC | 现状 | 优先级 |
|---|---|---|---|---|---|---|
| 排列方向 | `pc.panel.orientation` | vertical\|horizontal | vertical | M W Lp ✓ | 已有 | P0 |
| 面板背景 | `pc.panel.background` | color | `#F8FAFFF2` / dark `#171A20F2` | M W Lp ✓ | 已有 | P0 |
| 面板边框色、宽 | `pc.panel.borderColor`、`borderWidth` | color、0–4 | `#B8C3D0`、1 | M W Lp ✓ | 已有 | P0 |
| 面板圆角 | `pc.panel.cornerRadius` | 0–28 | 16 | M W Lp ✓ | 已有 | P0 |
| 内边距 | `pc.panel.paddingX`、`paddingY` | 0–32、0–28 | 10、10 | M W Lp ✓ | 已有 | P0 |
| 候选间距 | `pc.panel.gap` | 0–24 | 4 | M W Lp ✓ | 已有 | P0 |
| 编码与候选的间距 | `pc.panel.preeditGap` | 0–48 | 10 | M W Lp ✓ | 已有 | P1 |
| 尺寸限制 | `pc.panel.minWidth`、`maxWidth`、`maxHeight`、`screenMargin` | 48–480、160–2400、80–1600、0–40 | 见默认文件 | M W Lp ✓ | 已有，`maxWidth` 照常保留（B.4） | P2 |
| 面板阴影 | `pc.panel.shadow` | bool | true | M✓ W○ Lp○ | 已有，Windows 未实现 | P1 |
| 候选字号 | `pc.font.size` | 10–36 | 20 | M W Lp ✓ | 已有（`font.size`） | P0 |
| 序号、注释、编码字号 | `pc.font.labelSize`、`commentSize`、`preeditSize` | 9–28 | 15、16、15 | M W Lp ✓ | 已有 | P1 |
| 候选底色 | `pc.candidate.background` | color | 见 `crates/keytao-theme/default-theme.yaml:42` | M W Lp ✓ | 已有（`lib.rs:409`） | P1 |
| 候选文字色 | `pc.candidate.foreground` | color | `#263442` | M W Lp ✓ | 已有 | P0 |
| 高亮候选文字和背景 | `pc.candidate.selectedForeground`、`selectedBackground` | color | `#243241`、`#E6F0FF` | M W Lp ✓ | 已有 | P0 |
| 序号色（普通、高亮） | `pc.candidate.labelColor`、`selectedLabelColor` | color | `#7F8D9C`、accent | M W Lp ✓ | 已有 | P0 |
| 注释色（普通、高亮） | `pc.candidate.commentColor`、`selectedCommentColor` | color | 以 YAML 为准（B.9） | M W Lp ✓ | 已有 | P0 |
| 悬停背景 | `pc.candidate.hoverBackground` | color | `#F1F6FF` | M✓ W○ Lp○ | 已有 | P1 |
| 按下背景、文字 | `pc.candidate.pressedBackground`、`pressedForeground` | color | `#D4E7FF`、`#14233B` | M W Lp ✓ | 已有 | P2 |
| 候选边框 | `pc.candidate.borderColor`、`selectedBorderColor`、`borderWidth` | color、0–3 | transparent、`#5DA7D7`、1 | M W Lp ✓ | 已有 | P1 |
| 高亮圆角 | `pc.candidate.cornerRadius` | 0–24 | 11 | M W Lp ✓ | 已有 | P0 |
| 候选内边距、行内距、最小高、最大宽 | `pc.candidate.paddingX/paddingY/inlineGap/minHeight/maxWidth` | 0–28、0–24、0–18、20–72、72–640 | 见默认文件 | M W Lp ✓ | 已有 | P2 |
| 分隔线 | `pc.candidate.separatorVisible`、`separatorColor` | bool、color | false、`#DCE7F7` | M✓ W○ Lp○ | 已有 | P2 |
| 序号后缀 | `pc.candidate.labelSuffix` | ≤4 字素簇 | "." | M W Lp ✓ | 已有（`lib.rs:1005`） | P1 |
| 翻页按钮 | `pc.navigation.foreground/disabledForeground/hoverBackground/buttonSize/cornerRadius` | color、18–56、0–20 | 见默认文件 | M W Lp ✓（disabledForeground 仅 M） | 已有 | P2 |
| 中英提示框 | `pc.modeHint.background/foreground/borderColor/borderWidth/fontSize/width/height/cornerRadius/shadow` | color、0–4、12–42、36–180、28–140、0–32 | 见默认文件 | M W Lp ✓ | 已有 | P1 |
| 中英提示文字 | `pc.modeHint.chineseText`、`englishText` | ≤4 字素簇 | "中"、"英" | M W Lp ✓ | 已有 | P2 |
| 提示时长 | `pc.modeHint.duration` | 0.15–4 秒 | 0.75 | M W Lp ✓ | 已有 | P2 |

Lk 列全部为 —，表中省略。

### A.3 Mobile 键盘（`mobile.keyboard`）

按键样式字段集合记作 `KeyStyle`，定义见 B.7。

| 项 | 字段 | 类型/取值 | 默认（派生见 B.6） | Android | iOS | 现状 | 优先级 |
|---|---|---|---|---|---|---|---|
| 键盘背景色 | `mobile.keyboard.background` | color | `pc.panel.background` 混 accent 7% | ◐ | ◐ | 新增（现在借用 panel，`flutter_app/lib/src/ui/mobile_keyboard_preview.dart:58`） | P0 |
| 按键底色 | `key.background` | color | B.6 | ◐ | ◐ | 新增（现借 `candidate.background`，`KeytaoKeyboardView.kt:6979`，`crates/keytao-ios-ime/Sources/KeyTaoIOSIME/KeyTaoIOSKeyboardView.swift:7133-7134`） | P0 |
| 按键文字色 | `key.foreground` | color | `pc.candidate.foreground` | ◐ | ◐ | 新增 | P0 |
| 按键提示字色 | `key.hintForeground` | color | `pc.candidate.commentColor` | ◐ | ◐ | 新增 | P0 |
| 按键圆角 | `key.cornerRadius` | 0–24 | `pc.candidate.cornerRadius × 10/11` | ◐ | ◐ | 新增（`mobile_keyboard_preview.dart:61`） | P0 |
| 按下底色、文字 | `key.pressedBackground`、`pressedForeground` | color | `pc.candidate.selectedBackground`、`pressedForeground` | ◐ | ◐ | 新增 | P0 |
| 功能键 | `keyTypes.function.*` | KeyStyle | 底色 = `pc.panel.background` 混 accent 22%（柔和强调） | ◐ | ◐ | 新增（`KeytaoKeyboardView.kt:7000-7011,7073-7078`） | P0 |
| 空格、回车、退格 | `keyTypes.space/enter/backspace.*` | KeyStyle | 继承 function | ◐ | ◐ | 新增 | P1 |
| Shift | `keyTypes.shift.*` | KeyStyle | 继承 character | ◐ | ◐ | 新增 | P1 |
| 强调键类 | `keyClasses.accent.*` | KeyStyle | 同 function 的柔和强调 | ◐ | ◐ | 已有，但 `style=="accent"` 先被柔和强调分支截走（`KeytaoKeyboardView.kt:6975-6978`，`KeyTaoIOSKeyboardView.swift:7127-7131`） | P0 |
| 强调标点类 | `keyClasses.accentPunctuation.*` | KeyStyle | 同 function 的柔和强调 | ◐ | ◐ | 已有，写死 `，。,.`（`KeytaoKeyboardView.kt:7351`，`KeyTaoIOSKeyboardView.swift:7403`） | P0 |
| 选中态 | `keyStates.selected.*` | KeyStyle 颜色子集 | B.6 | ◐ | ◐ | 新增（iOS `KeyTaoIOSKeyboardView.swift:7121-7125`） | P1 |
| Shift 开 | `keyStates.shiftOn.*` | KeyStyle 颜色子集，仅作用于 shift 键 | 继承 `keyClasses.accent` | ◐ | ◐ | 新增 | P1 |
| 按键边框 | `key.borderColor`、`borderWidth` | color、0–2 | light `#1A222C1C`，dark `#FFFFFF16`；宽 1 | ◐ | ◐ | 新增（现写死，`KeytaoKeyboardView.kt:6965-6969`） | P1 |
| 按键阴影 | `key.shadowColor`、`shadowOffsetY` | color、0–4 | transparent、1 | ○ | ○ | 新增，**仅实心偏移阴影，无模糊** | P1 |
| 字号缩放 | `key.fontScale`、`hintFontScale` | 0.8–1.3 | 1.0 | ◐ | ◐ | 新增，乘在现行布局算出的字号上 | P1 |
| 上下滑提示字 | `key.swipeHintForeground`、`swipeHintVisible` | color、bool | `hintForeground`、true | ◐ | ◐ | 新增（布局来源 `crates/keytao-theme/src/mobile_layout.rs:83,85`） | P1 |
| 工具栏 | `mobile.keyboard.toolbar.background`、`foreground`、`selectedForeground` | color | transparent、`pc.navigation.foreground`、accent | ◐ | ◐ | 新增 | P1 |
| 扩展页面背景 | `mobile.keyboard.pageBackground` | color | `mobile.keyboard.background` | ◐ | ◐ | 新增（openPage：剪贴板、更多面板） | P1 |
| 浮动键盘外框 | `mobile.keyboard.floating.borderColor`、`shadowColor` | color | 同 `key.borderColor`、transparent | ◐ | ◐ | 新增 | P1 |
| Android 导航栏区域 | 无字段 | — | 取 `mobile.keyboard.background` | ◐ | — | 行为约定 | P1 |
| iOS 地球、麦克风区域 | 无字段 | — | 系统绘制 | — | — | 主题无法控制 | — |
| 键盘几何 | `height`、`horizontalGap`、`verticalGap`、`outerInset`、`maxKeyHeight` | — | — | ✓ | ✓ | 已有，**留在 keyboard.yaml，不进主题** | — |

### A.4 Mobile 候选栏（`mobile.candidateBar`）

| 项 | 字段 | 类型/取值 | 默认 | Android | iOS | 现状 | 优先级 |
|---|---|---|---|---|---|---|---|
| 栏背景 | `background` | color | transparent | ◐ | ◐ | 新增 | P0 |
| 候选文字色、高亮色 | `foreground`、`selectedForeground`、`selectedBackground` | color | 继承 `pc.candidate.*` | ◐ | ◐ | 新增（现在直接用 pc 值） | P0 |
| 注释色 | `commentColor` | color | 继承 `pc.candidate.commentColor` | ◐ | ◐ | 新增 | P1 |
| 字号缩放 | `fontScale` | 0.8–1.3 | 1.0 | ◐ | ◐ | 新增 | P0 |
| 显示注释 | `showComment` | bool | true | ◐ | ◐ | 新增 | P1 |
| 编码行 | `preeditForeground`、`preeditBackground` | color | 继承 `candidate.foreground`、transparent | ◐ | ◐ | 新增 | P1 |
| 展开候选面板背景 | `expandedBackground` | color | `mobile.keyboard.background` | ◐ | ◐ | 新增 | P2 |
| 栏高 | `candidateBarHeight` | — | 52 | ✓ | ✓ | 已有，**留在 keyboard.yaml** | — |

### A.5 不属于主题的现有设置

`ui.colorScheme`、`ui.embeddedComposition`、`ui.hideCandidatePanel`、`ui.showTrayModeIndicator` 是本机设置。它们**留在 theme.yaml 的 `ui:` 原位**，不进主题源文件、不参与分享。按键音、振动、按键气泡等行为设置不在本规范范围内。

---

## B. 主题文件规范 v1（源文件 `version: 3`）

### B.1 格式

- **主题源文件** `themes/<id>.yaml`：YAML，`version: 3`。只在本机落盘，手写用户可以加注释。
- **分享与粘贴导入**：只接受分享串或 JSON（`serde_json`），见 C.1。
- 键名沿用现有 camelCase。现有别名 `mode_hint`、`night`（`lib.rs:1166,1173`）继续接受，导出只写规范名。

### B.2 文件与职责

| 文件 | 内容 | 写入方 | 是否分享 |
|---|---|---|---|
| `themes/<id>.yaml`（用户数据目录下新增） | 主题源：`version: 3`、`meta`、`common`、`pc`、`mobile`、`light`、`dark` | App 编辑器、导入 | 是 |
| `theme.yaml`（现有路径，`lib.rs:651-675`） | 编译产物：v2 扁平结构 + 可选 `mobile:` 块 + 顶层 `themeId`、`themeHash`；`ui:` 只保留 A.5 的本机设置 | 只由编译器写 | 否 |
| `keyboard.yaml` | 布局与几何 | 现有 | 否（v1） |

**编译（应用主题）**：Rust 新增 `compile_theme(source, base_theme_yaml) -> theme.yaml`：
1. 对 light、dark 各解析一次 v3 源，得到两份 `ResolvedImeTheme`。
2. 写出 v2 扁平文件：基础层 = light 解析结果的全部显式字段，`dark:` = dark 解析结果的全部显式字段，`mobile:` 与 `dark.mobile:` 同理。`ui:` 中**不写** `accentColor`、`cornerRadius`、`backgroundColor`（已烘焙进各字段），保留现有本机设置。
3. 写 `version: 2`、`themeId`、`themeHash = sha256(canonical_json(source))` 的前 16 位十六进制。

因为两种外观都写满显式字段，旧解析器（先内置默认 base+dark，再用户 base+dark，`lib.rs:806-841`）得到的结果与新解析器逐字段相同。旧解析器遇到未知的 `mobile:`、`themeId`、`themeHash` 会忽略（`PartialTheme` 未启用 `deny_unknown_fields`）。

App 版本升级后在启动时重新编译一次当前主题，让新版内置默认值生效。

**真源冲突**：应用主题前，先读 theme.yaml 的内容，按编译规则重算一次 hash 与 `themeHash` 比对。不一致说明用户手改过 theme.yaml，弹出：**另存为新主题**（把当前 theme.yaml 按 B.4 迁移成新源文件）或**丢弃手改**。

### B.3 顶层结构（源文件）

```yaml
version: 3
meta:
  id: "rea.mint-glass"
  name: "薄荷玻璃"
  author: "rea"                  # optional
  appearance: both               # optional, both | light | dark
common: { accentColor, cornerRadius, backgroundColor, font: { family, weight } }
pc:
  font: { size, labelSize, commentSize, preeditSize }
  panel: { ... }
  candidate: { ... }
  navigation: { ... }
  modeHint: { ... }
mobile:
  keyboard: { background, pageBackground, key, keyTypes, keyClasses, keyStates, toolbar, floating }
  candidateBar: { ... }
light: { common?, pc?, mobile? }   # partial overrides
dark:  { common?, pc?, mobile? }
```

`THEME_SCHEMA_VERSION`（`lib.rs:38`）保持 2，它描述的是 theme.yaml 与 resolved JSON；新增常量 `THEME_SOURCE_VERSION = 3`，用于源文件和分享串前缀。

### B.4 从 v2 迁移与写回函数

**迁移入口**：首次启动新版 App 时，当前 theme.yaml 迁移为 `themes/my.current.yaml`，然后编译回 theme.yaml。迁移是一次性的。

字段映射：

| v2 | v3 源 |
|---|---|
| `ui.accentColor` / `cornerRadius` / `backgroundColor` | `common.*` |
| `ui.colorScheme`、`nightMode`、`embeddedComposition`、`hideCandidatePanel`、`showTrayModeIndicator` | 留在 theme.yaml `ui:`，不进源 |
| `font.family`、`weight` | `common.font.*` |
| `font.size`、`labelSize`、`commentSize`、`preeditSize` | `pc.font.*` |
| `panel`、`candidate`、`navigation`、`modeHint`（含 `maxWidth`） | `pc.*` |
| `dark:` / `light:` 下的同名块 | `dark.pc.*` / `light.pc.*` |
| `keyboard:`（旧布局块，`lib.rs:1171`） | 迁出到 keyboard.yaml（仅当 keyboard.yaml 不存在） |

**外观等价保证**：现行三个旋钮最后施加并覆盖显式值——accent 覆盖 selected/pressed/hover 系列和 modeHint 前景（`lib.rs:887-889,1077-1094`），cornerRadius 覆盖四处圆角（`lib.rs:846-851`），backgroundColor 按对比度整套换成另一变体的配色（`lib.rs:852-884`）。v3 改为“未设才派生”（B.6），所以迁移按下列步骤做：
1. 按上表机械映射得到源 S。
2. 用**旧解析器**对原 theme.yaml 分别解析 light、dark，得到 O_light、O_dark。
3. 用新解析器解析 S，得到 N_light、N_dark。
4. 凡是 N 与 O 不相等的字段，把 O 的值显式写进 S 的 `light:` / `dark:` 块。
5. 必须有一个 golden 测试：一组 v2 夹具（含“浅色模式选深色背景”、“accent + 显式 selectedBackground”、“cornerRadius + 显式 candidate 圆角”）迁移后，新旧解析结果在两种外观下逐字段相等。

**写回函数**：

| 函数 | 现在写什么 | 改为 |
|---|---|---|
| `write_ime_ui_settings_to_path`（`lib.rs:46-92`） | `ui`、`panel.orientation`、`font.size` | `colorScheme` 等本机设置写 theme.yaml `ui:`；accent、圆角、方向、字号写当前源文件，然后重新编译 |
| `write_theme_appearance_to_path`（`lib.rs:106`） | `ui` 外观项 | accent、圆角、背景写源文件，`colorScheme` 写 theme.yaml，然后重新编译 |
| IME 内的调用方：Android `KeytaoNativeBridge.writeThemeUi/writeThemeAppearance`（`flutter_app/android/app/src/main/kotlin/ink/rea/keytao_app/KeytaoNativeBridge.kt:48-60`），iOS `keytao_write_theme_ui/appearance`（`crates/keytao-ios-ime/Sources/KeyTaoIOSIME/KeyTaoIOSEngine.swift:719-727`） | 直接改 theme.yaml | 签名不变，行为随上面两个 Rust 函数改变；源文件目录与 theme.yaml 同目录，扩展可写 |

**resolved JSON 冻结**：`resolved_theme_json`（`lib.rs:893`）的现有结构不变——`version`、`ui`（含 `colorScheme`、`effectiveColorScheme` 等，macOS 用 `try decode` 读取，`ImeTheme.swift:273-276`）、`panel.maxWidth` 和 `candidate.maxWidth`（macOS 非可选，`ImeTheme.swift:108,132`）、`font.family` 为字符串（`ImeTheme.swift:89`）。**只允许追加** `mobile` 子对象。

### B.5 明暗变体

- 每个字段都可以在 `light:` / `dark:` 块里按同路径覆盖。不支持内联 `{light, dark}` 值。
- **解析顺序**（每个字段先命中先用）：`源[外观].path` → `源.path` → 内置默认 `[外观].path` → 内置默认 `.path` → B.6 派生。
- **外观推断**：源文件没有 `meta.appearance` 时：
  - 有 `dark:` 或 `light:` 块，或者基础层没有设置任何颜色字段 → `both`。
  - 否则（基础层设了颜色，但没有变体块）→ 按解析后 `pc.panel.background` 的相对亮度推断：小于 0.18 为 `dark`，否则 `light`（阈值与 `lib.rs:1080` 一致）。
- `appearance: light` 时，系统处于深色也用浅色解析结果，两个外观都**不从内置另一变体补字段**。`dark` 同理。这防止“用户只在基础层设了深色文字，到深色模式拿到内置深色背景”的混色。
- 移动端明暗判断（边框、回退底色等写死值）一律以**生效外观**为准，即 resolved JSON 新增的 `mobile.isDark`，不再按 `panel.background` 亮度猜（现状：`KeytaoKeyboardView.kt:7190-7193`、`mobile_keyboard_preview.dart:54-56`）。

### B.6 默认、继承与派生

允许部分主题。最小合法源只需 `version: 3` 和 `meta.id`、`meta.name`。

派生只在 Rust 实现一次。Kotlin `withAccentHex`（`KeytaoTheme.kt:79`）、Swift `withAccent`（`crates/keytao-ios-ime/Sources/KeyTaoIOSIME/KeyTaoIOSTheme.swift:223`）、Flutter 预览里的派生改为读取 resolved JSON 的 `mobile` 对象。

| 未设字段 | 派生自 |
|---|---|
| `pc.candidate.selectedLabelColor`、`selectedBorderColor`、`selected/pressed/hoverBackground`，`pc.modeHint.borderColor`、`foreground` | `common.accentColor`，公式同 `lib.rs:1077-1094` |
| `pc.panel` / `candidate` / `navigation` / `modeHint` 的圆角 | `common.cornerRadius` × 1、11/16、10/16、14/16 |
| `pc.panel.background` | `common.backgroundColor`（保留原 alpha）。若该外观下源未设置 `pc.candidate.foreground`，按对比度选用内置 light 或 dark 的整套前景与交互色，规则同 `lib.rs:852-884` |
| `mobile.keyboard.background` | `pc.panel.background` 混 accent 7% |
| `mobile.keyboard.key.background` | `pc.candidate.background`（alpha > 0 时原样）；否则 light `#FFFFFFD2`、dark `#2A303AAA` |
| `keyTypes.function.background`、`keyClasses.accent/accentPunctuation.background` | `pc.panel.background` 混 accent 22%（与 Android/iOS IME 一致，`KeytaoKeyboardView.kt:7073-7078`；Flutter 预览现在用键盘背景混合，`mobile_keyboard_preview.dart:68`，改为对齐） |
| `keyStates.selected.background` | 柔和强调键：`pc.panel.background` 混 accent 24%；其他键：`pc.candidate.selectedBackground` 乘 alpha 0.62（light）/ 0.48（dark） |
| `mobile.keyboard.key.cornerRadius` | `pc.candidate.cornerRadius` × 10/11 |
| `mobile.candidateBar.*` 颜色 | `pc.candidate.*` 同名字段 |

**alpha 规则**：源中**显式设置的颜色按原值渲染**，平台不得再改 alpha。iOS 现有的 `withAlphaComponent`（`KeyTaoIOSKeyboardView.swift:7125,7131,7134`）只保留在派生默认值里，由 Rust 算好写进 resolved JSON。

### B.7 按键样式与选择器

`KeyStyle` 字段（都可选）：

| 字段 | 类型 |
|---|---|
| `background`、`foreground`、`hintForeground` | color |
| `pressedBackground`、`pressedForeground` | color |
| `borderColor` | color |
| `borderWidth` | 0–2 |
| `cornerRadius` | 0–24 |
| `shadowColor` | color |
| `shadowOffsetY` | 0–4 |
| `fontScale`、`hintFontScale` | 0.8–1.3 |
| `fontWeight` | 9 档字重 |

**叠加顺序（低 → 高）**：`key` → `keyTypes.<type>` → `keyClasses.<name>` → `keyStates.<state>`。高层只覆盖自己设置了的字段。每层都可以在 `light:` / `dark:` 中覆盖。

**按键类型**：按**当前模式下生效的 action** 决定（与 `actionForMode` 一致，`KeytaoKeyboardView.kt:6999`；字段来源 `action`/`ascii_action`，`mobile_layout.rs:67-70,81`）。`MobileCommand.type` 是自由字符串（`mobile_layout.rs:132-139`），映射：
- `function`：`mode`、`keyboardMode`、`space`、`enter`、`backspace`（即现行柔和强调集合，`KeytaoKeyboardView.kt:7000-7011`，`KeyTaoIOSKeyboardView.swift:6816-6828`）
- `space`、`enter`、`backspace`：各自类型；未设字段继承 `function`
- `shift`：未设字段继承 `character`
- `character`：其余所有类型，包括 `input`、`directInput`、无 action、`openPage`、`floating`、`reset`、`nextInputMethod` 和任何未知类型

**内置键类（v1 只允许这两个类名，其他类名忽略并报告）**：
- `accent`：键的 `style == "accent"`（`mobile_layout.rs:77`）。
- `accentPunctuation`：当前模式下的 label 或 value 属于 `，。,.`。

现行代码里 `style=="accent"` 走 `selectedBackground` 的分支（`KeytaoKeyboardView.kt:6978`，`KeyTaoIOSKeyboardView.swift:7130-7131`）在柔和强调分支之后，是死代码。P0 删除它，以柔和混色为准（即现在用户实际看到的效果）。

**状态**：
- `keyStates.selected`：键处于选中态时。
- `keyStates.shiftOn`：**只作用于 shift 键本身**。
- 按下态只用 `pressedBackground`/`pressedForeground`，没有 `keyStates.pressed`。

### B.8 颜色、尺寸、字体

- **颜色**：`#RGB`、`#RRGGBB`、`#RRGGBBAA`（RGBA 顺序，`crates/keytao-theme/default-theme.yaml:27`）或 `transparent`。大小写不敏感，导出统一大写 8 位。
- **尺寸**：逻辑像素（Apple pt、Android dp、Windows/Linux DIP）。
- **字体族**：规范形式是逗号分隔字符串，例如 `"PingFang SC, Microsoft YaHei UI"`。解析时也接受字符串数组，拼接成逗号分隔字符串。resolved JSON 永远输出字符串。最多 8 项，每项最多 64 字符。
- **字重**：9 档全部保留，平台不支持时向下取最近一档。

### B.9 默认值单一来源

`crates/keytao-theme/default-theme.yaml` 是唯一真源，Rust 已嵌入并总是先应用它（`lib.rs:39,792`）。
- Android `KeytaoImeTheme.fallback()`（`KeytaoTheme.kt:95`）和 iOS `fallback`（`KeyTaoIOSTheme.swift:158`）改为调用一次 Rust resolve（路径全传 nil；Android 走 `KeytaoNativeBridge.resolveThemeJson`，`KeytaoNativeBridge.kt:37`；iOS 走 `keytao_resolve_theme_json_with_system_scheme`，`KeyTaoIOSTheme.swift:274`）。
- 只在 FFI 失败时用硬编码常量。这些常量由脚本从 YAML 生成，并有一个一致性测试比对生成结果与 YAML。
- 漂移修正：注释色、accent（`#4A8DF6`，替换回退里的 `#3B73D9`）以 YAML 为准。

### B.10 解析、未知字段与版本

**导入解析**：先解析成 `serde_json::Value`，逐字段校验类型与取值，合法的字段组装成 Partial，非法字段丢弃并写入导入报告。不直接用 `Deserialize` 结构体，因为现有结构体任一字段类型错误都会让整份文档失败（`lib.rs:798`）。
必须有一个测试：同一主题里一个坏颜色加一个好颜色，好颜色保留。

| 情况 | 处理 |
|---|---|
| 未知字段 | 忽略并报告；源文件落盘时原样保留 |
| `version` 等于 3 | 正常解析 |
| `version` 大于 3 | 拒绝导入，提示更新 App，不保存 |
| `version` 小于等于 2 或缺失 | 拒绝导入（v2 只来自本机 theme.yaml，走 B.4 迁移） |
| 字段类型错误、颜色不合法 | 丢弃该字段并报告。fail closed：非法值从不当作“不限制” |

### B.11 校验上限

- 数值：按 `sanitized()` 的范围 clamp 并报告（A 表）。
- 字符串：`meta.name`、`meta.author` 最多 64 个字素簇；`labelSuffix`、`chineseText`、`englishText` 最多 4 个字素簇。
- 结构：canonical JSON 最多 64 KiB，嵌套深度最多 8。
- 可读性：前景与背景对比度低于 3:1 时只警告。按键底色 alpha 为 0 允许（命中区域由几何决定）。

### B.12 安全与 id

主题是不可信输入。
- 不执行脚本或表达式，不做网络访问。
- v1 没有路径类字段。v2 的包内资源路径拒绝 URL scheme（`http:`、`https:`、`file:`、`data:`）、`..` 和绝对路径；这条规则**只作用于路径类字段**。
- `meta.name`、`meta.author`：去掉控制字符和 bidi 覆盖字符（U+202A–U+202E、U+2066–U+2069），按纯文本渲染，不自动识别链接。
- `meta.id`：正则 `^[a-z0-9][a-z0-9_-]*(\.[a-z0-9_-]+)*$`，长度 3–64；并拒绝任一点分段等于 Windows 保留名（`con`、`prn`、`aux`、`nul`、`com1`–`com9`、`lpt1`–`lpt9`）。文件名直接用 id。
- 解压后上限 64 KiB，超出立即中止。
- 导入永远不写 theme.yaml 的 `ui:` 本机设置。

### B.13 JSON Schema 草图

Schema 只描述类型和结构，**不写数值上下限**（范围由 `sanitized()` clamp，严格校验器会把 clamp 变成拒绝），也不写 `additionalProperties: false`（未知字段只报告）。

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["version", "meta"],
  "properties": {
    "version": { "const": 3 },
    "meta": {
      "type": "object",
      "required": ["id", "name"],
      "properties": {
        "id": { "type": "string", "pattern": "^[a-z0-9][a-z0-9_-]*(\\.[a-z0-9_-]+)*$" },
        "name": { "type": "string" },
        "author": { "type": "string" },
        "appearance": { "enum": ["both", "light", "dark"] }
      }
    },
    "common": { "$ref": "#/$defs/common" },
    "pc": { "type": "object", "description": "font/panel/candidate/navigation/modeHint, fields as table A.2" },
    "mobile": { "$ref": "#/$defs/mobile" },
    "light": { "$ref": "#/$defs/variant" },
    "dark": { "$ref": "#/$defs/variant" }
  },
  "$defs": {
    "color": { "type": "string", "pattern": "^(transparent|#([0-9A-Fa-f]{3}|[0-9A-Fa-f]{6}|[0-9A-Fa-f]{8}))$" },
    "weight": { "enum": ["ultralight", "thin", "light", "regular", "medium", "semibold", "bold", "heavy", "black"] },
    "common": {
      "type": "object",
      "properties": {
        "accentColor": { "$ref": "#/$defs/color" },
        "backgroundColor": { "$ref": "#/$defs/color" },
        "cornerRadius": { "type": "number" },
        "font": { "type": "object", "properties": {
          "family": { "oneOf": [ { "type": "string" }, { "type": "array", "items": { "type": "string" } } ] },
          "weight": { "$ref": "#/$defs/weight" } } }
      }
    },
    "keyStyle": {
      "type": "object",
      "properties": {
        "background": { "$ref": "#/$defs/color" },
        "foreground": { "$ref": "#/$defs/color" },
        "hintForeground": { "$ref": "#/$defs/color" },
        "pressedBackground": { "$ref": "#/$defs/color" },
        "pressedForeground": { "$ref": "#/$defs/color" },
        "borderColor": { "$ref": "#/$defs/color" },
        "borderWidth": { "type": "number" },
        "cornerRadius": { "type": "number" },
        "shadowColor": { "$ref": "#/$defs/color" },
        "shadowOffsetY": { "type": "number" },
        "fontScale": { "type": "number" },
        "hintFontScale": { "type": "number" },
        "fontWeight": { "$ref": "#/$defs/weight" }
      }
    },
    "mobile": {
      "type": "object",
      "properties": {
        "keyboard": {
          "type": "object",
          "properties": {
            "background": { "$ref": "#/$defs/color" },
            "pageBackground": { "$ref": "#/$defs/color" },
            "key": { "allOf": [ { "$ref": "#/$defs/keyStyle" }, { "properties": {
              "swipeHintForeground": { "$ref": "#/$defs/color" },
              "swipeHintVisible": { "type": "boolean" } } } ] },
            "keyTypes": { "type": "object",
              "propertyNames": { "enum": ["character", "function", "space", "enter", "backspace", "shift"] },
              "additionalProperties": { "$ref": "#/$defs/keyStyle" } },
            "keyClasses": { "type": "object",
              "propertyNames": { "enum": ["accent", "accentPunctuation"] },
              "additionalProperties": { "$ref": "#/$defs/keyStyle" } },
            "keyStates": { "type": "object",
              "propertyNames": { "enum": ["selected", "shiftOn"] },
              "additionalProperties": { "$ref": "#/$defs/keyStyle" } },
            "toolbar": { "type": "object" },
            "floating": { "type": "object" }
          }
        },
        "candidateBar": { "type": "object" }
      }
    },
    "variant": {
      "type": "object",
      "properties": { "common": { "$ref": "#/$defs/common" }, "pc": { "type": "object" }, "mobile": { "$ref": "#/$defs/mobile" } }
    }
  }
}
```

### B.14 完整示例

```yaml
version: 3
meta:
  id: rea.mint-glass
  name: 薄荷玻璃
  author: rea
  appearance: both

common:
  accentColor: "#2BB38A"
  cornerRadius: 14
  font:
    family: "PingFang SC, Microsoft YaHei UI, Noto Sans CJK SC"
    weight: medium

pc:
  font: { size: 19, labelSize: 14, commentSize: 15, preeditSize: 14 }
  panel:
    orientation: horizontal
    background: "#F4FBF8F0"
    borderColor: "#B5D8CB"
    borderWidth: 1
    paddingX: 10
    paddingY: 8
    gap: 6
    shadow: true
  candidate:
    foreground: "#1F3A31"
    selectedForeground: "#0E2A21"
    selectedBackground: "#D3F1E6"
    labelColor: "#7FA597"
    commentColor: "#4B6B60"
    labelSuffix: "."
  modeHint:
    background: "#1F5C4A"
    foreground: "#FFFFFF"
    chineseText: 中
    englishText: A

mobile:
  keyboard:
    background: "#E7F3EE"
    key:
      background: "#FFFFFFE0"
      foreground: "#1F3A31"
      hintForeground: "#7FA597"
      borderColor: "#00000014"
      borderWidth: 1
      cornerRadius: 9
      shadowColor: "#0000001F"
      shadowOffsetY: 1
    keyTypes:
      function: { background: "#CFE7DD" }
      enter: { background: "#2BB38A", foreground: "#FFFFFF" }
    keyClasses:
      accent: { background: "#2BB38A", foreground: "#FFFFFF" }
    keyStates:
      shiftOn: { background: "#2BB38A", foreground: "#FFFFFF" }
    toolbar: { foreground: "#4B6B60", selectedForeground: "#2BB38A" }
  candidateBar:
    background: transparent
    showComment: true

dark:
  pc:
    panel: { background: "#13201CF0", borderColor: "#2E4A40" }
    candidate:
      foreground: "#E3F2EC"
      selectedForeground: "#FFFFFF"
      selectedBackground: "#1F5C4A"
      labelColor: "#7FA597"
      commentColor: "#A9C7BC"
  mobile:
    keyboard:
      background: "#101A17"
      key: { background: "#24352FCC", foreground: "#E3F2EC", borderColor: "#FFFFFF14" }
      keyTypes:
        function: { background: "#1C2B26" }
```

---

## C. 分享与导入

### C.1 分享串

```
keytao-theme:3:<base64url_nopad(zlib(canonical_json(source)))>
```

- `3` 是 `THEME_SOURCE_VERSION`，不解压就能判断版本。
- 用 zlib（带 adler32 校验和），截断或篡改会在解压时报错。编解码在 Dart 侧用 `dart:io` 的 `ZLibCodec`，不新增依赖；校验与 canonical 化在 Rust 侧。
- **导出的是主题源 partial，不是解析后的完整结果**，否则 accent 等派生值会被固化。
- canonical JSON：
  - 按 RFC 8785（JCS）序列化：键按 UTF-16 码元排序，数字按 JCS 规则输出（`20` 与 `20.0` 相同）。
  - 所有字符串做 Unicode NFC 归一化。
  - 颜色统一大写 8 位；`font.family` 统一为逗号分隔字符串。
  - 等于默认值的字段保留。
- **粘贴解码前清洗**：去掉所有空白、零宽字符（U+200B–U+200D、U+FEFF）和 BOM。
- **长度**：“典型主题 1–3 KB”和 QQ、微信的单条长度上限尚未实测，P1 开发时先在两端实测记录。分享串超过 4000 字符时，界面引导改用文件分享。

**粘贴导入只接受两种文本**：分享串，或以 `{` 开头的 JSON（`serde_json`）。不接受 YAML 粘贴（聊天软件会把缩进换成 NBSP、把标点换成全角，且 serde_yaml 0.9 无法关闭锚点和别名，`crates/keytao-core/Cargo.toml:9`）。YAML 只用于本机源文件。

**剪贴板**：只在用户点击“粘贴导入”时读取，启动时不探测，避免 iOS 粘贴授权弹窗和 Android 13 剪贴板提示。

**文件**：导出 `<id>.keytao-theme.json`，内容为 canonical JSON，通过系统分享面板发送。

### C.2 上限

| 检查 | 规则 |
|---|---|
| 前缀版本号 | 大于 3 拒绝 |
| 分享串长度 | 清洗后不超过 96 KiB，超出不解码 |
| 解压后大小 | 不超过 64 KiB，超出立即中止 |
| 内容校验 | B.10、B.11、B.12 |
| 导入报告 | 丢弃的字段、clamp 的值、对比度警告 |

### C.3 冲突规则

判重用 canonical JSON 的 sha256。

| 情况 | 处理 |
|---|---|
| id 不存在 | 直接安装 |
| id 相同且哈希相同 | 提示“已安装”，不写入 |
| id 相同但内容不同 | **另存为副本**（默认）或覆盖。副本 id 加 `-2`、`-3` 后缀，名称加“（副本）” |
| id 与内置主题（`keytao.*`）相同 | 只能另存为副本 |
| 覆盖的是当前主题 | 立即重新编译 theme.yaml |

### C.4 分享时排除的内容

theme.yaml 的 `ui:` 本机设置、`themeId`/`themeHash`、keyboard.yaml 的布局与几何、本机路径。

---

## D. App 编辑体验（只列大纲）

### D.1 控件与字段

| 区块 | 控件 | 字段 |
|---|---|---|
| 主题库 | 卡片网格：内置 + 已安装；长按复制、导出、删除 | `themes/*.yaml`、`themeId` |
| 顶部操作 | 粘贴导入、复制分享串、导出文件 | C.1、C.3 |
| 外观 | 分段控件：自动 / 浅色 / 深色 | theme.yaml `ui.colorScheme`（本机） |
| 编辑目标 | 分段控件：编辑浅色或深色，决定写 `light:` 还是 `dark:` | B.5 |
| 通用 | 现有 `AccentPicker`、`BackgroundPicker`（`flutter_app/lib/src/ui/color_picker.dart:19,195`）、圆角滑块、字体族输入、字重分段 | `common.*` |
| PC 面板 | 方向、字号（现有）；“高级”折叠区：背景、边框、内边距、间距、阴影、各文字色 | `pc.*` |
| 键盘 | 键盘背景；按键、功能键、空格、回车、强调键的底色和字色；圆角、边框、阴影；字号缩放 | `mobile.keyboard.*` |
| 候选栏 | 字号缩放、显示注释、栏背景 | `mobile.candidateBar.*` |

**平台能力**：控件按当前平台显示“本平台不支持”灰态。数据来自 Rust 单一能力表，替代 `UiCapabilities`（`lib.rs:459-487`）。同时修正 Windows 声明 `full_custom()`（`crates/keytao-windows-ime/src/panel.rs:645`）、却没渲染 hover 和 shadow 的问题。Linux kimpanel 路径整页灰态。

**实时预览**：`candidate_panel_preview.dart`、`mobile_keyboard_preview.dart` 读 Rust 解析后的 JSON，不再自行派生。每项提供“恢复默认”（删除该字段）。

**编辑内置主题**：第一次改动时自动 fork 为 `my.<base>-N`。

### D.2 分期

| 期 | 内容 |
|---|---|
| P0 | v3 源解析（Value 逐字段校验）；`compile_theme`；B.4 迁移与 golden 测试；写回函数改造；派生收敛到 Rust，resolved JSON 追加 `mobile`；移动端读 `mobile` 并删除 `style=="accent"` 死代码；回退改调 Rust（B.9） |
| P1 | 主题库、分享串、粘贴导入、导入报告、真源冲突提示；键盘和候选栏 P0/P1 控件；能力表；分享串长度实测 |
| P2 | 分隔线、按下态细项等 A 表 P2 项 |
| v2 | zip 包、背景图（解码后长边不超过 2048 px，按屏幕尺寸降采样）、内嵌字体、单键覆盖 `keyOverrides`、自定义键类、`keyStates.capsLock`、从 Squirrel/Weasel/Trime/fcitx5 导入 |

---

## 附录：评审意见处理

30 条全部采纳，以下为部分采纳或改写的条目：

| # | 处理 | 理由 |
|---|---|---|
| 1 | 部分采纳：不加 `specVersion`，源文件用 `version: 3`；但 `THEME_SCHEMA_VERSION` 保持 2，新增 `THEME_SOURCE_VERSION = 3` | 采纳第 2 条后 theme.yaml 和 resolved JSON 仍是 v2 结构，把它们标成 3 会误导 |
| 5 | 采纳两个方案的组合：迁移时用旧解析器结果补显式字段，同时保留 backgroundColor 的对比度派生 | 只补字段能保证迁移等价，派生规则让新建主题也能用“只设背景色” |
| 12 | 选择删除死代码，以柔和混色为准 | 这是用户现在实际看到的效果 |
| 15 | 柔和强调的基色取 `pc.panel.background`，Flutter 预览对齐它 | Android/iOS IME 是实际渲染方，预览应跟随 |
| 18 | `keyOverrides` 部分随第 30 条移到 v2 | — |
| 22 | 采纳去掉 `shadowRadius`，实心偏移阴影两端都做 | 实心阴影只是多画一个偏移圆角矩形，开销可忽略 |
| 25 | `themeHash` 用源 canonical JSON 的 sha256 前 16 位 | 足够判别手改 |
| 26 | 选 zlib 而不是 CRC32 前缀 | Dart `dart:io` 自带，少一段格式 |
| 29 | 文件名直接用 id，靠正则和保留名拒绝保证安全 | 比哈希文件名更便于用户手动管理 |

### 未采纳的意见

无。

## 已定决策（2026-11-19）

1. 柔和强调色的基色定为 `pc.panel.background`，与 Android/iOS IME 一致；Flutter 预览改为跟随。
2. 主题文件名直接用 id（`themes/<id>.yaml`），靠 id 正则与 Windows 保留名拒绝保证安全。
3. 检测到 `theme.yaml` 被手改时，默认按钮为「另存为新主题」。
4. 实心按键阴影为 P1，Android 与 iOS 两端都做。
5. 分享串超过 4000 字符时引导改用文件分享（待 QQ/微信实测后调整）。
6. 各平台实现以视觉一致为目标：所有默认值与派生只在 Rust 计算一次，平台只按 resolved 结果渲染，不再自行改色或改 alpha；用同一份主题在各平台渲染对比验收。
