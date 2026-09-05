# KeyTao 移动端输入体验改进方案（对标 GBoard）

> 适用范围：Android 自绘键盘（`src-tauri/gen/android/app/src/main/java/ink/rea/keytao_app/`）、iOS 键盘扩展（`crates/keytao-ios-ime/Sources/KeyTaoIOSIME/`）、共享布局配置（`crates/keytao-theme/default-keyboard.yaml`）、设置前端（`src/App.tsx`）。
> 文中 `file:line` 均以仓库根 `/Users/rea/code/keytao-org/keytao-app/` 为基准。

## 目标

把 KeyTao 的日常打字体验拉到 GBoard 同级：手指按下有即时的视觉/触觉/听觉确认，长按能拿到一排备选而不是唯一一个字符，退格与光标可以用手势连续操作而不必切层，多指连打不丢字，候选与工具栏在输入过程中始终可达，并且这些行为的时序与开关对用户可调。方案只覆盖**形码中文输入法真正用得上的那部分 GBoard 体验**——滑行输入、英文自动纠错这类与键道方案冲突或价值极低的能力被显式排除（见「明确不做」），其余每一条都拆成一个边界清晰、可单独实现、可在真机上验收的条目，供 Codex 分批落地。

## 现状小结

评级：**solid** = 已达或超过 GBoard；**weak** = 有实现但明显落后；**missing** = 完全没有；**broken** = 有代码但实际失效。

| 领域 | Android | iOS | 说明 |
| --- | --- | --- | --- |
| 按键预览气泡 | missing | missing | 两端都只有键面下移 1dp/1pt + 换底色 |
| 按下高亮 | weak | weak | 同时只亮一个键；pressed 与 selected 共用同一色 token |
| 触感反馈 | solid（时机偏晚） | weak | Android 振幅/API33 属性完备；iOS 只有一种 `.light`，且仅抬手才响 |
| 按键音 | weak | partial（平台封顶） | Android 无 App 级开关/音量；iOS 只能用 `playInputClick()` |
| 长按动作 | weak | weak | 只产出单一命令，无备选面板；420ms 硬编码 |
| 长按重复（退格） | weak | weak | 恒速 72ms，无起始静默期、无加速、无按词删 |
| 退格滑动手势 | solid | solid | 横拖逐字删/恢复、纵滑清空/全恢复，强于 GBoard |
| 上下滑输入 | solid | solid | 缺滑动中的视觉预告 |
| 空格滑动移光标 | missing | missing | 配置层无 `swipeLeft/Right` 字段 |
| 滑动改键容错 | missing | missing | 滑到相邻键释放 = 什么都不出 |
| 多点触控连打 | weak | **broken** | iOS 三个回调都只取 `touches.first`，稳定丢字 |
| 候选栏 | weak（横滚是死代码） | weak（超宽直接丢弃） | 无横滑、无翻页、无长按、字号封顶 |
| 打字时工具栏 | broken（整条消失） | broken（整条消失） | 有候选即不可达中英/剪贴板/Emoji |
| Shift 三态 | weak | weak | ONCE 与 LOCKED 同色，仅字形差异 |
| 数字行 | missing | missing | 仅键道 `=` 引导时临时替换首行 |
| 工具栏可定制 | weak | weak | 固定 6–7 项，不可滚动/排序，长按通道是死代码 |
| 用户可见设置 | weak | weak | 仅震动/回车/浮动；高度、长按时长、阈值只能改 yaml |
| 面板滚动 | weak | weak | 无惯性、无回弹、无滚动条 |
| 无障碍 | 中 | weak | iOS 键元素无 activation 回调，VoiceOver 打不出字 |

---

# P1 打字手感核心

日常打字每分钟都会碰到的项，且当前实现可直接改造。**第一批全部落这一层。**

### P1-1 iOS 多点触控丢字（BUG，最高优先级）

- **平台**：iOS
- **现状**：`setup()` 设了 `isMultipleTouchEnabled = true`（KeyTaoIOSKeyboardView.swift:586），但 `touchesBegan/Moved/Ended` 三个回调都只取 `touches.first`（:397、:449、:489），状态机只有单个 `pressedKey`/`touchStart`/`longPressWorkItem`（:148、:155-160）。第二指落下会覆盖第一指的 `pressedKey`，第一指抬起时 `rect.contains(point)` 判假 → **该字直接丢失**，并且 `stopLongPressAndRepeat` 会打断新键的长按计时。键道本身鼓励高频滚指连击，这个缺陷是稳定复现的。
- **GBoard 行为**：完整 rollover——先按下的键在后一键按下后仍能独立提交，任何两指顺序组合都不丢字。
- **改法**：把 `pressedKey / touchStart / currentTouchPoint / backspaceGestureUnits / longPressWorkItem / pressedStackIndex` 收进以 `ObjectIdentifier(UITouch)` 为 key 的字典（`activeTouches`），三个回调改为 `for touch in touches` 遍历；面板滚动、退格拖拽、符号层纵向滚动仍限定为「第一个进入该区域的 touch」，其余 touch 在该手势期间忽略键位命中。绘制层的按下高亮改为遍历字典（与 P1-13 的多键高亮同批做）。
- **验收标准**：单元测试模拟 `A down → B down → A up → B up`，断言产出两个字符且顺序为 A、B；真机用两指交替快速敲 20 次，输入框字符数恰为 20、顺序无误；退格横拖回归用例（左拖 3 格删 3 字、右拖回 2 格恢复 2 字）仍通过。

### P1-2 Android 多指长按退化与主指移交缺陷（BUG）

- **平台**：Android
- **现状**：三处退化。(a) 非主指强制 `allowLongPress = false`，`scheduleLongPress` 对非 primary 直接 return（KeytaoKeyboardView.kt:3138-3147、:3295-3303），按住一键再长按另一键取数字/符号完全无效；(b) `finishKeyTouch` 在主指抬起时把 `primaryKeyPointerId` 移交给剩余 touch，但**不重新 `scheduleLongPress`**（:3167-3186），于是「按住 A → 按下抬起 B → 继续按住 A」之后 A 永久失去长按/连删；(c) 首指若落在候选栏/工具栏/展开面板上，`ACTION_POINTER_DOWN` 立即 return（:719-740），此后所有手指被吞到全部抬起，`ACTION_MOVE` 里 `updateKeyTouchMove` 排在最后一个分支也拿不到事件（:741-794）。
- **GBoard 行为**：每根手指独立持有自己的长按计时器与目标键；候选栏上的手指不影响键盘区其它手指。
- **改法**：把长按计时器从「单个 primary」改为 per-pointer（`Map<pointerId, Runnable>`），`scheduleLongPress` 去掉 primary 限制；`finishKeyTouch` 移交主指时为新主指重新挂计时器（或直接取消 primary 概念，只有退格连删这类独占手势保留单指锁）；`ACTION_POINTER_DOWN` 命中非键盘区时只标记该 pointer 为「被面板占用」，不 return 吞掉后续 pointer。
- **验收标准**：按住 `q` 不放，同时长按 `w` 能出 `2`；按住 `q` → 点一下 `w` → 继续按住 `q` 满 300ms 仍触发长按；首指停在候选栏上时，第二指在字母区打字正常出字。

