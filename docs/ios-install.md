# iOS / iPadOS IPA 签名与安装

KeyTao 的 GitHub Release 会提供名称类似下面的 iOS 构建：

```text
keytao-app-<version>-ios-arm64-unsigned.ipa
```

该文件是**未签名 IPA**，不能直接安装到 iPhone 或 iPad。本文说明如何为当前 KeyTao iOS 包签名、安装并启用键盘。

KeyTao 当前要求 iOS / iPadOS 15.0 或更高版本。

## 先了解 KeyTao IPA 的结构

KeyTao 不是只有一个普通 App。IPA 内同时包含：

```text
Payload/KeyTao.app
Payload/KeyTao.app/PlugIns/KeyTaoKeyboard.appex
```

签名时需要满足以下条件：

1. 主 App 和键盘扩展分别使用匹配各自 Bundle ID 的 provisioning profile。
2. 先签名 `KeyTaoKeyboard.appex`，最后签名外层 `KeyTao.app`。
3. 两个 profile 必须包含相同的 App Group，完整功能默认使用 `group.ink.rea.keytao-app`。
4. 不要使用 `codesign --deep` 代替逐层签名；`--deep` 只用于最后验证。

当前官方构建使用以下标识：

| 项目 | 标识 |
| --- | --- |
| 主 App Bundle ID | `ink.rea.keytao-app` |
| 键盘扩展 Bundle ID | `ink.rea.keytao-app.keyboard` |
| App Group | `group.ink.rea.keytao-app` |

只有拥有这些标识和 App Group 的开发者团队，才能直接为 Release 中的原始 IPA 创建匹配的描述文件。其他开发者需要从源码构建，并改成自己团队下唯一的 Bundle ID 和 App Group，不能只修改外层 App 的 Bundle ID。

## 选择安装方式

| 目标 | 推荐方式 | Apple Developer Program |
| --- | --- | --- |
| 在自己的设备上开发测试 | 从源码构建，使用 Xcode 自动签名 | 可使用免费 Apple ID，但 Personal Team 描述文件通常只有 7 天有效期 |
| 在已登记的少量设备上测试 | Development 或 Ad Hoc 签名 IPA | 需要付费会员 |
| 提供公开测试 | TestFlight | 需要付费会员 |
| 面向普通用户长期分发 | App Store | 需要付费会员和审核 |

Apple 的免费 Personal Team 只适合在自己的设备上测试，不是公开分发方式。它有设备、App 数量和 7 天描述文件有效期等限制，可使用的 capability 也少于付费会员。如果 Personal Team profile 没有授权 App Groups，只能移除该 entitlement 后测试包内数据。开源或免费软件不会免除 Apple 的代码签名要求。

