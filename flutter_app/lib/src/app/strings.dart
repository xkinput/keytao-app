// Verbatim copy from App.tsx, AndroidImeOnboarding.tsx and DebugTab.tsx.
// Keep dynamic copy here so both platform layouts share the same wording.
abstract final class AppStrings {
  static const title = 'KeyTao 键道';
  static const windowTitle = '键道';
  static const tagline = '键道，基于 librime 的跨平台原生输入法';
  static const boot = '键道正在启动...';
  static const appUpdate = 'KeyTao 有新版本可用';
  static const ime = '输入法';
  static const scheme = '方案';
  static const extension = '扩展安装';
  static const about = '关于';
  static const debug = '调试';
  static const schemeTitle = '键道方案';
  static const addonTitle = '附加方案';
  static const imeSettings = '输入法设置';
  static const macosIme = 'macOS 系统输入法';
  static const mobileKeyboard = '移动端键盘';
  static const customDirectory = '安装到自定义目录';
  static const customDirectoryDescription = '将方案安装到指定的输入法数据目录，安装完成后请手动重新部署输入法。';
  static const selectDirectory = '选择目录';
  static const reselectDirectory = '重新选择目录';
  static const openDirectory = '打开目录';
  static const opening = '打开中...';
  static const installNow = '立即安装';
  static const noDefaultCustom = '未检测到 default.custom.yaml，将自动创建';
  static const detectedSchemas = '检测到本地方案：';
  static const directoryEmpty = '目录为空';
  static const reading = '读取中...';
  static const customInstallDone = '安装完成，请手动重新部署输入法';
  static const easyEnglish = 'Easy English · 英文单词输入';
  static const wanxiang = '万象拼音 · 基础词库';
  static const wanxiangDescription =
      '下载约 35 MB，支持所有平台。不含额外语法模型和 Lua 扩展。安装后可在输入法方案菜单中选择“万象拼音”，保留现有方案和个人词库。';
  static const checking = '检测中';
  static const notInstalled = '未安装';
  static const installedNotDeployed = '已安装 · 未部署';
  static const install = '安装';
  static const installScheme = '安装方案';
  static const updateScheme = '更新方案';
  static const redeploy = '重新部署';
  static const uninstall = '卸载';
  static const checkingPermission = '检测权限...';
  static const installing = '安装中...';
  static const deploying = '部署中...';
  static const deploy = '部署';
  static const deployingLibrime = '正在部署 librime...';
  static const deployCurrent = '部署当前方案';
  static const installFirst = '请先安装方案';
  static const installKeytaoFirst = '请先安装键道方案';
  static const installMainFirst = '请先安装主方案';
  static const installEnglishTooltip = '安装并部署 Easy English';
  static const installWanxiangTooltip = '安装并部署万象拼音基础词库';
  static const noSchemeForDeploy = '未安装方案，请先安装';
  static const componentMissing = '系统组件缺失';
  static const schemeMissing = '未安装方案';
  static const pendingDeploy = '待部署';
  static const usable = '可使用';
  static const macosLoginWarning =
      '首次安装或更新 KeyTao 后，请注销并重新登录 macOS，再到系统设置中添加 KeyTao 输入法。';
  static const systemLocation = '系统位置：';
  static const directory = '目录：';
  static const selection = '选择：';
  static const checkLocal = '检查本地';
  static const checkUpdate = '检查新版本';
  static const changelog = '更新内容';
  static const releaseFailure = '获取版本信息失败：';
  static const localNotInstalled = '未安装，请先安装方案';
  static const dictManager = '词库管理器';
  static const testInput = '在此测试输入法…';
  static const operationLogs = '操作日志';
  static const clearOperationLogs = '清空日志';
  static const noLogs = '暂无日志';
  static const close = '关闭';
  static const cancel = '取消';
  static const apply = '应用';
  static const refresh = '刷新';
  static const embedded = '嵌入模式';
  static const embeddedDescription =
      '开启后在输入框内直接显示拼写字母；关闭时仅在候选窗口显示，完成后整体上屏（默认关闭）';
  static const toggleEmbedded = '切换嵌入模式';
  static const on = '开启';
  static const off = '关闭';
  static const englishMode = '英文模式';
  static const englishSchema = 'English 方案';
  static const englishAscii = 'ASCII 模式';
  static const englishHint = '请先在方案页安装附加方案 English';
  static const auto = '自动';
  static const light = '白天';
  static const dark = '夜间';
  static const followSystem = '跟随系统';
  static const fixedDark = '固定夜间';
  static const fixedLight = '固定白天';
  static const horizontal = '横排';
  static const vertical = '竖排';
  static const candidateFontSize = '候选字号';
  static const accentColor = '主题色';
  static const chooseAccentColor = '选择主题色';
  static const theme = '主题：';
  static const openTheme = '打开主题配置';
  static const gestures = '手势与时序';
  static const layout = '布局';
  static const longPressDelay = '长按延迟';
  static const keyboardHeight = '键盘高度';
  static const deleteSpeed = '删除速度';
  static const slow = '慢';
  static const standard = '标准';
  static const fast = '快';
  static const backspaceGesture = '退格滑动模式';
  static const selectThenDelete = '选中后删除';
  static const immediateDelete = '即时删除';
  static const flickKeys = '下拉输入角标符号';
  static const flickKeysHint = '与常驻数字行功能部分重叠';
  static const toggleFlickKeys = '切换 Flick keys';
  static const doubleSpacePeriod = '双击空格输入句号';
  static const doubleSpaceHint = '1100ms 内连按两次';
  static const swipeThreshold = '滑动判定阈值';
  static const floatingPortrait = '竖屏悬浮键盘';
  static const floatingLandscape = '横屏悬浮键盘';
  static const scale = '缩放';
  static const enterKey = '回车键';
  static const smartEnter = '智能判断';
  static const newline = '始终换行';
  static const config = '配置：';
  static const resetMobileKeyboard = '恢复移动端键盘默认设置';
  static const storagePermission = '需要文件访问权限';
  static const openStoragePermission = '开启文件访问权限';
  static const retryMigration = '重试数据迁移';
  static const androidOnboarding = '启用 Android 输入法';
  static const ready = '已就绪';
  static const needsSetup = '待配置';
  static const enableKeytao = '启用 KeyTao';
  static const enableDescription = '系统设置中打开 KeyTao 输入法开关。';
  static const selectKeytao = '切换到 KeyTao';
  static const selectDescription = '从系统输入法选择器中选中 KeyTao。';
  static const grantStorage = '授予文件访问权限';
  static const installKeytao = '安装键道方案';
  static const installDescription = '将所选键道方案写入用户目录，安装完成后还需要手动部署。';
  static const deployKeytao = '部署键道方案';
  static const deployDescription = '编译并启用已安装的方案，完成后输入法才可使用。';
  static const openSystemSettings = '打开系统设置';
  static const chooseKeytao = '选择 KeyTao';
  static const authorizeStorage = '授权文件访问';
  static const recheck = '重新检测';
  static const runtimeLogs = '运行日志';
  static const structuredRuntimeLogs = '结构化运行日志';
  static const imeLogs = 'keytao-ime (系统服务进程)';
  static const appLogs = 'keytao-app (当前界面进程)';
  static const logsEnabled = '已开启';
  static const logsDisabled = '已关闭';
  static const readingLogSettings = '读取设置中';
  static const logLevel = '级别';
  static const readLogSettingsFailure = '读取日志设置失败';
  static const readRuntimeLogFailure = '读取运行日志失败';
  static const readSystemLogsFailure = '读取系统日志失败';
  static const updateLogSettingsFailure = '更新日志设置失败';
  static const shareFailure = '分享失败';
  static const openLogDirectoryFailure = '打开日志目录失败';
  static const clearLogsFailure = '清空运行日志失败';
  static const copyPathFailure = '复制路径失败';
  static const logsCleared = '已清空运行日志';
  static const logPathCopied = '已复制日志目录路径';
  static const share = '分享';
  static const openLogDirectory = '打开日志目录';
  static const copyPath = '复制路径';
  static const clear = '清空';
  static const keytaoVersion = 'KeyTao 版本';
  static const librimeVersion = 'librime 版本';
  static const openccVersion = 'OpenCC 版本';
  static const platform = '平台';
  static const keytaoDirectory = '键道目录';
  static const easyEnglishRemoved = '[ADDON] Easy English 已卸载';
  static const wanxiangInstalled = '[ADDON] 万象拼音基础词库已安装并部署';
  static const wanxiangRemoved = '[ADDON] 万象拼音已卸载';