### P1-3 iOS 长按 123 键切换输入法无效（BUG，低成本）

- **平台**：iOS
- **现状**：`default-keyboard.yaml:64` 与 `:164` 给 123 键配了 `longPress: { type: nextInputMethod }`，但 iOS 的 `KeyTaoCommandType`（KeyTaoIOSConfig.swift:5-26）没有 `nextInputMethod` 分支，控制器 `didTrigger` 的 switch 落到 `default: break`（KeyTaoKeyboardViewController.swift:300-305），长按静默无反应。
- **GBoard 行为**：地球键/长按均可切换输入法或弹出系统键盘选择器。
- **改法**：在控制器 switch 里补 `case .nextInputMethod: advanceToNextInputMode()`，并保留既有 `keyboardPicker` 路径（覆盖在地球键上的透明 UIButton 绑 `handleInputModeList(from:with:)`，KeyTaoKeyboardViewController.swift:104-116）——展开面板/自定义布局时才退化为 `advanceToNextInputMode`。iOS 侧 `KeyTaoCommandType` 需新增该枚举值以免解析时被丢弃。
- **验收标准**：iOS 真机长按 123 键切到下一个输入法（或弹出键盘选择器），Android 行为不变。

### P1-4 按键预览气泡（key preview popup）

- **平台**：both
- **现状**：两端均无任何 popup/preview 实现。Android 按下表现只有矩形下移 1dp + `keySelectedBackground` + 阴影 1.6→0.8dp（KeytaoKeyboardView.kt:2172-2212、:3339-3350）；iOS 同理（KeyTaoIOSKeyboardView.swift:1305-1349、:1321-1329）。手指本身会遮住键面，1dp 位移在盲打时基本不可见。
- **GBoard 行为**：字符键按下时在指尖上方弹出放大字符气泡；是独立设置项「Popup on keypress」，AOSP 血统默认值 `config_default_key_preview_popup = true`，且与「按下高亮」是两套独立反馈（关掉气泡仍有高亮）。来源：hardreset.info/devices/apps/apps-gboard/disable-popup-on-keypress/；HeliBoard `config-per-form-factor.xml`。
- **改法**：
  - 抑制规则先行（AOSP `key_styles_common.xml` 的 `noKeyPreview` 语义）：shift、退格、空格、回车、emoji、设置、语言切换、123/ABC 层切换键**一律不弹**；另外抑制候选栏、工具栏、编辑面板、展开面板、悬浮键盘拖拽/缩放期间。抑制条件建议由 `KeySpec` 派生（有 `action` 且非 `input/directInput` 的键 = 不弹），不新增 yaml 字段。
  - Android：在 `drawKeyboard` 之后追加一层气泡绘制（同一 Canvas，最后画即可覆盖），顶排键的气泡向候选栏区域溢出；不新增 Window/PopupWindow。
  - iOS：同样在 `draw(_:)` 末尾画，顶排需允许覆盖候选栏区域。
  - 气泡内容取「当前会落的字」——与 P1-5 的滑动/长按状态联动：上滑时气泡内容切成 `swipeUp` 目标，长按后交给 P1-5 的备选面板接管。
  - 新增运行时设置 `keyPreviewEnabled`（默认开），走 P2-4 的设置通道。
- **验收标准**：按住任一字母键，键上方出现放大字符气泡，手指移开/抬起即消失；按 shift/退格/空格/回车不出现气泡；上滑 `q` 过程中气泡内容从 `q` 变为 `1`；设置里关闭气泡后按下高亮仍在；单手/悬浮模式下气泡不超出键盘视图被裁切。

### P1-5 长按备选面板（more-keys）+ 按住拖选释放

- **平台**：both（yaml + 两端）
- **现状**：`resolveLongPressCommand` 只返回**一个** `KeyCommand`（asciiLongPress → longPress → 单字符 hint → 默认动作），触发即直接提交（KeytaoKeyboardView.kt:2268-2277；KeyTaoIOSKeyboardView.swift:2766-2777、:2900-2921）。`KeySpec` 没有多备选字段（KeytaoAndroidImeConfig.kt:67-84）。yaml 里 `longPress` 共 30 处，且都是一对一映射（default-keyboard.yaml:30-68、:164）。
- **GBoard 行为**：长按弹出备选行，`config_key_selection_by_dragging_finger = true` —— 手指不抬起直接拖到目标备选上松手即选中，全程一次触摸；`config_show_popup_keys_keyboard_at_touched_point` 决定面板贴触点还是对齐按键（手机默认对齐按键）。来源：HeliBoard `config-per-form-factor.xml`。
- **改法**：
  1. yaml/`KeySpec` 新增 `alternates: [ ... ]`（元素形如 `{ label, value }` 或复用现有 command 结构），三处同步：`default-keyboard.yaml`、`KeytaoAndroidImeConfig.kt`、`KeyTaoIOSConfig.swift`。缺省时用现有 `longPress`/`hint` 单项自动生成一个只有一项的 alternates，保证旧配置不回归。
  2. 长按触发时不再立即提交，改为进入 `alternatesActive` 状态并绘制备选行（面板对齐按键、超出屏幕边缘时贴边），高亮项随手指 X 位移变化；`ACTION_UP`/`touchesEnded` 时提交高亮项；面板外释放 = 取消。
  3. 首批数据只填最有价值的：`，` → `！ ？ ； ：`，`。` → `… 、 ~`，数字键角标 + 对应符号，英文层元音 `a/e/i/o/u/n/c` 的重音字母。
- **平台注意**：iOS 备选面板同样只能画在 `inputView` 内，顶排的面板需向候选栏方向展开。
- **验收标准**：长按 `，` 弹出一行 `！ ？ ； ：`，手指不抬起横向拖动时高亮跟随，松手落下被高亮项；拖出面板外松手不落任何字符；未配置 alternates 的键长按行为与改造前完全一致（回归）；两端表现一致。

### P1-6 长按延迟改为 300ms 且可调 100–700ms

- **平台**：both
- **现状**：Android `longPressDelayMs = 420L`（KeytaoKeyboardView.kt:3658），iOS `static let longPressDelayMs = 420`（KeyTaoIOSKeyboardView.swift:3444），均为 companion/static 常量，用户不可调。
- **GBoard 行为**：设置 > Preferences 最底部「Key long press delay」，默认 **300ms**，范围 **100–700ms**，定义为「按键从主功能切换到次功能所需时间」。来源：hardreset.info/devices/apps/apps-gboard/set-key-long-press-delay/；support.google.com/pixelphone/thread/207062765。
- **改法**：常量改为读运行时配置 `longPressDelayMs`（默认 300，夹取 100–700），Android 走 `KeytaoAndroidImeConfig.applyRuntimeSettings`（:374-410），iOS 走 `KeyTaoIOSRuntimeSettings`（KeyTaoIOSConfig.swift:744-751）；设置页滑杆在 P2-4 一并加。工具栏长按计时器（KeytaoKeyboardView.kt:695-697）复用同一常量。
- **验收标准**：默认安装后长按出字明显快于当前版本；设置为 100ms 与 700ms 时手感差异可感知且不误触发；改值后无需重启键盘即生效（`viewWillAppear`/`applyRuntimeSettings` 链路已通）。

