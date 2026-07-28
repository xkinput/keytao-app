# keytao-theme

`keytao-theme` 是 KeyTao 输入法前端共享的主题语言和 UI model 层。它不绘制任何窗口，也不直接操作 librime；它只负责把 `theme.yaml` 解析成跨平台一致的 `ResolvedImeTheme`，再把输入法状态规整成候选窗和模式提示可以消费的 model。移动端软键盘布局不属于共享主题，由本 crate 的 `mobile_layout` 模块从独立的 `keyboard.yaml` 解析。

## 边界

```text
theme.yaml
  -> auto/light/dark UI selection + accent color
  -> ResolvedImeTheme

ImeState-like input + platform capabilities
  -> CandidatePanelModel / ModeHintModel

ResolvedImeTheme + Model
  -> macOS AppKit / Linux SHM-X11 / Windows layered window / system lookup table

keyboard.yaml
  -> mobile_layout::MobileLayout
  -> Android / iOS 软键盘 adapter
```

共享的是主题语义和 model，不共享平台绘制实现。AppKit、Wayland/X11、Windows TSF、IBus/GNOME 的生命周期和可控视觉能力不同，各平台 renderer 只能消费同一份语言并按能力降级。

## theme.yaml 与 keyboard.yaml 的职责

| 文档 | 解析入口 | 产出模型 | 消费方 |
| --- | --- | --- | --- |
| `theme.yaml` | `resolve_theme_from_paths` / `ThemeResolver` | `ResolvedImeTheme`、`CandidatePanelModel`、`ModeHintModel` | 五个平台的候选窗与模式提示 |
| `keyboard.yaml` | `mobile_layout::resolve_mobile_layout_from_paths` | `mobile_layout::MobileLayout` | 仅 Android / iOS 软键盘 |

- `theme.yaml` 只表达跨平台可落地的视觉语义：配色方案、强调色、字体、面板、候选、模式提示。它**不再**携带键盘布局，`ResolvedImeTheme` 与其 JSON 里都没有 `keyboard` 字段，桌面平台拿到的 theme JSON 不再夹带移动端数据。
- `keyboard.yaml` 表达软键盘的行/层/权重与按键命令（`action`、`asciiAction`、`swipeUp`、`swipeDown`、`longPress`、`stack`、`keyboardMode` 跳转）。这些语义只有自绘软键盘的平台能落地，因此不进入共享层。
- 兼容性：旧版 `theme.yaml` 里的 `keyboard:` 段仍然可以被解析，把该文件路径传给 `resolve_mobile_layout_from_paths` 即可（它接受 `keyboard.yaml` 文档，也接受带 `keyboard:` 段的 `theme.yaml`）。但 `light:` / `dark:` 变体不再能覆盖键盘布局。
- crate 根部保留了 `KeyboardTheme`、`resolve_keyboard_from_paths`、`resolved_keyboard_json`、`default_keyboard_yaml` 等旧名字作为别名，指向 `mobile_layout` 中的新类型与函数；新代码请直接用 `mobile_layout`。

## 主题位置

内置默认主题在本 crate 的 `default-theme.yaml`。桌面发行包也会随包放置同一份 `default-theme.yaml`，平台 renderer 优先读取随包文件，找不到时使用编译进 crate 的内置版本。用户覆盖主题放在 KeyTao 用户数据目录：

| 平台 | 用户主题路径 |
| --- | --- |
| macOS | `~/Library/keytao/theme.yaml` |
| Linux | `~/.local/share/keytao/theme.yaml` |
| Windows | `%APPDATA%/keytao/theme.yaml` |

开发调试可以设置：

```sh
KEYTAO_IME_THEME_PATH=/path/to/theme.yaml
```

调试随包默认主题查找可以设置：

```sh
KEYTAO_IME_DEFAULT_THEME_PATH=/path/to/default-theme.yaml
```

## 示例

用户主题只需要写要覆盖的字段：

```yaml
version: 2

ui:
  colorScheme: auto
  accentColor: "#3B73D9"

panel:
  orientation: vertical
  background: "#101820F0"
  cornerRadius: 16
  maxWidth: 320

font:
  size: 19
  labelSize: 14

candidate:
  selectedBackground: "#DCEBFF"
  foreground: "#F8FAF7"
  selectedForeground: "#FFFFFF"
  separatorVisible: true

modeHint:
  background: "#E6F0FFF2"
  foreground: "#2F5FB8"
  duration: 0.75
```

`ui.colorScheme` 支持：

| 值 | 效果 |
| --- | --- |
| `auto` | 跟随系统明暗主题 |
| `light` | 使用明亮 UI 配置 |
| `dark` | 使用夜间 UI 配置 |

`ui.accentColor` 可写 `#RRGGBB` 或 `#RRGGBBAA`，解析后会派生候选高亮、hover 和模式提示的强调色。

主题可以提供模式变体，根级字段作为通用配置，`light:` 和 `dark:` 下的字段只在对应实际模式生效：

```yaml
ui:
  colorScheme: auto
  accentColor: "#46A0FF"

dark:
  panel:
    background: "#171A20F2"
  candidate:
    selectedBackground: "#2D4B63"
    foreground: "#EEF3F7"
```

## 平台接入

Rust 平台前端使用 `ThemeResolver`：

```rust
use keytao_theme::{ThemeResolver, UiCapabilities};

let resolver = ThemeResolver::from_default_locations();
let theme = resolver.current();
let model = theme.candidate_panel_model(input, &UiCapabilities::full_custom());
```

Swift/macOS 通过 `keytao-core-ffi` 获取 normalized JSON，不直接解析 YAML。这样 YAML schema、默认值、校验和 fallback 规则只存在一份。

移动端软键盘布局走独立入口：

```rust
use keytao_theme::mobile_layout;

let layout = mobile_layout::resolve_mobile_layout_from_paths(None, Some(&user_keyboard_yaml));
let json = mobile_layout::mobile_layout_json(&layout)?;
```

Android/iOS 通过 `keytao_resolve_keyboard_json`（JNI 侧为 `nativeResolveKeyboardJson`）拿到同一份 JSON，不要再从 theme JSON 里取 `keyboard` 字段。

## 降级规则

- 自绘平台：macOS AppKit panel、Linux Wayland/X11/KDE/IBus fallback overlay、Windows layered candidate/mode hint window 可以完整消费颜色、圆角、padding、字号、横竖排和模式提示。
- 系统候选服务：IBus/GNOME/Kimpanel 只能消费候选 label、文本、comment、highlight、page 等结构；视觉由桌面环境决定。
- 主题不能控制 Rime session、候选选择逻辑、按键处理、候选数量或平台窗口定位策略。
