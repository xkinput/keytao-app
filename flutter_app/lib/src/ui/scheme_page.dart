import 'dart:async' show unawaited;

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'git_scheme_dialog.dart';
import 'progress.dart';
import 'package_orphans.dart';
import 'package_schema_selection.dart';
import 'settings.dart' show SchemaSwitchesSection, showCloudPackageTransfer;
import 'theme.dart';
import 'user_words.dart';
import 'widgets.dart';

const _homeSchemeKeys = {'keytao', 'xmjd', 'txjx', 'keydo', 'snow_jiandao'};
const _homeAddonKeys = ['rime_ice', 'easy_en'];

void _showLocalSchemeImportDialog(
  BuildContext context,
  AppController c, {
  String? schemeKey,
}) {
  showFDialog<void>(
    context: context,
    builder: (dialogContext, _, animation) => AppDialog(
      title: schemeKey == null ? '添加本地方案' : '重新导入本地方案',
      animation: animation,
      actions: [
        ActionButton(
          AppStrings.cancel,
          onPress: () => Navigator.pop(dialogContext),
        ),
        ActionButton(
          '选择 .zip 压缩包',
          key: const ValueKey('import-local-zip-button'),
          icon: FLucideIcons.fileArchive,
          onPress: () {
            Navigator.pop(dialogContext);
            unawaited(
              c.importLocalScheme(
                schemeKey: schemeKey,
                fromArchive: true,
                selectSchemas: (inspection) async => context.mounted
                    ? showLocalImportSchemaSelection(context, inspection)
                    : null,
              ),
            );
          },
        ),
        ActionButton(
          '选择 Rime 文件夹',
          key: const ValueKey('import-local-folder-button'),
          primary: true,
          icon: FLucideIcons.folderOpen,
          onPress: () {
            Navigator.pop(dialogContext);
            unawaited(
              c.importLocalScheme(
                schemeKey: schemeKey,
                fromArchive: false,
                selectSchemas: (inspection) async => context.mounted
                    ? showLocalImportSchemaSelection(context, inspection)
                    : null,
              ),
            );
          },
        ),
      ],
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包，选择 .zip 压缩包后将自动解压。',
            style: dialogContext.theme.typography.body.sm,
          ),
          const SizedBox(height: 10),
          Text(
            '系统会自动过滤 build、sync、log 等运行目录。导入后的方案上限为 512 MB 和 10,000 个文件。'
            '${c.isMobile ? '手机端选取的 ZIP 文件另有 1 GB 上限。' : ''}',
            style: dialogContext.theme.typography.body.sm.copyWith(
              color: AppPalette.of(dialogContext).muted,
            ),
          ),
        ],
      ),
    ),
  );
}

void _showAddonSchemes(BuildContext context, AppController c) {
  showFDialog<void>(
    context: context,
    builder: (context, _, animation) => ListenableBuilder(
      listenable: c,
      builder: (context, _) {
        final addons = _addonCells(context, c, compact: true);
        return AppDialog(
          title: AppStrings.addonTitle,
          animation: animation,
          constraints: const BoxConstraints(minWidth: 280, maxWidth: 1120),
          actions: [
            ActionButton(
              AppStrings.close,
              onPress: () => Navigator.pop(context),
            ),
          ],
          child: SizedBox(
            width: 1056,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                const Padding(
                  padding: EdgeInsets.only(bottom: 8),
                  child: EyebrowLabel(AppStrings.schemesSectionTitle),
                ),
                HairlineGridSection(
                  key: const ValueKey('addon-schemes'),
                  minCellWidth: 180,
                  columnCount: c.isMobile ? 2 : null,
                  children: [
                    for (final entry in addons.entries)
                      if (!_homeAddonKeys.contains(entry.key)) entry.value,
                    for (final key in {
                      ...schemes.keys,
                      for (final package in c.installedPackages)
                        package.schemeKey,
                    })
                      if (!_homeSchemeKeys.contains(key) &&
                          key != 'xmjd-feiding' &&
                          !addons.containsKey(key) &&
                          !c.isLocalScheme(key))
                        HairlineGridCell(
                          padding: EdgeInsets.all(c.isMobile ? 8 : 12),
                          child: SchemePackageRow(
                            controller: c,
                            schemeKey: key,
                            compact: true,
                            showSource: false,
                          ),
                        ),
                  ],
                ),
                if (c.showWanxiangGrammar) ...[
                  const SizedBox(height: 16),
                  _ModelsSection(controller: c),
                ],
              ],
            ),
          ),
        );
      },
    ),
  );
}

void showChangelog(BuildContext context, AppController c, String body) {
  showFDialog<void>(
    context: context,
    builder: (context, _, animation) => AppDialog(
      title: c.changelogTitle!,
      animation: animation,
      actions: [
        ActionButton(
          AppStrings.close,
          primary: true,
          onPress: () => Navigator.pop(context),
        ),
      ],
      child: SizedBox(
        width: 560,
        child: SelectableText(
          body,
          style: context.theme.typography.body.sm.copyWith(
            fontFamily: 'monospace',
          ),
        ),
      ),
    ),
  );
}

class SchemePage extends StatefulWidget {
  const SchemePage({super.key, required this.controller});
  final AppController controller;

  @override
  State<SchemePage> createState() => _SchemePageState();
}