### P1-7 退格重复：400ms 起始静默 + 50ms 间隔 + 按词删升级

- **平台**：both
- **现状**：Android `startRepeatingKey` 首次删除后直接 `postDelayed(repeatRunnable, 72ms)` 恒速重复（KeytaoKeyboardView.kt:3285-3293、:3659）；iOS 同为首延迟 = 长按延迟、之后固定 72ms（KeyTaoIOSKeyboardView.swift:2940-2952、:3444-3459）。既无起始静默期，也无加速；iOS 连删期间不发 click/haptic，完全无声无感。
- **GBoard 行为**：AOSP `config_key_repeat_start_timeout = 400ms`、`config_key_repeat_interval = 50ms`，delete 是唯一 `isRepeatable` 键；GBoard 实测长按退格先逐字符删，持续按住足够久后自动升级为**按词删**。来源：HeliBoard `config-common.xml`；forums.androidcentral.com/threads/723602；xda-developers.com/gboard-feature-adjust-speed-deleting-words/。
- **改法**：重复时序拆成三段常量（起始 400ms → 字符阶段 50ms → 持续 1.5s 后切「按词/按标点段」删除，中文按标点与中英文边界切段）；三个值走配置（用户侧只暴露一个「删除速度」档位：慢/标准/快，映射到三组常量，避免三个滑杆）。iOS 的重复回调补上 `performConfiguredHaptic()`（可降频，每 2 次一响），并把 `Timer` 换成带 `tolerance` 的实现以免 RunLoop 抖动。Android 手指轻微滑出退格键即 `stopLongPressAndRepeat` 的判定（:3160-3162）放宽到 `rect` 外扩 8dp，避免误停。
- **验收标准**：按住退格 400ms 内只删 1 个字符；1 秒内删除约 12–14 个字符；持续按住 2 秒后可见删除粒度变为整词/整段；手指在退格键上小幅抖动不中断连删；iOS 连删期间每次删除有触感/声音反馈。

### P1-8 空格滑动移光标

- **平台**：both
- **现状**：yaml 里空格只有 `swipeDown: { type: reset }`（default-keyboard.yaml:66），无 `longPress`、无横向手势。Android 触摸管线里只有退格走拖动分支（`handleBackspaceDrag` 开头 `isBackspaceKey` 直接 return，KeytaoKeyboardView.kt:3236）；iOS `touchesMoved` 只处理展开面板滚动/符号层滚动/退格横拖（KeyTaoIOSKeyboardView.swift:448-485）。光标移动目前只能通过 editor 层的 5×5 方向键网格（KeytaoInputMethodService.kt:968-971 → `moveCursor` :1036），必须先切层。
- **GBoard 行为**：官方帮助「Swipe left or right on the space bar」即可移动光标，开关在 Glide typing >「Enable gesture cursor control」；可跨行跨句连续移动；已知坑是手指滑出空格触到相邻键会被当成输入。来源：support.google.com/gboard/answer/2842292；pocket-lint.com/how-to-use-the-hidden-cursor-on-gboard/。
- **改法**：
  - Android：`updateKeyTouchMove` 里增加 `isSpaceKey` 分支——位移超过阈值（起点用 AOSP 触摸噪声阈值 12.6dp）后进入光标模式并**锁定该 pointer**，此后忽略相邻键命中；每累计固定 dp（建议 10dp，随键宽缩放）发一次 `KeyEvent.KEYCODE_DPAD_LEFT/RIGHT`，复用 `KeytaoInputMethodService.moveCursor`。抬手不落空格（`longPressConsumed = true` 语义）。
  - iOS：同样在 `touchesMoved` 加 `isSpaceKey` 分支，每 ~10pt 发一次 `edit/cursorLeft|cursorRight`，控制器已有 `moveCursor(byCharacterOffset:)`（KeyTaoKeyboardViewController.swift:790-798）。
  - 有编码（composing）时禁用该手势，直接交给 Rime，避免与选重冲突。
- **验收标准**：按住空格左右滑，光标逐字符移动且**不输入空格**；抬手不落字；跨行连续移动正常；手指滑到相邻键上不出字符；正在输入编码时滑动空格不触发光标模式；短按空格仍走原有 Rime 上屏逻辑。

### P1-9 触感与音效分层，按下即反馈

- **平台**：both（iOS 侧改动更大）
- **现状**：
  - Android `performConfiguredHaptic`（KeytaoKeyboardView.kt:3586-3619）只有 strong→`LONG_PRESS`、否则 `KEYBOARD_TAP` 两级；振幅/`VibrationAttributes.USAGE_TOUCH`/系统开关门控这部分实现扎实，保留。
  - iOS 全局只有一个 `UIImpactFeedbackGenerator(style: .light)`，「强」是把强度除数从 100 改成 60 硬凑（KeyTaoIOSKeyboardView.swift:164、:3373）；且 `performConfiguredHaptic` 只在 `touchesEnded`/工具栏/面板命令里调用，**`touchesBegan` 完全无反馈**（:396-446、:487-575）——这是 iOS「手感发虚」的头号原因。
- **GBoard 行为**：Android 侧 `HapticFeedbackConstants` 自 API 27 起提供 `KEYBOARD_PRESS`/`KEYBOARD_RELEASE` 区分按下与抬起/长按弹出；GBoard 设置把开关与强度分成四项独立设置。来源：developer.android.com/develop/ui/views/haptics/haptic-feedback；support.google.com/gboard/answer/6102154。
- **改法**：
  - 统一三级语义：**按下** = 轻（Android `KEYBOARD_PRESS`，iOS `.light`）；**落字/抬起** = 轻（`KEYBOARD_RELEASE`，iOS 若按下已反馈则不重复）；**长按弹面板/候选选中/层切换** = 中（`LONG_PRESS` / iOS `.medium` 或 `UISelectionFeedbackGenerator.selectionChanged()`）。
  - iOS：`touchesBegan` 命中 `pressedKey` 后立即反馈；`touchesEnded` 改为仅在「按下时未反馈过」的路径补发；不可用操作（编辑面板死键）用 `UINotificationFeedbackGenerator(.warning)`。
  - iOS Full Access 说明：未授予完全访问时 `UIFeedbackGenerator` 静默失效（已由 `hasFullAccess` 门控，KeyTaoKeyboardViewController.swift:138、:152）。改为「用户打开了震动开关但无 Full Access」时在键盘内 `showMessage` 提示一次，不再静默。
- **验收标准**：手指按下瞬间有触感，抬手无二次触感；长按弹出备选时的触感与普通按键可分辨；候选选中触感与按键不同；iOS 未开完全访问时打开震动开关会看到一次提示文案；Android 系统触感总开关关闭时键盘无振动（回归）。

### P1-10 滑动改键容错（slide-to-retarget）

