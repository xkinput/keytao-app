# keytao-theme

`keytao-theme` 是 KeyTao 输入法前端共享的主题语言和 UI model 层。它不绘制任何窗口，也不直接操作 librime；它只负责把 `theme.yaml` 解析成跨平台一致的 `ResolvedImeTheme`，再把输入法状态规整成候选窗和模式提示可以消费的 model。

## 边界

```text
theme.yaml
  -> ResolvedImeTheme

ImeState-like input + platform capabilities
  -> CandidatePanelModel / ModeHintModel

ResolvedImeTheme + Model
  -> macOS AppKit / Linux SHM-X11 / Windows layered window / system lookup table
```

共享的是主题语义和 model，不共享平台绘制实现。AppKit、Wayland/X11、Windows TSF、IBus/GNOME 的生命周期和可控视觉能力不同，各平台 renderer 只能消费同一份语言并按能力降级。

## 主题位置

内置默认主题在本 crate 的 `default-theme.yaml`。用户覆盖主题放在 KeyTao 用户数据目录：

| 平台 | 用户主题路径 |
| --- | --- |
| macOS | `~/Library/keytao/theme.yaml` |
| Linux | `~/.local/share/keytao/theme.yaml` |
| Windows | `%APPDATA%/keytao/theme.yaml` |

开发调试可以设置：

```sh
KEYTAO_IME_THEME_PATH=/path/to/theme.yaml
```

## 示例

用户主题只需要写要覆盖的字段：

```yaml
version: 1

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

## 平台接入

Rust 平台前端使用 `ThemeResolver`：

```rust
use keytao_theme::{ThemeResolver, UiCapabilities};

let resolver = ThemeResolver::from_default_locations();
let theme = resolver.current();
let model = theme.candidate_panel_model(input, &UiCapabilities::full_custom());
```

Swift/macOS 通过 `keytao-core-ffi` 获取 normalized JSON，不直接解析 YAML。这样 YAML schema、默认值、校验和 fallback 规则只存在一份。

## 降级规则

- 自绘平台：macOS AppKit panel、Linux Wayland/X11/KDE/IBus fallback overlay、Windows layered candidate/mode hint window 可以完整消费颜色、圆角、padding、字号、横竖排和模式提示。
- 系统候选服务：IBus/GNOME/Kimpanel 只能消费候选 label、文本、comment、highlight、page 等结构；视觉由桌面环境决定。
- 主题不能控制 Rime session、候选选择逻辑、按键处理、候选数量或平台窗口定位策略。