class _SchemePageState extends State<SchemePage> {
  bool _schemesExpanded = false;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onControllerChanged);
    _onControllerChanged();
  }

  @override
  void didUpdateWidget(covariant SchemePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_onControllerChanged);
      widget.controller.addListener(_onControllerChanged);
      _onControllerChanged();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onControllerChanged);
    super.dispose();
  }

  void _onControllerChanged() {
    if (!widget.controller.hasPendingSchemaSelectionPrompt) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final prompt = widget.controller.takePendingSchemaSelectionPrompt();
      if (prompt == null) return;
      unawaited(
        showPackageSchemaSelection(
          context,
          widget.controller,
          prompt.packageKey,
          activateOnConfirm: prompt.activateOnConfirm,
          convert: prompt.convert,
        ),
      );
    });
  }

  Future<void> _activate(String key) async {
    FocusManager.instance.primaryFocus?.unfocus();
    await widget.controller.activateSchemePackage(key);
  }

  Widget _package(String key) => SchemePackageRow(
    controller: widget.controller,
    schemeKey: key,
    showSource: false,
    onActivate: () => _activate(key),
  );

  Widget _schemeGridToggle(int hiddenCount) => TableCell(
    verticalAlignment: TableCellVerticalAlignment.fill,
    child: FTappable(
      semanticsLabel: _schemesExpanded ? '收起' : '展开全部方案',
      semanticsExpanded: _schemesExpanded,
      excludeSemantics: true,
      onPress: () => setState(() => _schemesExpanded = !_schemesExpanded),
      child: HairlineGridCell(
        child: Center(
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            spacing: 12,
            children: [
              if (!_schemesExpanded) const Icon(FLucideIcons.layers3, size: 24),
              Text(
                _schemesExpanded ? '收起' : '+$hiddenCount',
                style: context.theme.typography.body.sm.copyWith(
                  fontSize: 26,
                  fontWeight: FontWeight.w700,
                ),
              ),
              Icon(
                _schemesExpanded
                    ? FLucideIcons.chevronUp
                    : FLucideIcons.chevronDown,
                size: 20,
              ),
            ],
          ),
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final xmjdScheme = c.schemeFamily == 'xmjd'
        ? c.scheme
        : c.activeSchemeKey == 'xmjd-feiding'
        ? 'xmjd-feiding'
        : 'xmjd';
    final schemeKeys = <String>{
      for (final key in schemes.keys) key == 'xmjd' ? xmjdScheme : key,
      for (final package in c.installedPackages) package.schemeKey,
    };
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final mobile = c.isMobile;
    final palette = AppPalette.of(context);
    final cloudKeys = [
      for (final key in _homeSchemeKeys)
        if (!c.isLocalScheme(key)) key == 'xmjd' ? xmjdScheme : key,
    ];
    final addons = _addonCells(context, c, keys: _homeAddonKeys);
    final activeKey = c.installedPackages
        .where((package) => package.active)
        .map((package) => package.schemeKey)
        .firstOrNull;
    final localKeys = schemeKeys.where(c.isLocalScheme);
    final grid = HairlineGridSection(
      columnCount: 1,
      children: [
        if (c.deployWarning case final warning?)
          HairlineGridCell(
            child: StatusMessage(
              warning,
              icon: FLucideIcons.triangleAlert,
              color: warningColor(context),
            ),
          ),
        if (c.userWordsReport != null || c.userWordsReportError != null)
          UserWordsNotice(controller: c),
        Padding(
          padding: EdgeInsets.fromLTRB(
            desktop ? 24 : 0,
            8,
            desktop ? 24 : 0,
            12,
          ),
          child: Row(
            spacing: 8,
            children: [
              const EyebrowLabel('方案列表'),
              Expanded(
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  spacing: 8,
                  children: [
                    Flexible(child: _SchemeSource(controller: c)),
                    ActionButton(
                      AppStrings.addonTitle,
                      key: const ValueKey('addon-schemes-button'),
                      compact: !desktop,
                      onPress: () => _showAddonSchemes(context, c),
                    ),
                    FTooltip(
                      tipBuilder: (_, _) => const Text(AppStrings.checkUpdate),
                      child: FButton.icon(
                        variant: FButtonVariant.ghost,
                        onPress: c.schemeUpdatesLoading
                            ? null
                            : c.refreshSchemeUpdates,
                        child: const Icon(
                          FLucideIcons.refreshCw,
                          size: 16,
                          semanticLabel: AppStrings.checkUpdate,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        if (c.schemeUpdatesLoading) const FProgress(),
        LayoutBuilder(
          builder: (context, constraints) {
            const minCellWidth = 240.0;
            final cells = <Widget>[
              for (final key in cloudKeys)
                HairlineGridCell(
                  color: !mobile && key == activeKey ? palette.surface : null,
                  padding: mobile
                      ? const EdgeInsets.all(8)
                      : desktop
                      ? const EdgeInsets.all(20)
                      : null,
                  child: _package(key),
                ),
              for (final key in _homeAddonKeys) addons[key]!,
            ];
            final columns = desktop && constraints.hasBoundedWidth
                ? (constraints.maxWidth / minCellWidth).floor().clamp(
                    1,
                    cells.length,
                  )
                : 1;
            final collapsible =
                !mobile && columns > 1 && cells.length > columns * 2;
            final visibleCount = columns * 2 - 1;
            final hiddenCount = cells.length - visibleCount;
            if (collapsible && !_schemesExpanded) {
              final activeScheme = c.activeSchemeKey;
              var activeIndex = cloudKeys.indexOf(activeScheme);
              if (activeIndex < 0) {
                activeIndex = _homeAddonKeys.contains(activeScheme)
                    ? cloudKeys.length + _homeAddonKeys.indexOf(activeScheme)
                    : -1;
              }
              if (activeIndex >= visibleCount) {
                cells[visibleCount - 1] = cells[activeIndex];
              }
              cells.removeRange(visibleCount, cells.length);
            }
            return HairlineGridSection(
              key: const ValueKey('cloud-schemes'),
              minCellWidth: minCellWidth,
              columnCount: mobile ? 2 : null,
              children: [
                ...cells,
                if (collapsible) _schemeGridToggle(hiddenCount),
              ],
            );
          },
        ),
        HairlineGridCell(
          child: Section(
            key: const ValueKey('local-schemes'),
            title: AppStrings.localSchemeTitle,
            trailing: Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                ActionButton(
                  '从目录导入',
                  icon: FLucideIcons.plus,
                  onPress: c.busy
                      ? null
                      : () => _showLocalSchemeImportDialog(context, c),
                ),
                ActionButton(
                  '从 Git 添加',
                  key: const ValueKey('add-git-scheme-button'),
                  icon: FLucideIcons.download,
                  onPress: c.busy
                      ? null
                      : () => showGitSchemeDialog(context, c),
                ),
              ],
            ),
            children: [
              StatusMessage(
                '请导入纯 Rime 文件夹或 Rime 方案 .zip 压缩包（自动过滤 build 目录，上限 512 MB）',
                icon: FLucideIcons.info,
              ),
              HairlineGridSection(
                minCellWidth: 320,
                columnCount: mobile ? 2 : null,
                children: [
                  for (final key in localKeys)
                    HairlineGridCell(
                      color: !mobile && key == activeKey
                          ? palette.surface
                          : null,
                      padding: EdgeInsets.symmetric(
                        horizontal: mobile ? 8 : (desktop ? 20 : 12),
                        vertical: mobile ? 8 : 16,
                      ),
                      child: _package(key),
                    ),
                ],
              ),
              ActionRow(
                children: [
                  ActionButton(
                    '清理残留文件',
                    icon: FLucideIcons.trash2,
                    onPress: c.busy
                        ? null
                        : () => showPackageOrphans(context, c),
                  ),
                ],
              ),
            ],
          ),
        ),
        HairlineGridCell(
          child: Section(
            title: c.prefersLibjd ? '词库转换' : AppStrings.deploy,
            children: [
              if (c.packageError case final error?) ErrorMessage(error),
              if (c.schemeSyncError case final error?) ErrorMessage(error),
              if (!c.prefersLibjd)
                ValueRow(AppStrings.directory, c.activeSchemeDir),
              if (c.prefersLibjd)
                StatusMessage(c.isUsingLibjd ? '当前方案已转换' : '下载或导入方案后自动转换')
              else if (c.localStatusLine case final status?)
                StatusMessage(
                  '$status${c.localSchemaIds == null ? '' : ' ${c.localSchemaIds}'}',
                  icon: c.local!.deployed
                      ? FLucideIcons.circleCheck
                      : c.local!.installed
                      ? FLucideIcons.triangleAlert
                      : FLucideIcons.info,
                  color: c.local!.deployed
                      ? successColor(context)
                      : c.local!.installed
                      ? warningColor(context)
                      : null,
                ),
              ActionRow(
                children: [
                  if (c.canCancelDownload && !c.isManagingLibjd)
                    ActionButton(AppStrings.cancel, onPress: c.cancelDownload),
                  if (!c.prefersLibjd)
                    ActionButton(
                      c.isDeploying ? AppStrings.deploying : AppStrings.deploy,
                      icon: FLucideIcons.play,
                      tooltip: c.deployTooltip,
                      onPress: c.canDeploy ? c.deploy : null,
                    ),
                  ActionButton(
                    AppStrings.checkLocal,
                    icon: FLucideIcons.refreshCw,
                    onPress: c.busy ? null : c.refreshState,
                  ),
                  ActionButton(
                    AppStrings.openDirectory,
                    icon: FLucideIcons.folderOpen,
                    onPress: c.isAndroid || c.busy
                        ? null
                        : c.openActiveSchemeDirectory,
                  ),
                  if (!c.isMobile && !c.prefersLibjd)
                    ActionButton(
                      AppStrings.dictManager,
                      onPress: c.canOpenDictionaryManager
                          ? c.openDictionaryManager
                          : null,
                    ),
                ],
              ),
              if (c.installError case final error?) ErrorMessage(error),
              if (c.isInstalling || c.checkingStorage)
                InstallProgressView(
                  active: true,
                  progress: c.installProgress,
                  fraction: c.installFraction,
                  message: c.progress,
                ),
              if (!c.prefersLibjd && c.deploySteps.isNotEmpty)
                DeploySteps(controller: c),
              if (c.operationLogCount > 0)
                ActionRow(children: [OperationLogButton(controller: c)]),
            ],
          ),
        ),
        if (c.hasUsableBuild &&
            !c.usesLibjd(c.activeSchemeKey) &&
            (c.schemaSwitches.isNotEmpty || c.schemaSwitchError != null))
          HairlineGridCell(child: SchemaSwitchesSection(controller: c)),
        HairlineGridCell(
          child: const FTextField(
            hint: AppStrings.testInput,
            minLines: 3,
            maxLines: 5,
          ),
        ),
      ],
    );
    return AppPageBody(
      controller: c,
      page: AppPage.scheme,
      fullWidth: true,
      spacing: 0,
      children: [grid],
    );
  }
}

class _SchemeCardMenu extends StatefulWidget {
  const _SchemeCardMenu({super.key, required this.builder});

  final Widget Function(BuildContext, FPopoverController) builder;

  @override
  State<_SchemeCardMenu> createState() => _SchemeCardMenuState();
}

class _SchemeCardMenuState extends State<_SchemeCardMenu>
    with SingleTickerProviderStateMixin {
  late final _popover = FPopoverController(vsync: this);

  @override
  void dispose() {
    _popover.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => GestureDetector(
    behavior: HitTestBehavior.opaque,
    onSecondaryTap: _popover.show,
    child: widget.builder(context, _popover),
  );
}

class _SchemeMoreButton extends StatelessWidget {
  const _SchemeMoreButton({required this.onPress});

  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) => FTooltip(
    tipBuilder: (_, _) => const Text('更多操作'),
    child: FButton.icon(
      variant: FButtonVariant.outline,
      onPress: onPress,
      style: FButtonStyleDelta.delta(
        decoration: FVariants.all(
          ShapeDecoration(
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(999),
              side: BorderSide(
                color: AppPalette.of(context).hairline,
                width: 1.5,
              ),
            ),
          ),
        ),
        contentStyle: const FButtonContentStyleDelta.delta(
          constraints: BoxConstraints(minWidth: 44, minHeight: 44),
        ),
      ),
      child: const Icon(FLucideIcons.ellipsis, semanticLabel: '更多操作'),
    ),
  );
}

class _SchemeCardHeader extends StatelessWidget {
  const _SchemeCardHeader({required this.metadata, required this.heading});

  final Widget metadata;
  final Widget heading;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      ConstrainedBox(
        constraints: BoxConstraints(
          minHeight:
              MediaQuery.textScalerOf(context).scale(13) *
                  (context.theme.typography.body.sm.height ?? 1.2) +
              8,
        ),
        child: Align(child: metadata),
      ),
      const SizedBox(height: 4),
      ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 44),
        child: Align(alignment: Alignment.centerLeft, child: heading),
      ),
    ],
  );
}

class SchemePackageRow extends StatelessWidget {
  const SchemePackageRow({
    super.key,
    required this.controller,
    required this.schemeKey,
    this.onActivate,
    this.compact = false,
    this.showSource = true,
  });
  final AppController controller;
  final String schemeKey;
  final VoidCallback? onActivate;
  final bool compact;
  final bool showSource;

  @override
  Widget build(BuildContext context) =>
      _SchemeCardMenu(key: ValueKey(schemeKey), builder: _build);

  Future<void> _confirmCacheRelease(
    BuildContext context,
    String label,
    BigInt bytes, {
    required bool imeJd,
  }) async {
    final confirmed = await showFDialog<bool>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: label,
        animation: animation,
        actions: [
          ActionButton(
            AppStrings.cancel,
            onPress: () => Navigator.pop(context, false),
          ),
          ActionButton(
            '确认释放',
            primary: true,
            onPress: () => Navigator.pop(context, true),
          ),
        ],
        child: Text(
          '${controller.packageDisplayName(schemeKey)}可释放 ${formatLogBytes(bytes)}。'
          '此次释放仅涉及可重新生成的文件。'
          '${imeJd ? '切换到 ime-jd 后可重新转换词库。' : '下次在 App 中启用方案时将重新部署。'}',
        ),
      ),
    );
    if (confirmed == true) {
      await controller.releasePackageCache(schemeKey, imeJd: imeJd);
    }
  }

  Widget _build(BuildContext context, FPopoverController menu) {
    final c = controller;
    final palette = AppPalette.of(context);
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final mobile = c.isMobile;
    final package = c.installedPackageFor(schemeKey);
    final isLocal = package?.source == 'local';
    final update = c.schemeUpdateFor(schemeKey);
    final active = package?.active == true;
    final buildCache = active
        ? BigInt.zero
        : package?.buildCacheBytes ?? BigInt.zero;
    final imeJdCache = c.prefersLibjd
        ? BigInt.zero
        : package?.imeJdCacheBytes ?? BigInt.zero;
    final cacheBytes = buildCache + imeJdCache;
    final cacheSize = cacheBytes > BigInt.zero
        ? Text(
            '可释放 ${formatLogBytes(cacheBytes)}',
            style: context.theme.typography.body.sm.copyWith(
              color: palette.muted,
              fontSize: mobile ? 12 : 13,
            ),
          )
        : null;
    final local = c.local;
    final checksCurrentPackage =
        local?.installedScheme == null || local?.installedScheme == schemeKey;
    final inUse = c.prefersLibjd
        ? c.isUsingLibjd && c.activeSchemeKey == schemeKey
        : active && package!.inUse && checksCurrentPackage && c.hasUsableBuild;
    if ((package == null || !package.hasPackageZip) &&
        c.shouldFetchRemoteSchemeLogo(schemeKey)) {
      unawaited(c.ensureRemoteSchemeLogo(schemeKey));
    }
    final hasLogo = c.effectiveSchemeLogoPath(schemeKey) != null;
    final legacyDownload =
        active && !isLocal && !package!.hasPackageZip && update == null;
    final deployLabel = c.hasPackageDeployProgress
        ? c.packageDeployLabel(schemeKey)
        : c.isDeploying && c.deployingSchemeKey == schemeKey
        ? AppStrings.deploying
        : null;
    final deploying =
        deployLabel == AppStrings.packageDeploying ||
        deployLabel == AppStrings.deploying;
    final installing = c.installingSchemeKey == schemeKey;
    final changelogBody = schemeKey == 'keytao' ? c.changelogBody : null;
    final failures = c.deploySteps.where(
      (step) => step.state == DeployStepState.failed,
    );
    final hasPackageFailure = failures.any((step) => step.packageKey != null);
    final failedDeploySteps = failures.where(
      (step) =>
          step.packageKey == schemeKey ||
          step.packageKey == 'builtin/$schemeKey' ||
          (active && !hasPackageFailure && step.packageKey == null),
    );
    final deployFailed = failedDeploySteps.isNotEmpty;
    final needsDeploy =
        active &&
        !c.prefersLibjd &&
        !c.isDeploying &&
        (deployFailed || (checksCurrentPackage && local?.deployed == false));
    final installingDownload = installing && !c.isImportingLibjd;
    final installPercent = c.installProgress?.percent;
    final actions = ActionRow(
      keepPairTogether: mobile && !compact,
      children: [
        if (schemeKey == 'keytao' && showSource) _SchemeSource(controller: c),
        if (update != null && !installingDownload)
          ActionButton(
            '更新',
            compact: mobile && !compact,
            primary: !needsDeploy,
            icon: FLucideIcons.arrowUpCircle,
            onPress: c.busy ? null : () => c.updateSchemePackage(schemeKey),
          ),
        if (installingDownload)
          ActionButton(
            c.canCancelDownload
                ? (installPercent != null && installPercent > 0
                    ? '${AppStrings.cancel} $installPercent%'
                    : AppStrings.cancel)
                : (installPercent != null && installPercent > 0
                    ? '${AppStrings.installing} $installPercent%'
                    : AppStrings.installing),
            compact: mobile && !compact,
            icon: c.canCancelDownload ? FLucideIcons.x : null,
            onPress: c.canCancelDownload ? c.cancelDownload : null,
          )
        else if (deployLabel != null)
          ActionButton(deployLabel, primary: true, compact: mobile && !compact)
        else if (!isLocal && package == null)
          ActionButton(
            '下载',
            compact: mobile && !compact,
            primary: false,
            icon: FLucideIcons.download,
            onPress: c.busy ? null : () => c.installSchemePackage(schemeKey),
          )
        else if (mobile && inUse)
          const _InUseLabel()
        else if (package != null && (active || package.hasPackageZip))
          ActionButton(
            inUse
                ? '使用中'
                : needsDeploy
                ? AppStrings.deploy
                : '使用',
            primary: !inUse && (needsDeploy || update == null),
            compact: mobile && !compact,
            success: inUse,
            icon: inUse ? FLucideIcons.check : FLucideIcons.play,
            onPress: c.busy || inUse
                ? null
                : needsDeploy
                ? () => c.deploy(schemeKey: schemeKey)
                : onActivate ?? () => c.activateSchemePackage(schemeKey),
          )
        else if (package != null && !isLocal)
          ActionButton(
            '下载',
            compact: mobile && !compact,
            icon: FLucideIcons.download,
            onPress: c.busy ? null : () => c.downloadSchemePackage(schemeKey),
          ),
        if (changelogBody != null ||
            schemeRepositoryUrls[schemeKey] != null ||
            package != null)
          FPopoverMenu(
            control: FPopoverControl.managed(controller: menu),
            menuAnchor: Alignment.topRight,
            childAnchor: Alignment.bottomRight,
            menuBuilder: (_, popover, _) => [
              FItemGroup(
                children: [
                  if (changelogBody != null)
                    FItem(
                      title: const Text(AppStrings.changelog),
                      prefix: const Icon(FLucideIcons.scrollText),
                      onPress: () {
                        popover.hide();
                        showChangelog(context, c, changelogBody);
                      },
                    ),
                  if ((schemeRepositoryUrls[schemeKey] ?? package?.git?.repoUrl)
                      case final url?)
                    FItem(
                      title: const Text('仓库地址'),
                      prefix: const Icon(FLucideIcons.externalLink),
                      enabled: true,
                      onPress: () {
                        popover.hide();
                        c.open(url);
                      },
                    ),
                  if (package != null) ...[
                    if (legacyDownload)
                      FItem(
                        title: const Text('下载'),
                        prefix: const Icon(FLucideIcons.download),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                c.downloadSchemePackage(schemeKey);
                              },
                      ),
                    if (active && !c.prefersLibjd)
                      FItem(
                        title: const Text('重新部署'),
                        prefix: const Icon(FLucideIcons.refreshCw),
                        onPress: c.canDeploy
                            ? () {
                                popover.hide();
                                c.deploy(schemeKey: schemeKey);
                              }
                            : null,
                      ),
                    if (!c.prefersLibjd &&
                        (deployFailed ||
                            c.packageDeployError(schemeKey) != null))
                      FItem(
                        title: Text(isLocal ? '修复并重新部署' : '备份并重下修复'),
                        prefix: const Icon(FLucideIcons.wrench),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                c.repairSchemePackage(schemeKey);
                              },
                      ),
                    if (!c.prefersLibjd || package.hasPackageZip)
                      FItem(
                        title: Text(
                          c.prefersLibjd
                              ? selectConversionSchemaLabel
                              : AppStrings.selectSchemas,
                        ),
                        prefix: const Icon(FLucideIcons.listChecks),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                showPackageSchemaSelection(
                                  context,
                                  c,
                                  schemeKey,
                                  convert: c.prefersLibjd,
                                );
                              },
                      ),
                    if (!c.isMobile)
                      FItem(
                        title: const Text('打开方案目录'),
                        prefix: const Icon(FLucideIcons.folderOpen),
                        onPress: () {
                          popover.hide();
                          c.openSchemeDirectory(schemeKey);
                        },
                      ),
                    if (c.canToggleWanxiangGrammarOnPackages)
                      _wanxiangGrammarMenuItem(
                        context,
                        c,
                        schemeKey,
                        popover,
                        inAddonsDialog: compact,
                      ),
                    if (package.supportsUserWords)
                      FItem(
                        title: const Text('自造词'),
                        prefix: const Icon(FLucideIcons.bookOpen),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                openUserWords(context, c, schemeKey: schemeKey);
                              },
                      ),
                    if (active && c.accountUser != null)
                      FItem(
                        title: const Text('上传'),
                        prefix: const Icon(FLucideIcons.upload),
                        enabled: !c.busy && package.hasPackageZip,
                        onPress: c.busy || !package.hasPackageZip
                            ? null
                            : () {
                                popover.hide();
                                showCloudPackageTransfer(
                                  context,
                                  c,
                                  schemeKey: schemeKey,
                                );
                              },
                      ),
                    if (isLocal && package.git != null) ...[
                      FItem(
                        title: const Text('同步更新'),
                        prefix: const Icon(FLucideIcons.refreshCw),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                c.downloadSchemePackage(schemeKey);
                              },
                      ),
                      FItem(
                        title: const Text('配置 Git 来源'),
                        prefix: const Icon(FLucideIcons.slidersHorizontal),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                showGitSchemeDialog(
                                  context,
                                  c,
                                  existing: package,
                                );
                              },
                      ),
                    ] else if (isLocal)
                      FItem(
                        title: const Text('重新导入'),
                        prefix: const Icon(FLucideIcons.folderOpen),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                _showLocalSchemeImportDialog(
                                  context,
                                  c,
                                  schemeKey: schemeKey,
                                );
                              },
                      ),
                    if (c.accountUser != null)
                      FItem(
                        title: const Text('随账号同步'),
                        semanticsChecked: c.syncedSchemes.contains(schemeKey),
                        suffix: c.syncedSchemes.contains(schemeKey)
                            ? const Icon(FLucideIcons.check)
                            : null,
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                c.setSchemeAccountSync(
                                  schemeKey,
                                  !c.syncedSchemes.contains(schemeKey),
                                );
                              },
                      ),
                    for (final (imeJd, bytes, label) in [
                      (false, buildCache, '释放编译缓存'),
                      (true, imeJdCache, '释放 ime-jd 词典'),
                    ])
                      if (bytes > BigInt.zero)
                        FItem(
                          title: Text(label),
                          suffix: Text(formatLogBytes(bytes)),
                          onPress: c.busy
                              ? null
                              : () {
                                  popover.hide();
                                  _confirmCacheRelease(
                                    context,
                                    label,
                                    bytes,
                                    imeJd: imeJd,
                                  );
                                },
                        ),
                    FItem(
                      title: const Text(AppStrings.uninstall),
                      variant: FItemVariant.destructive,
                      prefix: const Icon(FLucideIcons.trash2),
                      enabled: !c.busy,
                      onPress: c.busy
                          ? null
                          : () {
                              popover.hide();
                              c.removeSchemePackage(schemeKey);
                            },
                    ),
                  ],
                ],
              ),
            ],
            builder: (_, popover, _) =>
                _SchemeMoreButton(onPress: popover.toggle),
          ),
      ],
    );
    final name = package?.displayName ?? schemeDisplayName(schemeKey);
    const xmjdVariants = {'xmjd': '顶功', 'xmjd-feiding': '非顶'};
    final xmjdVariant = xmjdVariants[schemeKey];
    Widget titleButton(VoidCallback onPress) => Semantics(
      selected: c.scheme == schemeKey,
      hint: xmjdVariant == null ? null : '切换顶功或非顶',
      child: FButton(
        variant: FButtonVariant.ghost,
        mainAxisSize: MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.start,
        style: const FButtonStyleDelta.delta(
          contentStyle: FButtonContentStyleDelta.delta(
            padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
          ),
        ),
        onPress: c.busy ? null : onPress,
        onSecondaryPress: menu.show,
        suffix: xmjdVariant == null
            ? null
            : const Icon(FLucideIcons.chevronDown, size: 14),
        child: Flexible(
          child: Text(
            xmjdVariant != null && package?.displayName == null
                ? schemes['xmjd']!
                : name,
            semanticsLabel: name,
            maxLines: mobile ? 1 : null,
            overflow: mobile ? TextOverflow.ellipsis : null,
            style: context.theme.typography.body.sm.copyWith(
              fontSize: compact || mobile
                  ? 16
                  : desktop
                  ? 22
                  : 24,
              height: 1.1,
              fontWeight: FontWeight.w700,
              letterSpacing: compact ? 0 : -.34,
            ),
          ),
        ),
      ),
    );
    final title = xmjdVariant == null
        ? titleButton(() => c.selectScheme(schemeKey))
        : FPopoverMenu(
            menuAnchor: Alignment.topLeft,
            childAnchor: Alignment.bottomLeft,
            menuBuilder: (_, popover, _) => [
              FItemGroup(
                enabled: !c.busy,
                children: [
                  for (final MapEntry(key: key, value: label)
                      in xmjdVariants.entries)
                    FItem(
                      title: Text(label),
                      semanticsChecked: key == schemeKey,
                      suffix: key == schemeKey
                          ? const Icon(FLucideIcons.check)
                          : null,
                      onPress: () {
                        popover.hide();
                        if (key != schemeKey) c.selectScheme(key);
                      },
                    ),
                ],
              ),
            ],
            builder: (_, popover, _) => titleButton(popover.toggle),
          );
    final state =
        !c.prefersLibjd && !c.isDeploying && (deployFailed || needsDeploy)
        ? AppBadge(
            deployFailed
                ? AppStrings.deployFailed
                : c.autoDeployQueued && active
                ? AppStrings.packageDeployQueued
                : AppStrings.pendingDeploy,
            background: deployFailed
                ? palette.error.withValues(alpha: .12)
                : null,
            foreground: deployFailed ? palette.error : null,
          )
        : c.prefersLibjd &&
              package?.hasPackageZip == true &&
              !inUse &&
              update == null
        ? AppBadge(c.usesImportedLibjd(schemeKey) ? '已转换' : '待转换')
        : inUse || deployLabel != null || (package != null && update == null)
        ? null
        : StatusPill(
            update != null ? '可更新' : '未安装',
            color: update != null ? ChromeAccent.of(context) : palette.muted,
          );
    final uninstalledInHeader =
        !compact && package == null && update == null && !deployFailed;
    final metadata = Wrap(
      spacing: 6,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        ?cacheSize,
        if (package?.version?.trim().isNotEmpty == true)
          Text(
            package!.version!,
            style: context.theme.typography.body.sm.copyWith(
              color: palette.muted,
            ),
          ),
        if (package == null && schemeAssets[schemeKey] != null)
          Text(
            schemeAssets[schemeKey]!,
            style: context.theme.typography.body.sm.copyWith(
              color: palette.muted,
            ),
          ),
        if (update != null)
          Text(
            '→ ${update.latestVersion}',
            style: context.theme.typography.body.sm.copyWith(
              color: palette.muted,
            ),
          ),
        if (compact && state != null) state,
      ],
    );
    final heading = compact
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                spacing: 8,
                children: [
                  if (hasLogo)
                    SchemaLogo(
                      controller: c,
                      size: 24,
                      schemeKey: schemeKey,
                      appLogoFallback: false,
                    ),
                  Flexible(child: title),
                ],
              ),
              metadata,
            ],
          )
        : Row(
            spacing: 6,
            children: [
              if (hasLogo)
                SchemaLogo(
                  controller: c,
                  size: mobile ? 24 : (desktop ? 32 : 30),
                  schemeKey: schemeKey,
                  appLogoFallback: false,
                ),
              Flexible(child: title),
              if (!mobile && !uninstalledInHeader) ?state,
            ],
          );
    return Column(
      key: ValueKey('scheme-package-$schemeKey'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (compact)
          heading
        else
          _SchemeCardHeader(
            heading: heading,
            metadata: Row(
              children: [
                EyebrowLabel(
                  inUse && !mobile
                      ? '在用'
                      : package?.git != null
                      ? 'Git'
                      : isLocal
                      ? '本地'
                      : '云端',
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Wrap(
                    alignment: WrapAlignment.end,
                    spacing: 6,
                    runSpacing: 4,
                    children: [
                      ?cacheSize,
                      if (package?.version?.trim().isNotEmpty == true)
                        Text(
                          package!.version!,
                          style: context.theme.typography.body.sm.copyWith(
                            color: inUse && !mobile
                                ? palette.successForeground
                                : palette.muted,
                            fontSize: mobile ? 12 : 13,
                            fontWeight: inUse && !mobile
                                ? FontWeight.w600
                                : FontWeight.w400,
                          ),
                        ),
                      if (uninstalledInHeader) ?state,
                      if (update != null)
                        Text(
                          '→ ${update.latestVersion}',
                          style: context.theme.typography.body.sm.copyWith(
                            color: palette.muted,
                            fontSize: mobile ? 12 : 13,
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        SizedBox(height: mobile ? 6 : 10),
        if (!compact && !desktop && !mobile && active && !showSource)
          Row(
            spacing: 8,
            children: [
              for (final action in actions.children)
                if (action is ActionButton)
                  Expanded(child: action)
                else
                  SizedBox(width: 44, child: action),
            ],
          )
        else
          actions,
        if (mobile && !compact && !uninstalledInHeader && state != null) ...[
          const SizedBox(height: 6),
          Align(alignment: Alignment.centerLeft, child: state),
        ],
        if (c.packageDeployError(schemeKey) case final error?) ...[
          const SizedBox(height: 8),
          ErrorMessage(error),
        ],
        if (!deploying && c.packageDeployError(schemeKey) == null)
          for (final step in failedDeploySteps) ...[
            const SizedBox(height: 8),
            ErrorMessage(step.message),
          ],
        if (installing && c.isImportingLibjd) ...[
          const SizedBox(height: 8),
          InstallProgressView(
            active: true,
            progress: c.installProgress,
            fraction: c.installFraction,
            message: c.progress,
          ),
        ] else if (c.scheme == schemeKey && c.releaseLoading) ...[
          const SizedBox(height: 8),
          const FProgress(),
        ],
        if (c.scheme == schemeKey && c.releaseError != null) ...[
          const SizedBox(height: 8),
          ErrorMessage(c.releaseError!),
        ],
      ],
    );
  }
}

class _InUseLabel extends StatelessWidget {
  const _InUseLabel();

  @override
  Widget build(BuildContext context) => Text(
    '使用中',
    style: context.theme.typography.body.xs.copyWith(
      color: AppPalette.of(context).muted,
      fontWeight: FontWeight.w600,
    ),
  );
}

class _SchemeSource extends StatelessWidget {
  const _SchemeSource({required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Semantics(
      key: const ValueKey('scheme-source'),
      label: '下载来源',
      enabled: !c.busy,
      child: FPopoverMenu(
        menuAnchor: Alignment.topRight,
        childAnchor: Alignment.bottomRight,
        menuBuilder: (_, popover, _) => [
          FItemGroup(
            enabled: !c.busy,
            children: [
              for (final entry in downloadSources.entries)
                FItem(
                  title: Text(entry.value),
                  semanticsChecked: c.source == entry.key,
                  suffix: c.source == entry.key
                      ? const Icon(FLucideIcons.check)
                      : null,
                  onPress: c.busy
                      ? null
                      : () {
                          popover.hide();
                          c.selectSource(entry.key);
                        },
                ),
            ],
          ),
        ],
        builder: (_, popover, _) => FButton(
          variant: FButtonVariant.ghost,
          mainAxisSize: MainAxisSize.min,
          onPress: popover.toggle,
          child: Flexible(
            child: Text(
              '来源 · ${downloadSources[c.source] ?? c.source}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ),
      ),
    );
  }
}

class SchemeSelection extends StatelessWidget {
  const SchemeSelection({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        LayoutBuilder(
          builder: (context, constraints) {
            final columns = constraints.maxWidth >= 560 ? 4 : 2;
            final width = (constraints.maxWidth - (columns - 1) * 8) / columns;
            return Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final entry in schemes.entries)
                  SizedBox(
                    width: width,
                    child: FTooltip(
                      tipBuilder: (_, _) => Text(
                        !c.isAndroid && c.schemeFamily == entry.key
                            ? c.selectedSchemeAsset
                            : schemeAssets[entry.key]!,
                      ),
                      child: Semantics(
                        selected: c.schemeFamily == entry.key,
                        child: FButton(
                          variant: c.schemeFamily == entry.key
                              ? FButtonVariant.primary
                              : FButtonVariant.outline,
                          onPress: c.busy || (c.isAndroid && c.releaseLoading)
                              ? null
                              : () => c.selectSchemeFamily(entry.key),
                          child: Flexible(
                            child: Column(
                              children: [
                                Text(entry.value),
                                const SizedBox(height: 3),
                                Text(
                                  c.schemeFamily == entry.key
                                      ? c.releaseVersion ??
                                            c.selectedSchemeAsset
                                      : schemeAssets[entry.key]!,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: const TextStyle(fontSize: 11),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
        // 星猫键道发了顶功 / 非顶功两个包，选中后再列一次具体文件。
        if (c.schemeFamily == 'xmjd') ...[
          const SizedBox(height: 12),
          ChoiceSetting<String>(
            label: '具体文件',
            value: c.scheme,
            options: xmjdVariantSchemes,
            hint: '两包词库内容相同，只差顶功开关',
            onChanged: c.busy || (c.isAndroid && c.releaseLoading)
                ? null
                : c.selectScheme,
          ),
        ],
        const SizedBox(height: 10),
        StatusMessage(c.schemeSelectionText, icon: FLucideIcons.download),
      ],
    );
  }
}

class VersionPicker extends StatelessWidget {
  const VersionPicker({
    super.key,
    required this.controller,
    this.showScheme = false,
  });
  final AppController controller;
  final bool showScheme;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    final sources = c.sourceVersions;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (c.scheme == 'keytao' && sources.length > 1) ...[
          ChoiceSetting<String>(
            label: '下载来源',
            value: c.source,
            options: {
              for (final entry in sources.entries)
                entry.key: '${downloadSources[entry.key]} ${entry.value}',
            },
            onChanged: c.selectSource,
          ),
          const SizedBox(height: 12),
        ],
        SettingRow(
          showScheme ? '${schemeDisplayName(c.scheme)} · 最新版本' : '最新版本',
          child: ActionRow(
            children: [
              if (c.scheme == 'keytao' &&
                  sources.length == 1 &&
                  sources.containsKey('github'))
                ActionButton(
                  '${downloadSources[sources.keys.single]} ${sources.values.single}',
                  onPress: () => c.selectSource(sources.keys.single),
                )
              else
                FBadge(
                  child: Text(
                    c.scheme != 'keytao' && c.releaseVersion != null
                        ? '${c.releaseVersion} · ${c.selectedSchemeAsset}'
                        : c.latestRelease?.github == null
                        ? c.latestRelease?.version ?? '—'
                        : c.releaseVersion ?? '—',
                  ),
                ),
              FTooltip(
                tipBuilder: (_, _) => const Text(AppStrings.checkUpdate),
                child: FButton.icon(
                  variant: FButtonVariant.ghost,
                  onPress: c.releaseLoading ? null : c.refreshRelease,
                  child: const Icon(
                    FLucideIcons.refreshCw,
                    size: 16,
                    semanticLabel: AppStrings.checkUpdate,
                  ),
                ),
              ),
              if (c.changelogBody != null)
                ActionButton(
                  AppStrings.changelog,
                  tooltip: AppStrings.changelog,
                  onPress: () => showChangelog(context, c, c.changelogBody!),
                ),
            ],
          ),
        ),
        if (c.releaseLoading) ...[const SizedBox(height: 8), const FProgress()],
        if (c.releaseError case final error?) ...[
          const SizedBox(height: 8),
          ErrorMessage(error),
        ],
      ],
    );
  }
}

class AddonCard extends StatelessWidget {
  const AddonCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => Column(
    mainAxisSize: MainAxisSize.min,
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      const Padding(
        padding: EdgeInsets.only(bottom: 8),
        child: EyebrowLabel(AppStrings.schemesSectionTitle),
      ),
      HairlineGridSection(
        minCellWidth: 320,
        columnCount: controller.isMobile ? 2 : null,
        children: _addonCells(context, controller).values.toList(),
      ),
      if (controller.showWanxiangGrammar) ...[
        const SizedBox(height: 16),
        _ModelsSection(controller: controller),
      ],
    ],
  );
}

FItemMixin _wanxiangGrammarMenuItem(
  BuildContext context,
  AppController c,
  String packageKey,
  FPopoverController popover, {
  required bool inAddonsDialog,
}) {
  final activeId = c.grammarModelForPackage(packageKey);
  final activeLabel =
      c.grammarModelShortName(activeId) ?? AppStrings.grammarModelOff;
  final models = c.effectiveGrammarModels;
  return FSubmenuItem(
    key: ValueKey('enable-wanxiang-grammar-$packageKey'),
    title: const Text(AppStrings.grammarModelMenu),
    prefix: const Icon(FLucideIcons.cpu),
    suffix: Row(
      mainAxisSize: MainAxisSize.min,
      spacing: 4,
      children: [
        Text(
          activeLabel,
          style: context.theme.typography.body.xs.copyWith(
            color: activeId != null
                ? ChromeAccent.of(context)
                : context.theme.colors.mutedForeground,
          ),
        ),
        const Icon(FLucideIcons.chevronRight, size: 14),
      ],
    ),
    enabled: !c.busy,
    submenuMaxHeight: 340,
    submenu: [
      FItemGroup(
        enabled: !c.busy,
        children: [
          FItem(
            key: ValueKey('grammar-model-option-$packageKey-off'),
            title: const Text(AppStrings.grammarModelOff),
            semanticsChecked: activeId == null,
            suffix: activeId == null ? const Icon(FLucideIcons.check) : null,
            onPress: c.busy
                ? null
                : () {
                    popover.hide();
                    unawaited(c.setPackageGrammarModel(packageKey, null));
                  },
          ),
          for (final model in models)
            FItem(
              key: ValueKey('grammar-model-option-$packageKey-${model.id}'),
              title: Text(model.name),
              subtitle: Text(
                model.description,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
              semanticsChecked: activeId == model.id,
              suffix: activeId == model.id
                  ? const Icon(FLucideIcons.check)
                  : !model.installed
                  ? Text(
                      AppStrings.notInstalled,
                      style: context.theme.typography.body.xs.copyWith(
                        color: context.theme.colors.mutedForeground,
                      ),
                    )
                  : null,
              onPress: c.busy
                  ? null
                  : () {
                      popover.hide();
                      if (!model.installed) {
                        if (!inAddonsDialog) {
                          _showAddonSchemes(context, c);
                        } else {
                          c.showToast(AppStrings.wanxiangGrammarNeedDownload);
                        }
                        return;
                      }
                      unawaited(c.setPackageGrammarModel(packageKey, model.id));
                    },
            ),
        ],
      ),
    ],
  );
}

class _ModelsSection extends StatelessWidget {
  const _ModelsSection({required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final mobile = c.isMobile;
    final models = c.effectiveGrammarModels;
    return Column(
      key: const ValueKey('addon-models'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Padding(
          padding: EdgeInsets.only(bottom: 8),
          child: EyebrowLabel(AppStrings.modelsTitle),
        ),
        HairlineGridSection(
          minCellWidth: 260,
          columnCount: mobile ? 1 : null,
          children: [
            for (final model in models) _modelCard(context, c, model, mobile),
          ],
        ),
        if (c.wanxiangGrammarError case final error?) ...[
          const SizedBox(height: 8),
          ErrorMessage(error),
        ],
      ],
    );
  }

  Widget _modelCard(
    BuildContext context,
    AppController c,
    dynamic model,
    bool mobile,
  ) {
    final id = model.id as String;
    final name = model.name as String;
    final description = model.description as String;
    final repoUrl = model.repoUrl as String;
    final installed = model.installed as bool;
    final size = model.size as BigInt;
    final isWanxiangHans = id == AppController.wanxiangHansModelId;
    final managing = c.isManagingGrammarModel(id);
    final sizeText = isWanxiangHans
        ? c.wanxiangGrammarSize
        : formatLogBytes(size);
    final pct = c.wanxiangGrammarProgress?.percent ?? 0;
    final canCancel = c.canCancelWanxiangGrammarDownload;
    return HairlineGridCell(
      key: isWanxiangHans
          ? const ValueKey('wanxiang-grammar')
          : ValueKey('grammar-model-$id'),
      padding: EdgeInsets.all(mobile ? 10 : 14),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 8,
            runSpacing: 4,
            children: [
              Text(
                name,
                style: context.theme.typography.body.sm.copyWith(
                  fontWeight: FontWeight.w700,
                ),
              ),
              Text(
                sizeText,
                style: context.theme.typography.body.xs.copyWith(
                  color: context.theme.colors.mutedForeground,
                ),
              ),
              if (installed)
                StatusPill('已安装', color: AppPalette.of(context).success),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            description,
            style: context.theme.typography.body.xs.copyWith(
              color: context.theme.colors.mutedForeground,
            ),
          ),
          const SizedBox(height: 10),
          ActionRow(
            children: [
              if (managing)
                ActionButton(
                  canCancel
                      ? (pct > 0
                          ? '${AppStrings.cancel} $pct%'
                          : AppStrings.cancel)
                      : (pct > 0
                          ? '${AppStrings.deploying} $pct%'
                          : AppStrings.deploying),
                  icon: canCancel ? FLucideIcons.x : null,
                  onPress: canCancel ? c.cancelWanxiangGrammarDownload : null,
                )
              else
                ActionButton(
                  installed ? AppStrings.uninstall : AppStrings.download,
                  icon: installed
                      ? FLucideIcons.trash2
                      : FLucideIcons.download,
                  onPress: c.canManageGrammarModel(model)
                      ? () => c.manageGrammarModel(id, !installed)
                      : null,
                ),
              FPopoverMenu(
                menuAnchor: Alignment.topRight,
                childAnchor: Alignment.bottomRight,
                menuBuilder: (_, popover, _) => [
                  FItemGroup(
                    children: [
                      FItem(
                        title: const Text('仓库地址'),
                        prefix: const Icon(FLucideIcons.externalLink),
                        onPress: () {
                          popover.hide();
                          c.open(repoUrl);
                        },
                      ),
                    ],
                  ),
                ],
                builder: (_, popover, _) =>
                    _SchemeMoreButton(onPress: popover.toggle),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

Map<String, Widget> _addonCells(
  BuildContext context,
  AppController c, {
  bool compact = false,
  Iterable<String>? keys,
}) {
  final allowedKeys = keys?.toSet();
  Widget addon({
    required String key,
    required String packageKey,
    required String title,
    required String repositoryUrl,
    required String status,
    required String label,
    required String tooltip,
    required bool showInstall,
    required VoidCallback? install,
    required bool installed,
    required VoidCallback? uninstall,
    required bool active,
    required int? progressPercent,
    required String? error,
    Widget? variant,
    Widget? extra,
  }) {
    final overlay = packageKey == 'easy_en';
    final currentPackage = c.local?.installedScheme;
    final englishEnabled = c.local?.easyEnglishEnabled;
    final isolatedPackage = c.isolatedPackages
        .where((package) => package.packageKey == packageKey)
        .firstOrNull;
    final cloudCard = !compact && _homeAddonKeys.contains(packageKey);
    final rime = c.rimeAddons[packageKey];
    final info = overlay ? c.addon : rime?.status;
    final hasUpdate = overlay ? c.addonHasUpdate : rime?.hasUpdate == true;
    final inUse = c.prefersLibjd && !overlay
        ? c.isUsingLibjd && c.activeSchemeKey == packageKey
        : isolatedPackage?.active == true &&
              (isolatedPackage?.ready == true ||
                  (currentPackage == packageKey && c.hasUsableBuild));
    final importing = c.isImportingLibjd && c.installingSchemeKey == packageKey;
    final uninstalled = !inUse && !installed && info != null;
    final deployKey = overlay && active
        ? currentPackage ?? packageKey
        : packageKey;
    final deployLabel = c.hasPackageDeployProgress
        ? c.packageDeployLabel(deployKey)
        : c.deployingSchemeKey == deployKey && (c.isDeploying || active)
        ? AppStrings.deploying
        : null;
    final cardError = c.packageDeployError(packageKey) ?? error;
    final palette = AppPalette.of(context);
    final mobile = c.isMobile;
    final titleStyle = context.theme.typography.body.sm.copyWith(
      fontSize: mobile
          ? 16
          : compact
          ? 18
          : MediaQuery.sizeOf(context).width >= 720
          ? 22
          : 24,
      height: 1.1,
      fontWeight: FontWeight.w700,
      letterSpacing: cloudCard ? -.34 : null,
    );
    if (c.shouldFetchRemoteSchemeLogo(
      packageKey,
      repoUrl: repositoryUrl,
      addonInstalled: installed,
    )) {
      unawaited(
        c.ensureRemoteSchemeLogo(
          packageKey,
          repoUrl: repositoryUrl,
          addonInstalled: installed,
        ),
      );
    }
    final hasAddonLogo = c.effectiveSchemeLogoPath(packageKey) != null;
    final cardState =
        cloudCard &&
            !inUse &&
            !uninstalled &&
            (info == null ||
                hasUpdate ||
                (!c.prefersLibjd && !info.deployed) ||
                (c.prefersLibjd && !overlay))
        ? StatusPill(
            info == null
                ? AppStrings.checking
                : hasUpdate
                ? '可更新'
                : c.prefersLibjd && !overlay
                ? (c.usesImportedLibjd(packageKey) ? '已转换' : '待转换')
                : AppStrings.pendingDeploy,
            color: hasUpdate
                ? ChromeAccent.of(context)
                : installed
                ? palette.success
                : palette.muted,
          )
        : null;
    final titleText = Text(
      title,
      maxLines: cloudCard ? 1 : (mobile ? 2 : null),
      overflow: cloudCard || mobile ? TextOverflow.ellipsis : null,
      strutStyle: cloudCard && !mobile
          ? StrutStyle.fromTextStyle(titleStyle, forceStrutHeight: true)
          : null,
      style: cloudCard && !mobile && title.length > 10
          ? titleStyle.copyWith(fontSize: 18)
          : titleStyle,
    );
    final name = hasAddonLogo
        ? Row(
            spacing: 6,
            mainAxisSize: MainAxisSize.min,
            children: [
              SchemaLogo(
                controller: c,
                size: cloudCard && !mobile
                    ? (MediaQuery.sizeOf(context).width >= 720 ? 32 : 30)
                    : 24,
                schemeKey: packageKey,
                appLogoFallback: false,
              ),
              Flexible(child: titleText),
            ],
          )
        : titleText;
    return HairlineGridCell(
      key: ValueKey(key),
      padding: mobile
          ? const EdgeInsets.all(8)
          : compact
          ? const EdgeInsets.all(12)
          : cloudCard && MediaQuery.sizeOf(context).width >= 720
          ? const EdgeInsets.all(20)
          : null,
      child: _SchemeCardMenu(
        builder: (context, menu) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (cloudCard) ...[
              _SchemeCardHeader(
                metadata: Row(
                  children: [
                    EyebrowLabel(inUse && !mobile ? '在用' : '云端'),
                    const SizedBox(width: 8),
                    Expanded(
                      child: uninstalled
                          ? Align(
                              alignment: Alignment.centerRight,
                              child: StatusPill('未安装', color: palette.muted),
                            )
                          : Text(
                              installed ? info!.version : '',
                              textAlign: TextAlign.end,
                              style: context.theme.typography.body.sm.copyWith(
                                color: inUse && !mobile
                                    ? palette.successForeground
                                    : palette.muted,
                                fontSize: mobile ? 12 : 13,
                                fontWeight: inUse && !mobile
                                    ? FontWeight.w600
                                    : FontWeight.w400,
                              ),
                            ),
                    ),
                  ],
                ),
                heading: Row(
                  spacing: 6,
                  children: [
                    Flexible(child: name),
                    if (!mobile) ?cardState,
                  ],
                ),
              ),
              SizedBox(height: mobile ? 6 : 10),
            ] else ...[
              name,
              const SizedBox(height: 4),
              Text(
                c.hasPackageDeployProgress ? deployLabel ?? status : status,
                style: context.theme.typography.body.xs.copyWith(
                  color: context.theme.colors.mutedForeground,
                ),
              ),
              if (variant != null) ...[const SizedBox(height: 12), variant],
              const SizedBox(height: 12),
            ],
            ActionRow(
              keepPairTogether: mobile && cloudCard,
              children: [
                if (overlay && installed)
                  ActionButton(
                    englishEnabled == true ? '从当前方案移除' : '加入当前方案',
                    compact: mobile && cloudCard,
                    primary: englishEnabled != true,
                    onPress:
                        c.busy ||
                            currentPackage == null ||
                            englishEnabled == null
                        ? null
                        : () => c.setPackageEasyEnglish(
                            currentPackage,
                            !englishEnabled,
                          ),
                  ),
                if (active)
                  ActionButton(
                    (progressPercent ?? 0) > 0
                        ? '下载中 $progressPercent%'
                        : AppStrings.installing,
                    compact: mobile && cloudCard,
                    onPress: null,
                  )
                else if (showInstall &&
                    (overlay
                        ? !installed || hasUpdate
                        : label != AppStrings.deploy))
                  ActionButton(
                    cloudCard ? (installed ? '更新' : '下载') : label,
                    compact: mobile && cloudCard,
                    primary: installed,
                    icon: FLucideIcons.download,
                    tooltip: overlay ? null : tooltip,
                    onPress: install,
                  ),
                if (!active && !overlay && inUse && mobile && deployLabel == null)
                  const _InUseLabel()
                else if (!active && !overlay && (installed || deployLabel != null))
                  ActionButton(
                    deployLabel ?? (inUse ? '使用中' : '使用'),
                    compact: mobile && cloudCard,
                    primary: !inUse,
                    success: inUse && deployLabel == null,
                    icon: inUse ? FLucideIcons.check : FLucideIcons.play,
                    onPress: c.busy || inUse || deployLabel != null
                        ? null
                        : () => c.switchPackage(packageKey),
                  ),
                FPopoverMenu(
                  control: FPopoverControl.managed(controller: menu),
                  menuAnchor: Alignment.topRight,
                  childAnchor: Alignment.bottomRight,
                  menuBuilder: (_, popover, _) => [
                    FItemGroup(
                      children: [
                        if (packageKey == 'wanxiang')
                          FSubmenuItem(
                            key: const ValueKey('wanxiang-variant-submenu'),
                            title: const Text('版本切换'),
                            prefix: const Icon(FLucideIcons.layers),
                            suffix: Row(
                              mainAxisSize: MainAxisSize.min,
                              spacing: 4,
                              children: [
                                Text(
                                  AppStrings
                                          .wanxiangVariants[c
                                              .selectedWanxiangVariant] ??
                                      c.selectedWanxiangVariant,
                                  style: context.theme.typography.body.xs
                                      .copyWith(
                                        color: context
                                            .theme
                                            .colors
                                            .mutedForeground,
                                      ),
                                ),
                                const Icon(
                                  FLucideIcons.chevronRight,
                                  size: 14,
                                ),
                              ],
                            ),
                            enabled: !c.busy,
                            submenuMaxHeight: 320,
                            submenu: [
                              FItemGroup(
                                enabled: !c.busy,
                                children: [
                                  for (final entry
                                      in AppStrings.wanxiangVariants.entries)
                                    FItem(
                                      key: ValueKey(
                                        'wanxiang-variant-${entry.key}',
                                      ),
                                      title: Text(entry.value),
                                      semanticsChecked:
                                          c.selectedWanxiangVariant ==
                                          entry.key,
                                      suffix:
                                          c.selectedWanxiangVariant == entry.key
                                          ? const Icon(FLucideIcons.check)
                                          : null,
                                      onPress: c.busy
                                          ? null
                                          : () {
                                              popover.hide();
                                              c.selectWanxiangVariant(
                                                entry.key,
                                              );
                                            },
                                    ),
                                ],
                              ),
                            ],
                          ),
                        if (!overlay &&
                            cloudCard &&
                            installed &&
                            !c.prefersLibjd)
                          FItem(
                            title: const Text('重新部署'),
                            prefix: const Icon(FLucideIcons.refreshCw),
                            onPress: c.canDeploy
                                ? () {
                                    popover.hide();
                                    c.deploy(schemeKey: packageKey);
                                  }
                                : null,
                          ),
                        if (!overlay && installed)
                          FItem(
                            title: Text(
                              c.prefersLibjd
                                  ? selectConversionSchemaLabel
                                  : AppStrings.selectSchemas,
                            ),
                            prefix: const Icon(FLucideIcons.listChecks),
                            enabled: !c.busy,
                            onPress: c.busy
                                ? null
                                : () {
                                    popover.hide();
                                    showPackageSchemaSelection(
                                      context,
                                      c,
                                      packageKey,
                                      convert: c.prefersLibjd,
                                    );
                                  },
                          ),
                        if (!overlay && installed && !c.isMobile)
                          FItem(
                            title: const Text('打开方案目录'),
                            prefix: const Icon(FLucideIcons.folderOpen),
                            enabled: !c.busy,
                            onPress: c.busy
                                ? null
                                : () {
                                    popover.hide();
                                    c.openSchemeDirectory(packageKey);
                                  },
                          ),
                        if (!overlay &&
                            installed &&
                            packageKey != 'wanxiang' &&
                            c.canToggleWanxiangGrammarOnPackages)
                          _wanxiangGrammarMenuItem(
                            context,
                            c,
                            packageKey,
                            popover,
                            inAddonsDialog: compact,
                          ),
                        FItem(
                          title: const Text('仓库地址'),
                          prefix: const Icon(FLucideIcons.externalLink),
                          onPress: () {
                            popover.hide();
                            c.open(repositoryUrl);
                          },
                        ),
                        if (!overlay && installed)
                          FItem(
                            title: const Text(AppStrings.uninstall),
                            variant: FItemVariant.destructive,
                            prefix: const Icon(FLucideIcons.trash2),
                            enabled: uninstall != null,
                            onPress: uninstall == null
                                ? null
                                : () {
                                    popover.hide();
                                    uninstall();
                                  },
                          ),
                      ],
                    ),
                  ],
                  builder: (_, popover, _) =>
                      _SchemeMoreButton(onPress: popover.toggle),
                ),
              ],
            ),
            if (mobile && cardState != null) ...[
              const SizedBox(height: 6),
              Align(alignment: Alignment.centerLeft, child: cardState),
            ],
            if (importing) ...[
              const SizedBox(height: 12),
              InstallProgressView(
                active: true,
                progress: c.installProgress,
                message: c.progress,
              ),
            ],
            if (cardError != null) ...[
              const SizedBox(height: 8),
              ErrorMessage(cardError),
            ],
            if (extra != null) ...[const SizedBox(height: 16), extra],
          ],
        ),
      ),
    );
  }

  Widget rimeAddon(String id, RimeAddonSlot slot) {
    final repositoryKey = id == 'triple_pinyin_lssp' ? 'rime_lssp' : id;
    final active = c.operation == slot.operation;
    return addon(
      key: 'addon-${repositoryKey.replaceAll('_', '-')}',
      packageKey: id,
      title: slot.title,
      repositoryUrl: schemeRepositoryUrls[repositoryKey]!,
      status: c.rimeAddonStatusText(id),
      label: slot.buttonLabel,
      tooltip: slot.tooltip,
      showInstall: slot.showInstall,
      install: c.canInstallAddon ? () => c.manageRimeAddon(id, true) : null,
      installed: slot.installed,
      uninstall: !c.busy && slot.installed
          ? () => c.manageRimeAddon(id, false)
          : null,
      active: active,
      progressPercent: slot.progress?.percent,
      error: slot.error,
    );
  }

  return {
    if (allowedKeys == null || allowedKeys.contains('easy_en'))
      'easy_en': addon(
      key: 'addon-easy-en',
      packageKey: 'easy_en',
      title: schemeDisplayName('easy_en'),
      repositoryUrl: schemeRepositoryUrls['easy_en']!,
      status: c.addonStatusText,
      label: c.addonButtonLabel,
      tooltip: c.addonTooltip,
      showInstall: c.showAddonInstall,
      install: c.canInstallAddon ? c.installAddon : null,
      installed: c.addon?.installed == true,
      uninstall: c.canUninstallAddon ? c.uninstallAddon : null,
      active: c.isManagingAddon,
      progressPercent: c.addonProgress?.percent,
      error: c.addonError,
    ),
    if (allowedKeys == null || allowedKeys.contains('wanxiang'))
      'wanxiang': addon(
      key: 'addon-wanxiang',
      packageKey: 'wanxiang',
      title: AppStrings.wanxiang,
      repositoryUrl: schemeRepositoryUrls['wanxiang']!,
      status: c.wanxiang?.installed == true
          ? c.wanxiangStatusText
          : '${c.wanxiangStatusText} · ${AppStrings.wanxiangVariants[c.selectedWanxiangVariant] ?? c.selectedWanxiangVariant}',
      label: c.wanxiangButtonLabel,
      tooltip: c.wanxiangTooltip,
      showInstall: c.showWanxiangInstall,
      install: c.canInstallWanxiang ? () => c.manageWanxiang(true) : null,
      installed: c.wanxiang?.installed == true,
      uninstall: c.canUninstallWanxiang ? () => c.manageWanxiang(false) : null,
      active: c.isManagingWanxiang,
      progressPercent: c.wanxiangProgress?.percent,
      error: c.wanxiangError,
    ),
    for (final entry in c.rimeAddons.entries)
      if (allowedKeys == null || allowedKeys.contains(entry.key))
        entry.key: rimeAddon(entry.key, entry.value),
  };
}