- **平台**：both
- **现状**：按下后手指滑到相邻键，`pressedKey` 不跟随（`updateKeyTouchMove` 只更新坐标并取消长按），释放时 `shouldAcceptKeyRelease` 用**原键**的 rect 判定（KeytaoKeyboardView.kt:3149-3165、:3330-3337）：在原键内→出原字；纵向超阈值且横向不超 0.65×键宽→走 swipe 分支仍出原键；否则**整个按键作废，什么都不出**。iOS 同为 `rect.contains(point)` 单点判定。
- **GBoard 行为**：手指滑到哪个键，就输入哪个键（配合 `config_key_hysteresis_distance_for_sliding_modifier = 8.0dp` 的迟滞防抖）。来源：HeliBoard `config-common.xml`。
- **改法**：`updateKeyTouchMove` 里当手指移出当前键 rect 且落入另一个**字符键** rect 时，改写该 pointer 的目标键（带 8dp 迟滞，避免边界抖动），同步更新按下高亮与预览气泡，并重置长按计时器。功能键（退格/空格/shift/回车/层切换）不参与 retarget，以免破坏 P1-7/P1-8 的独占手势。纵向滑动手势判定优先级高于 retarget（先判 swipeUp/Down）。
- **验收标准**：按下 `d` 后横向滑到 `f` 松手，输出 `f` 而非无输出；滑到键边界来回抖动 5dp 不发生目标跳变；按下 `d` 上滑超过阈值仍走 `d` 的 swipeUp 命令；按住退格横拖仍是逐字删除（回归）。

### P1-11 打字时工具栏不再整条消失（已被裁决 A-1 取代）

- **平台**：both
- **现状**：`drawCandidateBar` 的分支是互斥的——Android 只有 `panelModel.candidates.isEmpty()` 时才画 toolbar（KeytaoKeyboardView.kt:976-992），iOS 顺序为「候选 > preedit > 工具栏」（KeyTaoIOSKeyboardView.swift:719-749）。一旦出现候选，Rime 功能面板 / 中英切换 / 选择 / 剪贴板 / Emoji / 单手 / 悬浮**全部不可达**，必须先清空编码。这是当前对「操作连续性」伤害最大的一条。
- **GBoard 行为**：工具栏与候选共存——建议条左端收纳成一个图标（G/四宫格），点开才展开完整工具栏；右端箭头进功能面板。来源：hongkiat.com/blog/18-gboard-tips-tricks；androidpolice.com/gboard-hidden-settings-finally-made-typing-faster-and-enjoyable/。
- **裁决 A-1**：候选出现时由候选项占满候选栏，不保留左侧工具栏 chip；仅在未显示候选时展示工具栏。本裁决覆盖上面的历史方案描述。
- **验收标准**：输入编码出现候选时，候选项从候选栏左边缘开始布局，候选栏内没有常驻工具栏 chip；候选清空后工具栏恢复；两端一致。

### P1-12 候选栏容量：横向滑动 + 溢出不丢弃

- **平台**：both
- **现状**：
  - Android：`candidateScrollX / candidateDragging / maxCandidateScroll()` 是**死代码**——`drawCandidateBar` 绘制前无条件 `resetCandidateScroll()` 并把 `candidateContentWidth` 设为 `width`，`maxCandidateScroll()` 恒为 0（KeytaoKeyboardView.kt:994、:1026、:3061-3063、:768-775）。用户横扫候选栏得到的是「没反应」。画不下的候选直接 `break`（:1014-1022）。
  - iOS：`inlineCandidateLayout` 剩余宽度不足 24pt 就 `break`，**放不下的候选被直接丢弃**（KeyTaoIOSKeyboardView.swift:1652-1683）。
  - 另有已实现但未接线的能力：`NEXT_PAGE / PREVIOUS_PAGE` 在 service 里已走 `engine.changePage`（KeytaoInputMethodService.kt:517-518），yaml 里**没有任何键绑定它们**。
- **GBoard 行为**：建议条内容超宽可横向滑动浏览，右端有展开入口。
- **改法**：Android 修复 `candidateContentWidth`（按实际排布累计宽度）并让 `ACTION_MOVE` 真正改写 `candidateScrollX`（带边界 clamp）；iOS 补一份同样的横向滚动状态（`candidateScrollX` + `touchesMoved` 分支，滚动中抑制点击上屏）。两端把「滑到右端边界继续拖」映射为 `NEXT_PAGE`，反向为 `PREVIOUS_PAGE`，从而把已有的分页能力接上线。
- **验收标准**：候选多于一屏时左右滑可浏览全部候选且不误触上屏；滑到末尾继续拖会翻到下一页；点击候选仍能正常上屏（滑动位移超过 touchSlop 才算滑动）；iOS 不再出现候选被静默丢弃。

### P1-13 按下高亮：多键并发 + Shift 三态可分辨

- **平台**：both
- **现状**：
  - Android `pressedKey = activeKeyTouches.values.lastOrNull()?.key`，绘制用 `pressedKey?.spec == keyRect.spec` 判断（KeytaoKeyboardView.kt:3205-3207、:2152、:2163）——双指同时按住两个键只有最新那个亮；且比的是 `KeySpec` 数据类相等性，符号层里两个完全相同的 spec 会**一起亮**。
  - Shift：`displayLabel` 只在 LOCKED 时把 ⇧ 换成 ⇪，`isActiveKey` 对 ONCE 与 LOCKED 返回同样的 true，两态共用同一个 `keySelectedBackground`（:2287-2290、:2313-2315；iOS 同：KeyTaoIOSKeyboardView.swift:2954-2957、:2997-2999）。
- **GBoard 行为**：每根手指按住的键各自高亮；Shift 用填充 vs 下划线 + 不同色明确区分一次性大写与锁定（双击 Shift 开 Caps Lock 再点关闭）。来源：support.google.com/gboard/answer/2842292。
- **改法**：高亮判定改为按 **KeyRect 实例/索引**而非 `KeySpec` 值相等，并遍历所有活动 touch 绘制（依赖 P1-1/P1-2 的 per-touch 状态）。Shift 三态视觉：OFF = 常规底色；ONCE = 描边高亮（不填充）；LOCKED = 实心填充 + 底部横杠 + ⇪ 字形。新增 `pressedBackground` 主题 token，与 `selectedBackground` 分离，避免「按下」与「shift 激活」同色。
- **验收标准**：两指同时按住两个键，两个键都高亮；符号层里重复符号只亮被按的那个；截图对比 OFF/ONCE/LOCKED 三态可分辨；按下一个已激活的 shift 键时颜色与未激活按下态不同。

---

# P2 效率与发现性

不是每分钟都碰，但决定「用户能不能发现并用上」的能力。第二批。

### P2-1 键角提示（hint）显示开关 + 覆盖面补齐

- **平台**：both（yaml + 设置）
- **现状**：hint 绘制在键面右上角（Android KeytaoKeyboardView.kt:2206-2211、字号 `keyHintSizeSp()` :1467-1469，封顶 12sp 下限 9sp），yaml 中 28 个键带 hint（default-keyboard.yaml:30-68）。机制完备但：无显示开关；符号层、Emoji 层、功能键全无 hint；9–12sp 在小屏上偏小。
- **GBoard 行为**：「Touch & hold keys for symbols」开关控制**是否显示角标**，关掉后仍可长按输入——显示与可用解耦。来源：9to5google.com/2025/10/28/gboard-flick-keys/。
- **改法**：新增运行时设置 `keyHintVisible`（默认开），只控制绘制、不影响长按；hint 字号下限提到 10sp 并随键高缩放；给符号层高频键补 hint（与 P1-5 的 alternates 数据同批填）。
- **验收标准**：关闭开关后键角文字消失但长按仍出对应字符；小屏（<360dp 宽）上 hint 可辨认。