符合资格的非营利组织、教育机构和政府实体可以申请 [Apple Developer Program 会费减免](https://developer.apple.com/support/membership-fee-waiver/)，但项目开源或不盈利本身不等于自动符合减免条件。

## 方式一：从源码使用 Xcode 自动签名

这是非 KeyTao 官方开发者团队最可靠的方式，也最容易看清主 App 和键盘扩展各自的签名错误。

### 1. 准备环境

需要 macOS、最新版稳定版 Xcode、Node.js、pnpm 和 Rust。先连接 iPhone 或 iPad，并在 Xcode 中登录 Apple ID。

```bash
git clone https://github.com/xkinput/keytao-app.git
cd keytao-app
pnpm install
rustup target add aarch64-apple-ios
```

### 2. 使用自己的标识

如果不属于 KeyTao 官方开发者团队，请先为自己的团队准备三个唯一标识，例如：

```text
com.example.keytao
com.example.keytao.keyboard
group.com.example.keytao
```

需要一致修改以下位置：

- `src-tauri/tauri.conf.json` 中的 App identifier。
- `src-tauri/src/lib.rs` 中的 iOS App Group identifier。
- `crates/keytao-ios-ime/Sources/KeyTaoIOSIME/KeyTaoIOSEngine.swift` 中的 App Group identifier。
- `crates/keytao-ios-ime/Resources/KeyTaoApp.entitlements` 中的 App Group。
- `crates/keytao-ios-ime/Resources/KeyTaoKeyboard.entitlements` 中的 App Group。

生成 iOS 工程时再提供键盘 Bundle ID、App Group 和 Team ID：

```bash
export KEYTAO_IOS_KEYBOARD_BUNDLE_ID="com.example.keytao.keyboard"
export KEYTAO_IOS_APP_GROUP="group.com.example.keytao"
export KEYTAO_IOS_DEVELOPMENT_TEAM="YOUR_TEAM_ID"
pnpm init:ios
```

`KEYTAO_IOS_APP_GROUP` 会更新生成工程中的主 App entitlement，但键盘运行时和源码 entitlement 仍需要按上面的文件列表同步修改。

### 3. 构建运行库并打开 Xcode

```bash
pnpm build:ios -- --no-sign --ci
open src-tauri/gen/apple/keytao-app.xcodeproj
```

在 Xcode 中完成以下设置：

1. 选择 `keytao-app_iOS` target，在 **Signing & Capabilities** 中开启 **Automatically manage signing**，选择自己的 Team。
2. 选择 `KeyTaoKeyboard` target，使用同一个 Team，并开启自动签名。
3. 付费团队需要确认两个 target 都包含 **App Groups** capability，并勾选同一个 App Group。
4. 选择已连接的 iPhone 或 iPad，运行 `keytao-app_iOS` scheme。

如果使用免费 Personal Team，而 Xcode 提示 profile 不支持 App Groups，需要从两个 target 移除 App Groups capability 和对应 entitlement。这样只能测试键盘包内自带的 Rime 数据，主 App 与键盘扩展之间的方案、主题和部署状态共享不会完整工作。

## 方式二：手动重签 Release IPA

此方式适合 KeyTao 官方团队，或已经拥有与 IPA 内 Bundle ID 完全匹配的证书和描述文件的开发者。其他团队不能直接注册已被占用的官方 Bundle ID，应使用“方式一”从源码构建自己的包。

### 1. 准备证书与描述文件

需要准备：

- 一个可用的 `Apple Development` 或 `Apple Distribution` 签名证书及其私钥。
- 主 App 的 `.mobileprovision` 文件。
- 键盘扩展的 `.mobileprovision` 文件。
- 使用 Development 或 Ad Hoc 安装时，目标设备 UDID 必须包含在描述文件中。
- 两个 App ID 都要启用 App Groups，并分配同一个 App Group。

可用下面的命令查看本机签名身份：

```bash
security find-identity -v -p codesigning
```

### 2. 检查描述文件

```bash
security cms -D -i KeyTaoApp.mobileprovision > /tmp/keytao-app-profile.plist
security cms -D -i KeyTaoKeyboard.mobileprovision > /tmp/keytao-keyboard-profile.plist
plutil -extract Entitlements.application-identifier raw -o - /tmp/keytao-app-profile.plist
plutil -extract Entitlements.application-identifier raw -o - /tmp/keytao-keyboard-profile.plist
/usr/libexec/PlistBuddy \
  -c "Print :Entitlements:com.apple.security.application-groups" \
  /tmp/keytao-app-profile.plist
/usr/libexec/PlistBuddy \
  -c "Print :Entitlements:com.apple.security.application-groups" \
  /tmp/keytao-keyboard-profile.plist
```

两个 `application-identifier` 应分别以主 App 和键盘扩展 Bundle ID 结尾，两个 App Group 输出必须相同。

### 3. 解包并按顺序签名

下面的脚本适用于当前 KeyTao Release IPA 结构。先修改开头的五个变量：

```bash
#!/usr/bin/env bash
set -euo pipefail

IPA="keytao-app-<version>-ios-arm64-unsigned.ipa"
OUTPUT="KeyTao-signed.ipa"
APP_PROFILE="KeyTaoApp.mobileprovision"
KEYBOARD_PROFILE="KeyTaoKeyboard.mobileprovision"
IDENTITY="Apple Development: Your Name (TEAMID)"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

ditto -x -k "$IPA" "$WORK_DIR"

APP="$WORK_DIR/Payload/KeyTao.app"
KEYBOARD="$APP/PlugIns/KeyTaoKeyboard.appex"

test -x "$APP/KeyTao"
test -x "$KEYBOARD/KeyTaoKeyboard"

security cms -D -i "$APP_PROFILE" > "$WORK_DIR/app-profile.plist"
security cms -D -i "$KEYBOARD_PROFILE" > "$WORK_DIR/keyboard-profile.plist"

plutil -extract Entitlements xml1 \
  -o "$WORK_DIR/app-entitlements.plist" \
  "$WORK_DIR/app-profile.plist"
plutil -extract Entitlements xml1 \
  -o "$WORK_DIR/keyboard-entitlements.plist" \
  "$WORK_DIR/keyboard-profile.plist"

rm -rf "$APP/_CodeSignature" "$KEYBOARD/_CodeSignature"
cp "$APP_PROFILE" "$APP/embedded.mobileprovision"
cp "$KEYBOARD_PROFILE" "$KEYBOARD/embedded.mobileprovision"

codesign --force \
  --sign "$IDENTITY" \
  --entitlements "$WORK_DIR/keyboard-entitlements.plist" \
  --timestamp=none \
  --generate-entitlement-der \
  "$KEYBOARD"

codesign --force \
  --sign "$IDENTITY" \
  --entitlements "$WORK_DIR/app-entitlements.plist" \
  --timestamp=none \
  --generate-entitlement-der \
  "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"
ditto -c -k --sequesterRsrc --keepParent "$WORK_DIR/Payload" "$OUTPUT"

echo "Created $OUTPUT"
```

如果以后 IPA 中增加了动态 framework，也需要先签最深层 framework，再签 `.appex`，最后签 `.app`。

### 4. 验证签名结果

```bash
VERIFY_DIR="$(mktemp -d)"
ditto -x -k KeyTao-signed.ipa "$VERIFY_DIR"

codesign --verify --deep --strict --verbose=2 "$VERIFY_DIR/Payload/KeyTao.app"
codesign -d --entitlements :- "$VERIFY_DIR/Payload/KeyTao.app"
codesign -d --entitlements :- "$VERIFY_DIR/Payload/KeyTao.app/PlugIns/KeyTaoKeyboard.appex"
```

重点确认两层签名均有效、Bundle ID 正确，并且两个 entitlement 中的 App Group 完全一致。

## 安装已签名 IPA

### 1. 打开 Developer Mode

真机安装开发签名或 Ad Hoc 签名构建前，在设备中进入：

```text
设置 > 隐私与安全性 > 开发者模式
```

开启后按系统提示重启设备并再次确认。

### 2. 使用 Xcode 安装

1. 使用数据线或已配对的无线连接接入设备。
2. 打开 Xcode 的 **Window > Devices and Simulators**。
3. 选择设备，将 `KeyTao-signed.ipa` 拖入 Installed Apps 区域。

如果当前 Xcode 版本不接受 IPA，可先解包，再安装 `.app`：

```bash
INSTALL_DIR="$(mktemp -d)"
ditto -x -k KeyTao-signed.ipa "$INSTALL_DIR"
xcrun devicectl list devices
xcrun devicectl device install app \
  --device "YOUR_DEVICE_ID" \
  "$INSTALL_DIR/Payload/KeyTao.app"
```

### 3. 使用 Apple Configurator 安装

1. 从 Mac App Store 安装 Apple Configurator。
2. 连接并信任 iPhone 或 iPad。
3. 在 Apple Configurator 中选择设备。
4. 选择 **Add > Apps > Choose from my Mac**，选择已签名 IPA。

Development 或 Ad Hoc 包只能安装到 provisioning profile 已登记的设备。描述文件过期、设备不在列表中或任一层签名不匹配，系统都会拒绝安装。

## 启用 KeyTao 键盘

安装完成后：

1. 先打开一次 KeyTao 主 App。
2. 进入 **设置 > 通用 > 键盘 > 键盘 > 添加新键盘**。
3. 在第三方键盘中选择 **KeyTao 输入法**。
4. 再次进入 KeyTao 输入法设置，按需开启 **允许完全访问**。
5. 回到 KeyTao 主 App，安装方案并执行部署。
6. 打开任意普通文本框，长按地球键并选择 KeyTao 输入法。

KeyTao 需要“允许完全访问”才能通过 App Group 与主 App 共享已安装方案、主题和部署状态。不开放时，键盘只能使用扩展自身容器或包内的基础数据。

iOS 在密码、安全输入框和部分特殊键盘类型中会强制切回系统键盘，这是系统限制，不代表 KeyTao 安装失败。

## 常见问题

### 提示“无法安装”或无法验证完整性

通常由以下原因导致：

- IPA 仍未签名。
- 主 App 或键盘扩展缺少签名。
- 描述文件已过期。
- 设备 UDID 不在 Development 或 Ad Hoc profile 中。
- profile 的 App ID 与包内 Bundle ID 不匹配。

### App 能打开，但系统设置中没有 KeyTao 输入法

检查 IPA 是否包含并正确签名：

```text
Payload/KeyTao.app/PlugIns/KeyTaoKeyboard.appex
```

同时确认扩展的 profile 对应 `ink.rea.keytao-app.keyboard`，而不是主 App 的 profile。

### 可以选择 KeyTao，但方案或主题没有同步

依次检查：

1. 主 App 和键盘扩展的 entitlement 是否包含同一个 App Group。
2. 两个 provisioning profile 是否都授权该 App Group。
3. 是否为 KeyTao 输入法开启了“允许完全访问”。
4. 是否在主 App 中完成了方案安装和部署。

### 免费签名几天后失效

这是 Personal Team 的正常限制。重新连接 Xcode 构建安装即可；若要通过 TestFlight、Ad Hoc 或 App Store 稳定分发，需要加入 Apple Developer Program。

### 第三方签名工具安装后键盘异常

某些工具会自动改写 Bundle ID、移除 entitlement，或只签外层 App。KeyTao 包含键盘扩展和 App Group，这类改写很容易导致扩展无法加载或无法共享方案。不要把 Apple ID 密码、证书私钥或未加密的 `.p12` 上传给不可信服务。

## Apple 官方资料

- [免费账户与 Apple Developer Program 对比](https://developer.apple.com/support/compare-memberships/)
- [在模拟器或真机上运行 App](https://developer.apple.com/documentation/xcode/running-your-app-on-simulated-or-physical-devices)
- [在设备上开启 Developer Mode](https://developer.apple.com/documentation/xcode/enabling-developer-mode-on-a-device)
- [向已登记设备分发 App](https://developer.apple.com/documentation/xcode/distributing-your-app-to-registered-devices)
- [创建 Development provisioning profile](https://developer.apple.com/help/account/provisioning-profiles/create-a-development-provisioning-profile/)
- [创建 Ad Hoc provisioning profile](https://developer.apple.com/help/account/provisioning-profiles/create-an-ad-hoc-provisioning-profile/)
- [为 App ID 开启 App capabilities](https://developer.apple.com/help/account/identifiers/enable-app-capabilities/)
- [创建自定义键盘](https://developer.apple.com/documentation/uikit/creating-a-custom-keyboard)
- [配置自定义键盘的 Open Access](https://developer.apple.com/documentation/uikit/configuring-open-access-for-a-custom-keyboard)
- [TestFlight 概览](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview/)
- [Apple Code Signing Guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html)
