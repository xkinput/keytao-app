import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'progress.dart';
import 'package_orphans.dart';
import 'settings.dart' show SchemaSwitchesSection, showCloudPackageTransfer;
import 'theme.dart';
import 'user_words.dart';
import 'widgets.dart';

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
  String? _activatingScheme;

  Future<void> _activate(String key) async {
    FocusManager.instance.primaryFocus?.unfocus();
    setState(() => _activatingScheme = key);
    try {
      await widget.controller.activateSchemePackage(key);
    } finally {
      if (mounted) setState(() => _activatingScheme = null);
    }
  }

  Widget _package(String key, {bool compact = false}) => SchemePackageRow(
    controller: widget.controller,
    schemeKey: key,
    compact: compact,
    showSource: false,
    activating: _activatingScheme == key,
    onActivate: () => _activate(key),
  );

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final xmjdScheme = c.schemeFamily == 'xmjd' ? c.scheme : 'xmjd';
    final schemeKeys = <String>{
      for (final key in schemes.keys) key == 'xmjd' ? xmjdScheme : key,
      for (final package in c.installedPackages) package.schemeKey,
    };
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final palette = AppPalette.of(context);
    final cloudKeys = schemeKeys.where((key) => !c.isLocalScheme(key));
    final activeKey = c.installedPackages
        .where((package) => package.active)
        .map((package) => package.schemeKey)
        .firstOrNull;
    final localKeys = schemeKeys.where(c.isLocalScheme);
    return AppPageBody(
      controller: c,
      page: AppPage.scheme,
      fullWidth: true,
      spacing: 0,
      children: [
        if (c.userWordsReport != null || c.userWordsReportError != null)
          UserWordsNotice(controller: c),
        Padding(
          padding: EdgeInsets.fromLTRB(
            desktop ? 32 : 0,
            8,
            desktop ? 32 : 0,
            12,
          ),
          child: Row(
            children: [
              const Expanded(child: EyebrowLabel('云端方案')),
              _SchemeSource(controller: c),
              FTooltip(
                tipBuilder: (_, _) => const Text(AppStrings.checkUpdate),
                child: FButton.icon(
                  variant: FButtonVariant.ghost,
                  onPress: c.busy || c.schemeUpdatesLoading
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
        if (c.schemeUpdatesLoading) const FProgress(),
        if (!desktop && activeKey != null)
          Container(
            margin: const EdgeInsets.only(bottom: 24),
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              color: palette.surface,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(18),
            ),
            child: _package(activeKey),
          ),
        if (desktop)
          HairlineGridSection(
            key: const ValueKey('cloud-schemes'),
            minCellWidth: 320,
            children: [
              for (final key in cloudKeys)
                HairlineGridCell(
                  color: key == activeKey ? palette.surface : null,
                  padding: const EdgeInsets.all(32),
                  child: _package(key),
                ),
            ],
          )
        else
          Column(
            key: const ValueKey('cloud-schemes'),
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final key in cloudKeys.where((key) => key != activeKey))
                HairlineGridCell(
                  padding: const EdgeInsets.symmetric(vertical: 16),
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      border: Border(top: BorderSide(color: palette.hairline)),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.only(top: 12),
                      child: _package(key, compact: true),
                    ),
                  ),
                ),
            ],
          ),
        HairlineGridSection(
          minCellWidth: 440,
          children: [
            HairlineGridCell(
              child: Section(
                key: const ValueKey('local-schemes'),
                title: AppStrings.localSchemeTitle,
                trailing: ActionButton(
                  '添加本地方案',
                  icon: FLucideIcons.plus,
                  onPress: c.busy ? null : c.importLocalScheme,
                ),
                children: [
                  for (final key in localKeys)
                    if (desktop || key != activeKey)
                      _package(key, compact: true),
                  ActionButton(
                    '清理残留文件',
                    icon: FLucideIcons.trash2,
                    onPress: c.busy
                        ? null
                        : () => showPackageOrphans(context, c),
                  ),
                ],
              ),
            ),
            HairlineGridCell(
              child: Section(
                title: AppStrings.deploy,
                children: [
                  if (c.packageError case final error?) ErrorMessage(error),
                  if (c.schemeSyncError case final error?) ErrorMessage(error),
                  ValueRow(AppStrings.directory, c.defaultDir),
                  if (c.localStatusLine case final status?)
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
                      if (c.canCancelDownload)
                        ActionButton(
                          AppStrings.cancel,
                          onPress: c.cancelDownload,
                        ),
                      ActionButton(
                        AppStrings.deploy,
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
                            : c.openDefaultDirectory,
                      ),
                      if (!c.isMobile)
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
                  if (c.deploySteps.isNotEmpty) DeploySteps(controller: c),
                  if (c.operationLogCount > 0)
                    ActionRow(children: [OperationLogButton(controller: c)]),
                ],
              ),
            ),
          ],
        ),
        if (c.local?.deployed == true && c.schemaSwitches.isNotEmpty)
          HairlineGridCell(child: SchemaSwitchesSection(controller: c)),
        HairlineGridCell(child: AddonCard(controller: c)),
        HairlineGridCell(
          child: const FTextField(
            hint: AppStrings.testInput,
            minLines: 3,
            maxLines: 5,
          ),
        ),
      ],
    );
  }
}