### P2-2 Flick keys：键上下拉输入角标符号

- **平台**：both
- **现状**：下滑手势通道已存在（`swipeDown`，`resolveCommand` 按 deltaY 三分支：Android :2248-2256；iOS :2732-2764），但 yaml 里 `swipeDown` 只有空格的 `reset` 一处，`swipeUp` 出现 0 次；角标符号只能通过长按取得。
- **GBoard 行为**：16.2（2025-10）新增 Preferences > Shortcuts >「Flick keys to enter symbols: Touch a key and pull down to enter its hinted symbol」，顶排 QWERTYUIOP 下拉分别得 1–0，键内字母被数字以下滑动画短暂替换，默认关闭，官方建议与「Touch & hold keys for symbols」搭配。来源：9to5google.com/2025/10/28/gboard-flick-keys/；androidauthority.com/gboard-flick-for-symbols-typing-gesture-3611242/。
- **改法**：不改手势引擎，只做**语义补默认**——当键有 `hint`/`longPress` 而无 `swipeDown` 时，下滑自动落 hint 对应命令（代码里的 fallback，不必逐键写 yaml）。由运行时设置 `flickKeysEnabled` 控制（默认开；KeyTao 无滑行输入，误触风险显著低于 GBoard）。视觉反馈复用 P1-4 的气泡（下滑过程中气泡内容切成目标符号）。
- **验收标准**：下拉 `q` 输出 `1`、下拉 `a` 输出 `@`，下拉过程中气泡显示目标符号；关闭开关后下拉不再输出符号而按原 `swipeDown` 处理；空格的 `swipeDown: reset` 不受影响（显式配置优先于 fallback）。

### P2-3 常驻数字行开关

- **平台**：both
- **现状**：无常驻数字行。唯一的数字行是键道特例：`shouldUseInlineNumberRow` 仅当 `!asciiMode && hasComposition && preedit.contains("=")` 时把第一行整行替换为 1234567890，并清空该行所有 hint/longPress/swipe（Android :2326-2358；iOS :2296-2324）。这是只有键道用户懂的隐式规则，且用户无法主动开启。
- **GBoard 行为**：设置 > Preferences >「Number row」常驻一行数字；与 Flick keys 属替代关系。来源：androidpolice.com/gboard-hidden-settings-finally-made-typing-faster-and-enjoyable/。
- **改法**：新增运行时设置 `numberRowEnabled`（默认关）。开启时在字母层顶部插入一排数字（复用现有 inline 数字行的行构造逻辑），键盘总高按行数比例增加（联动 `keyboardHeightDp`）。设置页在该项旁提示与 P2-2 Flick 的功能重叠。键道 `=` 引导的临时替换逻辑保留不变。
- **验收标准**：开启后字母层顶部常驻数字行、键盘总高相应增加，关闭后完全恢复；开启状态下输入 `=` 引导时不出现「两行数字」；横竖屏切换后状态保持。

### P2-4 设置项暴露：把已有配置接到设置页

- **平台**：both（`src/App.tsx` + 两端 runtime settings）
- **现状**：设置页只有 4 组：震动开关、震动强度 1–100（默认 42）、回车行为、悬浮启用+缩放（src/App.tsx:1822-1880）。运行时下发通道只认 haptics / enterKeyBehavior / floating（KeytaoAndroidImeConfig.kt:374-410；KeyTaoIOSConfig.swift:744-751），**前端加了别的开关配置层也读不到**。而 `keyboardHeightDp`(266)、`candidateBarHeightDp`(52)、`swipeThresholdDp`(34) 在解析层已存在且可调（KeytaoAndroidImeConfig.kt:267-308；default-keyboard.yaml:8-14），唯一入口是手改 yaml。iOS 设置页复用的是 `androidImeInputSettings` 分支（App.tsx:1068-1096），没有 iOS 专属项。
- **GBoard 行为**：键盘高度、长按延迟、数字行、按键音/振动强度均为设置页一级项。
- **改法**：一次性扩展 runtime settings schema（两端 + 前端共用字段名），新增：`keyboardHeightDp`、`candidateBarHeightDp`、`swipeThresholdDp`、`longPressDelayMs`(P1-6)、`deleteSpeed`(P1-7)、`keyPreviewEnabled`(P1-4)、`keySoundEnabled`+`keySoundVolume`(P2-5)、`keyHintVisible`(P2-1)、`flickKeysEnabled`(P2-2)、`numberRowEnabled`(P2-3)、`candidateFontScale`(P2-8)。设置页按「反馈 / 手势与时序 / 布局」三组呈现，iOS 分支加平台差异说明文案。**这是 P1 多个条目的共同依赖，须在第一批内先落地 schema 部分。**
- **验收标准**：设置页每一项改动后回到任意输入框，键盘立即生效无需重启；App Group / SharedPreferences 中可见对应 JSON 字段；恢复默认按钮能把全部项复位。

### P2-5 按键音：App 级开关与音量（Android）+ iOS 平台说明

- **平台**：Android 可做完整；iOS 受限
- **现状**：Android `playConfiguredKeySound` 只判系统 `SOUND_EFFECTS_ENABLED` 后直接 `audioManager.playSoundEffect(fx)`，无 config 字段、未使用带 volume 的重载（KeytaoKeyboardView.kt:3621-3637）；按命令类型区分 DELETE/RETURN/SPACEBAR/STANDARD 这点保留。iOS 用 `UIDevice.current.playInputClick()` + `enableInputClicksWhenVisible`（KeyTaoIOSKeyboardView.swift:3369、:3470-3472），这是扩展唯一合法路径。
- **GBoard 行为**：Preferences > Key press 下四个独立项：Sound on keypress / Volume on keypress / Haptic feedback on keypress / Vibration strength。来源：support.google.com/gboard/answer/6102154。
- **改法**：Android 加 `keySoundEnabled` + `keySoundVolume`（0–100，映射到 `playSoundEffect(fx, volume)`），仍受系统总开关门控。iOS 在设置页该项显示为「跟随系统按键音（iOS 扩展无法自定义音色与音量）」的只读说明。
- **验收标准**：Android 关闭 App 级按键音后无声音但振动仍在；音量 20% 与 100% 可听出差异；iOS 设置页显示平台说明且不出现无效滑杆。

### P2-6 双击空格出句号

- **平台**：both
- **现状**：`handleSpace` 只有「有编码交给 Rime / 无编码 commit 一个空格」（KeytaoInputMethodService.kt:855-861），无双击判定。
- **GBoard 行为**：设置项「Double-space for period」，AOSP 判定窗口 `config_double_space_period_timeout = 1100ms`。来源：hardreset.info/devices/apps/apps-gboard/enable-double-space-period/；HeliBoard `config-common.xml`。
- **改法**：无编码状态下，1100ms 内连击两次空格 → 删除刚提交的空格并插入「中文态：`。`；英文态：`. `」。仅在前一个字符是「非空白、非标点」时触发。默认开，可关。
- **验收标准**：连击两次空格得到 `。`（中文）/ `. `（英文）；间隔超过 1100ms 得到两个空格；句尾已有标点时连击两次空格仍得到两个空格；正在输入编码时不触发。