  // Existing Flutter fallbacks retained for callers of the original controller.
  static const systemFailure = '系统操作失败';
  static const installDone = '安装完成';
  static const installFailed = '安装失败';
  static const deployIncomplete = '部署未完成';
  static const deployDone = '部署完成';
  static const deployFailed = '部署失败';
  static const deployCheckFailed = '部署后检查未通过，请查看操作日志';
  static const finishSetupFirst = '请先完成输入法设置';
  static const englishNotReady = '英文方案未就绪';

  static String easyEnglishInstalled(String version) =>
      '[ADDON] Easy English 已安装并部署 v$version';

  static String currentVersion(String version) => '当前 v$version';
  static String changelogTitle(String release) => '$release 更新内容';
  static String deployedVersion(String version) => '已部署 v$version';
  static String itemCount(int count) => '$count 个项目';
  static String verifyFailures(int count) => '⚠ 有 $count 个文件校验失败';
  static String installLogCount(int count) => '安装日志（$count 条）';
  static String operationLogCount(int count) => '日志 ($count)';
  static String useColor(String hex) => '使用颜色 $hex';
  static String toggle(String label) => '切换$label';
  static String floatingScale(String label) => '$label缩放';
  static String permissionDescription(String? path) =>
      path == null || path.isEmpty
      ? '授权后 KeyTao 才能写入用户目录并让输入法读取方案。'
      : '授权后 KeyTao 才能写入 $path 并让输入法读取方案。';
  static String recentLines(int count) => '仅显示最近 $count 行';
  static String logStats(int count, String size) => '$count 个文件 · 共 $size';
  static String failure(String prefix, String error) => '$prefix：$error';
  static String savedLog(String path) => path.startsWith('Download/')
      ? '已保存到 下载/${path.substring('Download/'.length)}'
      : '下载目录保存失败，日志暂存于应用缓存：$path';
  static String localStatus({
    required bool installed,
    required bool deployed,
    String? version,
    String? label,
  }) {
    if (!installed) return localNotInstalled;
    final suffix =
        '${version == null || version.isEmpty ? '' : ' $version'}'
        '${label == null ? '' : ' · $label'}';
    return deployed ? '已安装并部署$suffix' : '方案已安装$suffix，请手动部署后使用';
  }
}