class SchemePackageRow extends StatelessWidget {
  const SchemePackageRow({
    super.key,
    required this.controller,
    required this.schemeKey,
    this.activating = false,
    this.onActivate,
    this.compact = false,
    this.showSource = true,
  });
  final AppController controller;
  final String schemeKey;
  final bool activating;
  final VoidCallback? onActivate;
  final bool compact;
  final bool showSource;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final palette = AppPalette.of(context);
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final package = c.installedPackageFor(schemeKey);
    final isLocal = package?.source == 'local';
    final update = c.schemeUpdateFor(schemeKey);
    final active = package?.active == true;
    final hasLogo =
        (active && c.activeSchemaLogoPath != null) || package?.logoPath != null;
    final activeLegacy = active && !package!.hasPackageZip && update == null;
    final deploying = activating || (active && c.isDeploying);
    final installing = c.installingSchemeKey == schemeKey;
    final changelogBody = schemeKey == 'keytao' ? c.changelogBody : null;
    final failedDeploySteps = c.deploySteps.where(
      (step) => step.state == DeployStepState.failed,
    );
    final deployFailed = failedDeploySteps.isNotEmpty;
    final needsDeploy = active && (deployFailed || c.local?.deployed != true);
    final actions = ActionRow(
      children: [
        if (schemeKey == 'keytao' && showSource) _SchemeSource(controller: c),
        if (update != null)
          ActionButton(
            '更新',
            primary: !needsDeploy,
            icon: FLucideIcons.arrowUpCircle,
            onPress: c.busy ? null : () => c.updateSchemePackage(schemeKey),
          ),
        if (!isLocal && (package == null || activeLegacy))
          ActionButton(
            activeLegacy ? '重新下载' : '下载',
            primary: false,
            icon: FLucideIcons.download,
            onPress: c.busy ? null : () => c.installSchemePackage(schemeKey),
          )
        else if (package != null &&
            !active &&
            (package.hasPackageZip || !isLocal))
          ActionButton(
            package.hasPackageZip ? '启用' : '重新下载',
            primary: update == null,
            icon: package.hasPackageZip
                ? FLucideIcons.play
                : FLucideIcons.download,
            onPress: c.busy
                ? null
                : package.hasPackageZip
                ? onActivate ?? () => c.activateSchemePackage(schemeKey)
                : () => c.downloadSchemePackage(schemeKey),
          ),
        if (active)
          ActionButton(
            AppStrings.deploy,
            primary: true,
            icon: FLucideIcons.play,
            onPress: c.canDeploy ? c.deploy : null,
          ),
        if (changelogBody != null ||
            schemeRepositoryUrls[schemeKey] != null ||
            (package != null &&
                (package.supportsUserWords ||
                    isLocal ||
                    c.accountUser != null ||
                    !active)))
          FPopoverMenu(
            menuAnchor: Alignment.topRight,
            childAnchor: Alignment.bottomRight,
            menuBuilder: (_, popover, _) => [
              FItemGroup(
                enabled: !c.busy,
                children: [
                  if (changelogBody != null)
                    FItem(
                      title: const Text(AppStrings.changelog),
                      prefix: const Icon(FLucideIcons.scrollText),
                      onPress: c.busy
                          ? null
                          : () {
                              popover.hide();
                              showChangelog(context, c, changelogBody);
                            },
                    ),
                  if (schemeRepositoryUrls[schemeKey] case final url?)
                    FItem(
                      title: const Text('仓库地址'),
                      prefix: const Icon(FLucideIcons.externalLink),
                      onPress: c.busy
                          ? null
                          : () {
                              popover.hide();
                              c.open(url);
                            },
                    ),
                  if (package != null) ...[
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
                    if (isLocal)
                      FItem(
                        title: const Text('重新导入'),
                        prefix: const Icon(FLucideIcons.folderOpen),
                        onPress: c.busy
                            ? null
                            : () {
                                popover.hide();
                                c.importLocalScheme(schemeKey: schemeKey);
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
                    FItem(
                      title: const Text('删除'),
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
            builder: (_, popover, _) => FTooltip(
              tipBuilder: (_, _) => const Text('更多操作'),
              child: FButton.icon(
                variant: FButtonVariant.outline,
                onPress: c.busy ? null : popover.toggle,
                style: FButtonStyleDelta.delta(
                  decoration: FVariants.all(
                    ShapeDecoration(
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(999),
                        side: BorderSide(color: palette.hairline, width: 1.5),
                      ),
                    ),
                  ),
                  contentStyle: const FButtonContentStyleDelta.delta(
                    constraints: BoxConstraints(minWidth: 44, minHeight: 44),
                  ),
                ),
                child: const Icon(FLucideIcons.ellipsis, semanticLabel: '更多操作'),
              ),
            ),
          ),
      ],
    );
    final name = package?.displayName ?? schemeDisplayName(schemeKey);
    final title = Semantics(
      selected: c.scheme == schemeKey,
      child: FButton(
        variant: FButtonVariant.ghost,
        mainAxisSize: MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.start,
        style: const FButtonStyleDelta.delta(
          contentStyle: FButtonContentStyleDelta.delta(
            padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
          ),
        ),
        onPress: c.busy ? null : () => c.selectScheme(schemeKey),
        child: Flexible(
          child: Text(
            schemeKey.startsWith('xmjd') && package?.displayName == null
                ? schemes['xmjd']!
                : name,
            semanticsLabel: name,
            style: context.theme.typography.body.sm.copyWith(
              fontSize: compact
                  ? 18
                  : desktop
                  ? 34
                  : 30,
              height: 1.1,
              fontWeight: FontWeight.w700,
              letterSpacing: compact ? 0 : -.34,
            ),
          ),
        ),
      ),
    );
    final state = deploying
        ? const SizedBox.square(dimension: 18, child: FCircularProgress())
        : active
        ? AppBadge(
            deployFailed
                ? '部署失败'
                : c.local?.deployed == true
                ? '使用中'
                : '待部署',
            background: deployFailed
                ? palette.error.withValues(alpha: .12)
                : null,
            foreground: deployFailed ? palette.error : null,
          )
        : null;
    final metadata = Wrap(
      spacing: 6,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
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
                      size: 28,
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
                  size: desktop ? 52 : 36,
                  schemeKey: schemeKey,
                  appLogoFallback: false,
                ),
              Flexible(child: title),
              ?state,
              if (update != null)
                Flexible(
                  child: Text(
                    '→ ${update.latestVersion}',
                    style: context.theme.typography.body.sm.copyWith(
                      color: palette.muted,
                    ),
                  ),
                ),
            ],
          );
    return Column(
      key: ValueKey('scheme-package-$schemeKey'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (!compact) ...[
          Row(
            children: [
              Expanded(
                child: EyebrowLabel(
                  active
                      ? '在用'
                      : isLocal
                      ? '本地'
                      : '云端',
                ),
              ),
              if (package?.version?.trim().isNotEmpty == true)
                Text(
                  package!.version!,
                  style: context.theme.typography.body.sm.copyWith(
                    color: active ? palette.successForeground : palette.muted,
                    fontSize: 13,
                    fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                  ),
                ),
            ],
          ),
          const SizedBox(height: 14),
        ],
        if (compact)
          Row(
            children: [
              Expanded(flex: 3, child: heading),
              const SizedBox(width: 8),
              Flexible(flex: 4, child: actions),
            ],
          )
        else
          heading,
        if (active && !deploying)
          for (final step in failedDeploySteps) ...[
            const SizedBox(height: 8),
            ErrorMessage(step.message),
          ],
        if (schemeKey == (c.schemeFamily == 'xmjd' ? c.scheme : 'xmjd')) ...[
          const SizedBox(height: 8),
          BigTextRadioGroup<String>(
            label: '${schemes['xmjd']} 变体',
            value: schemeKey,
            options: const {'xmjd': '顶功', 'xmjd-feiding': '非顶'},
            onChanged: c.busy ? null : c.selectScheme,
          ),
        ] else if (!compact && (desktop || !active)) ...[
          const SizedBox(height: 14),
          SizedBox(
            height: 44,
            child: Align(
              alignment: Alignment.centerLeft,
              child: package == null
                  ? Text(
                      '未下载',
                      style: context.theme.typography.body.sm.copyWith(
                        color: palette.muted,
                      ),
                    )
                  : const SizedBox.shrink(),
            ),
          ),
        ],
        if (!compact) ...[
          const SizedBox(height: 10),
          if (!desktop && active && !showSource)
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
        ],
        if (installing || deploying) ...[
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
          onPress: c.busy ? null : popover.toggle,
          child: Text('来源 · ${downloadSources[c.source] ?? c.source}'),
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
                          child: Column(
                            children: [
                              Text(entry.value),
                              const SizedBox(height: 3),
                              Text(
                                c.schemeFamily == entry.key
                                    ? c.releaseVersion ?? c.selectedSchemeAsset
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
                  onPress: c.busy || c.releaseLoading ? null : c.refreshRelease,
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
  Widget build(BuildContext context) {
    final c = controller;
    Widget addon({
      required String title,
      required String status,
      required String label,
      required String tooltip,
      required bool showInstall,
      required VoidCallback? install,
      required bool installed,
      required VoidCallback? uninstall,
      required bool active,
      required Widget progress,
      required String? error,
      Widget? variant,
      Widget? extra,
    }) => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          title,
          style: context.theme.typography.body.sm.copyWith(
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 4),
        Text(
          status,
          style: context.theme.typography.body.xs.copyWith(
            color: context.theme.colors.mutedForeground,
          ),
        ),
        if (variant != null) ...[const SizedBox(height: 12), variant],
        const SizedBox(height: 12),
        ActionRow(
          children: [
            if (showInstall)
              ActionButton(
                label,
                primary: true,
                icon: FLucideIcons.download,
                tooltip: tooltip,
                onPress: install,
              ),
            if (installed)
              ActionButton(AppStrings.uninstall, onPress: uninstall),
          ],
        ),
        if (active) ...[const SizedBox(height: 12), progress],
        if (error != null) ...[const SizedBox(height: 8), ErrorMessage(error)],
        if (extra != null) ...[const SizedBox(height: 16), extra],
      ],
    );
    return Section(
      title: AppStrings.addonTitle,
      icon: FLucideIcons.bookOpen,
      children: [
        addon(
          title: AppStrings.easyEnglish,
          status: c.addonStatusText,
          label: c.addonButtonLabel,
          tooltip: c.addonTooltip,
          showInstall: c.showAddonInstall,
          install: c.canInstallAddon ? c.installAddon : null,
          installed: c.addon?.installed == true,
          uninstall: c.canUninstallAddon ? c.uninstallAddon : null,
          active: c.isManagingAddon,
          progress: InstallProgressView(
            active: c.isManagingAddon,
            progress: c.addonProgress,
          ),
          error: c.addonError,
        ),
        addon(
          title: AppStrings.wanxiang,
          variant: Align(
            alignment: Alignment.centerLeft,
            child: SizedBox(
              width: 160,
              child: Semantics(
                label: AppStrings.wanxiangVariant,
                enabled: !c.busy,
                child: FSelect<String>(
                  enabled: !c.busy,
                  control: FSelectControl.lifted(
                    value: c.selectedWanxiangVariant,
                    onChange: c.selectWanxiangVariant,
                  ),
                  items: {
                    for (final entry in AppStrings.wanxiangVariants.entries)
                      entry.value: entry.key,
                  },
                ),
              ),
            ),
          ),
          status: c.wanxiangStatusText,
          label: c.wanxiangButtonLabel,
          tooltip: c.wanxiangTooltip,
          showInstall: c.showWanxiangInstall,
          install: c.canInstallWanxiang ? () => c.manageWanxiang(true) : null,
          installed: c.wanxiang?.installed == true,
          uninstall: c.canUninstallWanxiang
              ? () => c.manageWanxiang(false)
              : null,
          active: c.isManagingWanxiang,
          progress: InstallProgressView(
            active: c.isManagingWanxiang,
            progress: c.wanxiangProgress,
          ),
          error: c.wanxiangError,
          extra: c.showWanxiangGrammar
              ? Column(
                  key: const ValueKey('wanxiang-grammar'),
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    SettingRow(
                      AppStrings.wanxiangGrammar,
                      child: ActionRow(
                        children: [
                          Text(
                            c.wanxiangGrammarSize,
                            style: context.theme.typography.body.xs.copyWith(
                              color: context.theme.colors.mutedForeground,
                            ),
                          ),
                          if (c.isManagingWanxiangGrammar)
                            ActionButton(
                              AppStrings.cancel,
                              onPress: c.canCancelWanxiangGrammarDownload
                                  ? c.cancelWanxiangGrammarDownload
                                  : null,
                            )
                          else
                            ActionButton(
                              c.wanxiangGrammar?.installed == true
                                  ? '删除'
                                  : '下载',
                              icon: c.wanxiangGrammar?.installed == true
                                  ? FLucideIcons.trash2
                                  : FLucideIcons.download,
                              onPress: c.canManageWanxiangGrammar
                                  ? () => c.manageWanxiangGrammar(
                                      c.wanxiangGrammar?.installed != true,
                                    )
                                  : null,
                            ),
                        ],
                      ),
                    ),
                    if (c.isManagingWanxiangGrammar) ...[
                      const SizedBox(height: 8),
                      InstallProgressView(
                        active: true,
                        progress: c.wanxiangGrammarProgress,
                      ),
                    ],
                    if (c.wanxiangGrammarError case final error?) ...[
                      const SizedBox(height: 8),
                      ErrorMessage(error),
                    ],
                  ],
                )
              : null,
        ),
        addon(
          title: AppStrings.rimeIce,
          status: c.rimeIceStatusText,
          label: c.rimeIceButtonLabel,
          tooltip: c.rimeIceTooltip,
          showInstall: c.showRimeIceInstall,
          install: c.canInstallRimeIce ? () => c.manageRimeIce(true) : null,
          installed: c.rimeIce?.installed == true,
          uninstall: c.canUninstallRimeIce
              ? () => c.manageRimeIce(false)
              : null,
          active: c.isManagingRimeIce,
          progress: InstallProgressView(
            active: c.isManagingRimeIce,
            progress: c.rimeIceProgress,
          ),
          error: c.rimeIceError,
        ),
      ],
    );
  }
}