### P2-7 工具栏可滚动 + 溢出处理

- **平台**：both
- **现状**：Android `drawToolbar` 把 7 个 chip 硬塞进一行——compression 夹在 [0.6, 1]，不够就整体等比缩（KeytaoKeyboardView.kt:1678-1743、:1798-1824），无横向滚动、无「更多」按钮；单手模式（宽度 ×0.78）下挤成一团。iOS 6 项同样已接近宽度上限（KeyTaoIOSKeyboardView.swift:2326-2359、:1825-1849）。
- **GBoard 行为**：工具栏可横向滚动，且可在编辑模式下拖拽重排/置顶。来源：9to5google.com/2023/06/02/customize-gboard-shortcuts/。
- **改法**：先做**横向滚动**（复用 P1-12 的候选栏滚动实现），compression 下限从 0.6 提到 0.85，装不下就滚动而不是继续压缩。拖拽自定义排序放到 P3-6。
- **验收标准**：360dp 宽单手模式下工具栏 chip 不再挤压变形，可横向滑动看到全部项；滑动中点击不误触发命令。

### P2-8 候选字号上限放开

- **平台**：Android（iOS 同步核对）
- **现状**：`candidateTextSizeSp() = min(theme.fontSizeSp - 2, 16)` 下限 13——**硬封顶 16sp**，label 封顶 13sp、comment 封顶 12sp（KeytaoKeyboardView.kt:1448-1452）。用户调大主题字号也顶不上去。中文候选在 16sp 下辨识度低于主流中文输入法的 18–20sp。
- **改法**：封顶提到 22sp，并新增 `candidateFontScale`（0.8–1.4，默认 1.0）走 P2-4 通道；候选栏高度不足时按比例回缩而不是硬截断。
- **验收标准**：主题字号 20sp 时候选文字明显大于当前版本；字号拉满时候选文字不被候选栏裁切；两端视觉一致。

### P2-9 候选长按菜单

- **平台**：both
- **现状**：主候选栏没有 pressed 状态字段，`drawCandidateOption` 恒以 `pressed=false` 绘制（KeytaoKeyboardView.kt:853-864、:1489-1497）；长按计时器只挂在按键上（:201-217；iOS `scheduleLongPressIfNeeded` 只对 `pressedKey` 生效，:2900-2903）。中文输入法标配的「长按候选删除用户词/查看编码」完全没有代码路径。
- **GBoard 行为**：长按建议条候选，拖到出现的垃圾桶图标上松手，删除该条**学习得到的**预测（词典自带词不可删）；全局兜底是设置 > 隐私 >「Delete learned words and data」。来源：roboin.io/article/2024/02/22/gboard-suggestion-delete/。
- **改法**：候选加按下态与长按计时（复用按键的计时器实现）；长按弹出小菜单：「删除该词（仅用户词）/ 查看编码」。用户词与系统词典词的区分取 Rime 侧的候选来源标记；系统词只显示编码并给不可删的提示。
- **验收标准**：长按用户造的词弹出菜单，选删除后该词不再出现在同一编码的候选里；长按系统词典词只显示编码并提示不可删除；短按候选仍正常上屏。

### P2-10 Emoji 最近使用 + 分类跳转条

- **平台**：both
- **现状**：`grep recentEmoji|recentlyUsed|frequent` 在 Android 包内 0 命中——Emoji 面板每次都从固定分类第一页开始。iOS 符号/Emoji 层支持整块纵向滚动（KeyTaoIOSKeyboardView.swift:428-435、:2604-2627）但无分类跳转条。
- **GBoard 行为**：Emoji 面板有最近使用分组与底部分类 tab。
- **改法**：本地持久化最近使用的 32 个 Emoji（Android SharedPreferences / iOS App Group），置于面板首个分组；面板底部加一行分类 tab，点击滚动到对应分组偏移。
- **验收标准**：使用过的 Emoji 出现在面板首屏且按最近使用排序；点击底部分类 tab 直接跳到该分组；重启键盘后最近使用仍保留。

### P2-11 常驻中英态指示 + 设置直达

- **平台**：both
- **现状**：中英态有三处呈现（工具栏 chip、空格键上的 schemaName、MODE 键文案：Android :2746-2760、:2294-2299；iOS :2350-2358、:2383-2396），但工具栏在打字时消失（P1-11），此时只剩空格键上的方案名。设置入口：Android `ToolbarIcon.SETTINGS` 图标与 `drawSettingsToolbarIcon` 已实现但 `toolbarActions()` 从不产出 SETTINGS 项（:110、:2111-2122、:2635-2676）——画好没接线；iOS 的设置 chip 只存在于功能面板内部。
- **改法**：把中英态指示放在空格键 keycap，或放在未组词、未显示候选时出现的工具栏；不得在候选栏中显示中英态 chip。工具栏补一个 SETTINGS 项（Android 直接接现成的绘制函数）。
- **验收标准**：可从空格键 keycap 或未组词时的工具栏确认当前中英态；出现候选时，候选栏内没有中英态或工具栏 chip；工具栏点齿轮直达设置页。

### P2-12 退格拖动删除的视觉预览

- **平台**：both
- **现状**：横向拖动退格按 0.22×键宽 为步长增量删除/恢复（最多 96 单位，反向可恢复；Android :3229-3275、:3660；iOS :2672-2726）——**功能强于 GBoard，但无视觉反馈**，用户只能靠输入框实时变化判断。
- **GBoard 行为**：gesture delete 在退格上左滑时**高亮**将被删除的词，抬手才真正删除，右滑回退可取消；误删后被删文本出现在候选栏，点一下恢复。来源：androidpolice.com/gboard-backspace-psa/。
- **改法**：保留现有「即时删除 + 反向恢复」模型（它更强，别改坏），只补反馈：拖动过程中在候选栏位置显示「已删除 N 字：<被删文本尾部>」的浮条，抬手 2 秒后消失，点击浮条 = 全部恢复。
- **验收标准**：左拖删除过程中候选栏显示被删字数与内容；右拖回退时数字同步减少；抬手后点浮条能一次性恢复本次手势删除的全部内容；不影响纵滑 deleteAll/restoreAll。

### P2-13 触摸去噪阈值参数化

- **平台**：both
- **现状**：Android `swipeThresholdDp` 默认 34dp（yaml 可配，夹 12–96），但无触摸噪声过滤；`shouldAcceptKeyRelease` 用 `max(2×touchSlop, 0.65×键宽)` 判定横向取消（:3330-3337）。iOS `swipeThresholdDp` 只随一手模式缩放，用户不可调（KeyTaoIOSConfig.swift:498-509）。
- **GBoard 行为**：AOSP `config_touch_noise_threshold_time = 40ms`、`config_touch_noise_threshold_distance = 12.6dp`、`config_key_hysteresis_distance_for_sliding_modifier = 8.0dp`。来源：HeliBoard `config-common.xml`。
- **改法**：在 DOWN 时做逐指针回弹抑制：记录同一 pointer 上一次 UP 的时间与位置；新的 DOWN 若距该 pointer 上一次 UP `<40ms` **且** 距离 `<12.6dp`，则作为回弹噪声丢弃。任何干净点击都不按本次触摸的持续时长过滤。迟滞距离 8dp 供 P1-10 使用；`swipeThresholdDp` 接入设置页（P2-4）。
- **验收标准**：同键 60ms 间隔双击两次都生效；换键 30ms 连点两次都生效；同位置 20ms 回弹被忽略；滑动阈值设置项两端均生效。

### P2-14 下滑收起键盘

- **平台**：Android（iOS 不适用）
- **现状**：`onTouchEvent` 无「键盘区域整体下滑 → hideWindow」处理，也无 `ACTION_OUTSIDE` 分支（KeytaoKeyboardView.kt:657-901）。悬浮模式的底部拖柄/四角 resize（:2679-2744）是移动缩放，不是收起。
- **GBoard 行为**：键盘上向下滑动可收起键盘。
- **改法**：在候选栏/工具栏空白区域（不含按键区，避免与 P2-2 的 flick 冲突）识别向下快速滑动（>1.5×swipeThreshold 且速度超阈值）→ `requestHideSelf(0)`。
- **验收标准**：在候选栏空白处下滑键盘收起；在字母键上下滑仍是 flick 输入符号；悬浮模式下不误触发。

### P2-15 iOS VoiceOver 打字可用（无障碍，不可省）

- **平台**：iOS
- **现状**：`rebuildAccessibilityElements` 为每个键建 `UIAccessibilityElement`，traits 用 `.button` 且**没有 activation 回调**（KeyTaoIOSKeyboardView.swift:628-673）——VoiceOver 双击字母键不输出任何字符；候选元素同样无 activation。只有工具栏 chip 和剪贴板删除按钮用了带 `accessibilityActivate` 的 `KeyTaoActivatingAccessibilityElement`（:13-19）。
- **改法**：键与候选也换成 `KeyTaoActivatingAccessibilityElement` 并接 `didTrigger` / `didSelectCandidate`；traits 加 `UIAccessibilityTraits.keyboardKey`（系统据此启用 VoiceOver「触摸打字」模式，抬手即上屏）。
- **验收标准**：开启 VoiceOver 后逐键浏览 + 双击能打出字；切到「触摸打字」模式后抬手即上屏；候选可朗读并可双击选中。

### P2-16 iOS 编辑面板死键处理

- **平台**：iOS
- **现状**：editor 网格里的 全选 / 清除 / 选择(toggleSelection) / 重做 四个键被 `alwaysUnsupportedEditVerbs` 标为不可用，以 0.38 alpha 画出来但点击直接 return（KeyTaoIOSKeyboardView.swift:3460-3467、:1306-1315、:438-443；default-keyboard.yaml:3067-3093）。根因是 `UITextDocumentProxy` 只提供 `insertText/deleteBackward/adjustTextPosition`，没有选区或撤销栈 API——**平台限制，不可实现**。当前「画出来但点不动」会让用户反复戳。
- **改法**：iOS 编辑网格隐藏这四个键，用可实现的功能补位：整词删除、括号/引号包裹（配合光标移动）、常用短语。若保留则点击时 `showMessage` 一次性说明原因。
- **验收标准**：iOS 编辑网格中不存在长期灰死、点击无任何反馈的按键；Android 编辑网格功能不变。

### 子批次 A 补充：剪贴板条目上屏后自动返回键盘

- **平台**：both
- **验收标准**：点选任一剪贴板文本条目时，上屏动作派发后关闭面板并返回字母键盘（当前链路无成功/失败回执）；删除单条（✕）与清空操作不关闭面板。两端一致。

---

# P3 锦上添花 / 大工程

### P3-1 布局与绘制解耦（消除命中矩形竞态）

- **平台**：both
- **现状**：`keyRects` 在 `onDraw` 开头清空、在 `drawKeyboard` 末尾才回填（`candidateRects/toolbarRects/expandedCandidateRects` 同理；KeytaoKeyboardView.kt:471-488、:2143-2170、:3305-3312），`findKey` 命中的是**上一帧**的布局。切层、展开面板、悬浮缩放的那一帧可能命中已不存在的键位。
- **子批次 A 审计结论**：布局与绘制耦合会造成状态切换帧的错键风险，但与本次快速点击丢击无关；丢击根因是抬手阶段按本次触摸时长过滤，改为 DOWN 时基于同一 pointer 上一次 UP 的回弹抑制后，P3-1 无需提前到子批次 A。
- **改法**：把布局计算从 `onDraw` 抽到 `onMeasure/onSizeChanged` + 状态变更时触发的 `rebuildLayout()`，`onDraw` 只消费布局结果。iOS 同理（`draw(_:)` 内的布局计算抽到 `layoutSubviews`）。
- **验收标准**：真机手工确认切层动画进行中连续点击不产生错键输入；结构检查确认所有层/尺寸/滚动状态变更都会同步重建命中矩形，且 DOWN 在命中前兜底重建。当前 Android JVM 单元测试没有 View/Robolectric seam，iOS policy harness 也不链接 UIKit，因此不以 host 单元测试宣称实际 View 命中链路覆盖；待加入平台 View harness 后再补自动化。

### P3-2 面板惯性滚动 / 回弹 / 滚动条

- **平台**：both
- **现状**：展开候选、剪贴板、符号分类页都是「拖多少滚多少」，`ACTION_UP` 直接结束——无 `VelocityTracker`/`OverScroller`、无回弹、无滚动条（Android :755-767、:776-789、:831-852；iOS :453-465、:752-760）。96 条候选或长 Emoji 列表要反复拖拽。
- **改法**：Android 接 `VelocityTracker` + `OverScroller`；iOS 用简单的速度衰减动画（`CADisplayLink`）。同时加细滚动条指示。
- **验收标准**：快速甩动候选面板后列表继续滑行并平滑停止；到达边界有轻微回弹；滚动时右侧出现滚动条并在停止后淡出。

### P3-4 退格「按词选中 → 抬手删除 → 可撤回」模型

- **平台**：both
- **现状**：现有横拖是「即时删除 + 反向恢复」，P2-12 只补了视觉。GBoard 的模型是「高亮选中 → 抬手才删 → 被删文本进候选栏可一键恢复」，语义上更安全。
- **改法**：作为可切换的第二种退格手势模式（设置项：即时删除 / 选中后删除），默认保持现有模式（它是 KeyTao 的既有优势）。
- **验收标准**：切到「选中后删除」模式，左滑时文本被高亮但未删除，抬手才删除，候选栏出现恢复入口；切回默认模式行为与现状一致。

### P3-5 按下态主题 token 与过渡动画

- **平台**：both
- **现状**：pressed 复用 `selected` 色（与 shift 激活同色），自绘无过渡（iOS :1321-1329、:1390-1400）。
- **改法**：主题新增 `pressedBackground/pressedForeground`；加 80ms 的短促淡入淡出（Android `ValueAnimator` / iOS `CADisplayLink`），仅在按下/抬起时驱动重绘的局部区域。
- **验收标准**：连打时每次按下的高亮变化可见但不拖尾；60fps 下无掉帧；关闭动画（无障碍「减弱动态效果」）时回到即时切换。

### P3-6 工具栏拖拽自定义（置顶/更多）

- **平台**：both
- **现状**：`ToolbarAction/ToolbarRect` 有 `longPressCommand` 字段且 `ACTION_DOWN` 会挂计时器，但 `toolbarActions()` 构造的 7 个 action **没有一个**传该字段（KeytaoKeyboardView.kt:218-225、:695-697、:2635-2676）——死代码。
- **改法**：先低成本接线（长按 Emoji 直达最近使用、长按剪贴板清空、长按中英切到符号层），再做 GBoard 式的编辑模式（拖拽在「置顶栏/更多」之间移动并重排，顺序持久化到 runtime settings）。
- **验收标准**：长按工具栏项触发二级动作；编辑模式下拖拽排序后键盘立即按新顺序绘制，重启后保持。

### P3-7 一次触摸到达二级功能（长按拖到图标）

- **平台**：both
- **现状**：切到符号层/悬浮/单手都要点击多次。
- **GBoard 行为**：长按逗号并拖到目标图标——拖到齿轮=设置、向右拖=左侧悬浮键盘；长按回车=右侧悬浮键盘。来源：support.google.com/gboard/answer/2842292。
- **改法**：复用 P1-5 的 alternates 面板机制，把「命令类」备选（设置/悬浮/单手/符号层）作为逗号与回车的 alternates 项。数据即可实现，无需新代码路径。
- **验收标准**：长按逗号拖到齿轮松手打开设置；长按回车拖到悬浮图标松手进入悬浮模式。

### P3-8 iOS 一手模式高度独立调节

- **平台**：iOS
- **现状**：`KeyTaoResizableKeyboardContainerView` 用一个 `UIPanGesture` 做边缘拖拽改 scale，**只有横向缩放**；高度只随 scale 等比变化（KeyTaoFloatingKeyboardInteraction.swift:74-157；KeyTaoKeyboardViewController.swift:1215-1244）。
- **改法**：增加纵向拖拽改 `heightConstraint.constant`，按屏幕方向分别持久化到既有的 `layoutStateStore`。
- **验收标准**：一手模式下上下拖拽边缘可单独调整高度并持久化；横竖屏各存一份；切回全宽模式高度恢复默认。

---

## 明确不做

以下能力**已评估并决定不实现**，避免后续批次重复讨论或误实现：

| 项 | 原因 |
| --- | --- |
| 滑行输入（glide typing / 轨迹输入） | 与形码方案根本冲突：键道按键序列不是词的字母序列，轨迹匹配无意义。英文层的落差可接受。 |
| 英文自动纠错 / autocorrect | 需要英文词频模型与纠错词典，工程量大且不服务主场景（中文形码）；英文输入由已安装的 Rime schema 负责。 |
| 中文自动首字母大写 / `TYPE_TEXT_FLAG_CAP_SENTENCES` | 中文无大小写；英文层作为次要场景，收益低于实现与误触成本。 |
| 语音输入、翻译、GIF、贴纸、表情搜索 | GBoard 的云服务能力，KeyTao 无对应后端，且与「本地输入法」定位冲突。 |
| iOS 自由浮动键盘（任意位置移动 + dock/undock） | **平台硬限制**：键盘扩展只能在系统给的 `inputView` 矩形内绘制，无 move handle 可言。iOS 只做「一手模式（缩窄靠边）+ 分栏 + 高度拖拽」（P3-8）。 |
| iOS 自定义按键音色与音量 | **平台硬限制**：扩展只能调 `UIDevice.current.playInputClick()`，音色由系统决定、无音量参数，且遵循用户的系统「按键声音」总开关。设置页只显示说明文案（P2-5）。 |
| iOS 全选 / 选择 / 重做 / 清除 | **平台硬限制**：`UITextDocumentProxy` 无选区与撤销栈 API。处理方式见 P2-16（隐藏或说明），不尝试用 hack 实现。 |
| iOS 无 Full Access 时的剪贴板与触感 | **平台硬限制**：未授予完全访问时 `UIPasteboard` 被拒、`UIFeedbackGenerator` 静默失效。现有 `hasFullAccess` 门控与 `showMessage` 提示已正确（KeyTaoKeyboardViewController.swift:738-769），只补 P1-9 的震动开关提示，不做其它绕行。 |
| 「候选展开成多行网格」声称对齐 GBoard | 公开资料中没有 GBoard 中文/日文该交互的权威描述。KeyTao 的展开候选页按自身设计演进即可，文档与 PR 描述中不要写「对齐 GBoard」。 |
| GBoard 的删词速度滑杆 | GBoard 当前是固定速率（滑杆仅见于灰度）。KeyTao 用 P1-7 的三档「删除速度」替代，不做连续滑杆。 |

---

## 实施安排

### 第 1 批（P1，打字手感核心）

顺序建议（前两项是 BUG，必须先落，且是后续多项的前置）：

1. **前置**：P2-4 的 runtime settings schema 扩展（只做字段通道，不做完整设置 UI）——P1-4/P1-6/P1-7 均依赖它。
2. **BUG 修复**：P1-1（iOS 多点触控）→ P1-2（Android 多指长按/主指移交）→ P1-3（iOS 长按 123）。P1-1 与 P1-2 的 per-touch 状态重构是 P1-10、P1-13 的基础，必须先合入。
3. **时序类**：P1-6（长按 300ms 可调）→ P1-7（退格 400/50 + 按词删）。
4. **手势类**：P1-8（空格滑动移光标）→ P1-10（滑动改键容错）。
5. **视觉反馈类**：P1-4（预览气泡）→ P1-5（长按备选面板）→ P1-13（多键高亮 + Shift 三态）→ P1-9（触感音效分层）。
6. **候选/工具栏**：P1-11（工具栏折叠共存）→ P1-12（候选横滑与翻页）。

### 分工与流程

- **实现**：全部交 Codex（`codex:codex-rescue`）。每个编号一个独立提交，提交信息带编号（如 `P1-7: backspace repeat timing`），便于回滚与验收对照。
- **不写测试的部分**：绘制、颜色、气泡布局、工具栏排布属简单代码，**不写单元测试**，只做真机验收。
- **必须写测试的部分**：P1-1（多点触控 rollover 序列）、P1-2（长按计时器移交）、P1-7（重复时序状态机）、P1-8（位移累计到光标步进的映射）——这四项是并发/时序/数据完整性路径，各留一个最小可运行用例即可，不建套件。
- **评审**：Codex 实现后由独立的 Claude（Opus 5，`model: "opus"`，与实现者不共享上下文）做跨模型评审，重点看 P1-1/P1-2 的触摸状态机是否有遗留的单点假设、以及退格横拖手势（两端的既有优势）是否被回归破坏。
- **部署与验收**：评审修复合入后部署，由用户在**真机**上逐条按每项的「验收标准」核对；Android 与 iOS 各跑一遍。回归清单必须包含：退格横拖删除/恢复、纵滑 deleteAll/restoreAll、上下滑输入、悬浮键盘移动缩放、单手模式、编码中切层不丢编码。
- **第 2 批（P2）** 在第 1 批真机验收通过后启动；**P3** 按用户当时的优先判断单独排期，不与前两批混排。
