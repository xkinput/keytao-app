import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'progress.dart';
import 'settings.dart' show runCloudAccountAction;
import 'user_words.dart';
import 'widgets.dart';

class SchemePage extends StatefulWidget {
  const SchemePage({super.key, required this.controller});
  final AppController controller;

  @override
  State<SchemePage> createState() => _SchemePageState();
}

class _SchemePageState extends State<SchemePage> {
  String? _activatingScheme;

  Future<void> _activate(String key) async {
    setState(() => _activatingScheme = key);
    try {
      await widget.controller.activateSchemePackage(key);
    } finally {
      if (mounted) setState(() => _activatingScheme = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final xmjdScheme = c.schemeFamily == 'xmjd' ? c.scheme : 'xmjd';
    final schemeKeys = <String>{
      for (final key in schemes.keys) key == 'xmjd' ? xmjdScheme : key,
      for (final package in c.installedPackages) package.schemeKey,
    };
    return AppPageBody(
      controller: c,
      page: AppPage.scheme,
      children: [
        if (c.userWordsReport != null || c.userWordsReportError != null)
          UserWordsNotice(controller: c),
        Section(
          title: AppStrings.schemeTitle,
          icon: FLucideIcons.download,
          trailing: FTooltip(
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
          children: [
            ActionRow(
              children: [
                ActionButton(
                  '添加本地方案',
                  icon: FLucideIcons.folderOpen,
                  onPress: c.busy ? null : c.importLocalScheme,
                ),
              ],
            ),
            for (final key in schemeKeys)
              SchemePackageRow(
                controller: c,
                schemeKey: key,
                activating: _activatingScheme == key,
                onActivate: () => _activate(key),
              ),
            if (c.schemeUpdatesLoading) const FProgress(),
            if (c.packageError case final error?) ErrorMessage(error),
            if (c.schemeSyncError case final error?) ErrorMessage(error),
            if (!c.isLocalScheme(c.scheme))
              VersionPicker(controller: c, showScheme: true),
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
                  ActionButton(AppStrings.cancel, onPress: c.cancelDownload),
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
        AddonCard(controller: c),
        const FTextField(hint: AppStrings.testInput, minLines: 3, maxLines: 5),
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
  });
  final AppController controller;
  final String schemeKey;
  final bool activating;
  final VoidCallback? onActivate;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final package = c.installedPackageFor(schemeKey);
    final isLocal = package?.source == 'local';
    final update = c.schemeUpdateFor(schemeKey);
    final active = package?.active == true;
    final activeLegacy = active && !package!.hasPackageZip && update == null;
    final deploying = activating || (active && c.isDeploying);
    final failedDeploySteps = c.deploySteps.where(
      (step) => step.state == DeployStepState.failed,
    );
    final deployFailed = failedDeploySteps.isNotEmpty;
    final needsDeploy = active && (deployFailed || c.local?.deployed != true);
    return Column(
      key: ValueKey('scheme-package-$schemeKey'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Semantics(
          selected: c.scheme == schemeKey,
          child: FButton(
            variant: FButtonVariant.ghost,
            mainAxisAlignment: MainAxisAlignment.start,
            style: const FButtonStyleDelta.delta(
              contentStyle: FButtonContentStyleDelta.delta(
                padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
              ),
            ),
            onPress: c.busy ? null : () => c.selectScheme(schemeKey),
            child: Text(package?.displayName ?? schemeDisplayName(schemeKey)),
          ),
        ),
        if (package != null || update != null) ...[
          const SizedBox(height: 6),
          Wrap(
            spacing: 8,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              if (deploying)
                Row(
                  children: [
                    const SizedBox.square(
                      dimension: 18,
                      child: FCircularProgress(),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: InstallProgressView(
                        active: true,
                        progress: c.installProgress,
                        fraction: c.installFraction,
                        message: c.progress,
                      ),
                    ),
                  ],
                )
              else if (active)
                FBadge(
                  child: Text(
                    deployFailed
                        ? '部署失败'
                        : c.local?.deployed == true
                        ? '使用中'
                        : '待部署',
                  ),
                ),
              if (package != null)
                Text(
                  package.version?.trim().isNotEmpty == true
                      ? package.version!
                      : '—',
                  style: context.theme.typography.body.sm.copyWith(
                    color: context.theme.colors.mutedForeground,
                  ),
                ),
              if (update != null)
                Text(
                  '→ ${update.latestVersion}',
                  style: context.theme.typography.body.sm.copyWith(
                    color: context.theme.colors.mutedForeground,
                  ),
                ),
            ],
          ),
        ],
        if (active && !deploying)
          for (final step in failedDeploySteps) ...[
            const SizedBox(height: 8),
            ErrorMessage(step.message),
          ],
        if (schemeKey == (c.schemeFamily == 'xmjd' ? c.scheme : 'xmjd')) ...[
          const SizedBox(height: 8),
          Semantics(
            enabled: !c.busy,
            child: ExcludeFocus(
              excluding: c.busy,
              child: IgnorePointer(
                ignoring: c.busy,
                child: Opacity(
                  opacity: c.busy ? .5 : 1,
                  child: FTabs(
                    control: FTabControl.lifted(
                      index: schemeKey == 'xmjd' ? 0 : 1,
                      onChange: (index) =>
                          c.selectScheme(index == 0 ? 'xmjd' : 'xmjd-feiding'),
                    ),
                    style: const FTabsStyleDelta.delta(
                      spacing: 0,
                      minHeight: 32,
                    ),
                    children: const [
                      FTabEntry(
                        label: Text('顶功', style: TextStyle(fontSize: 12)),
                        child: SizedBox.shrink(),
                      ),
                      FTabEntry(
                        label: Text('非顶', style: TextStyle(fontSize: 12)),
                        child: SizedBox.shrink(),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ],
        const SizedBox(height: 10),
        ActionRow(
          children: [
            if (package != null)
              ActionButton(
                '自造词',
                icon: FLucideIcons.bookOpen,
                onPress: c.busy
                    ? null
                    : () => openUserWords(context, c, schemeKey: schemeKey),
              ),
            if (package != null && c.accountUser != null)
              ActionButton(
                '上传',
                onPress: c.busy || !package.hasPackageZip
                    ? null
                    : () => runCloudAccountAction(context, c, () async {
                        final preview = await c.previewCloudPackage(schemeKey);
                        if (preview == null || !context.mounted) return false;
                        final files = (preview['files'] as List).cast<String>();
                        final size =
                            (preview['archive_size'] as num) / (1024 * 1024);
                        final confirmed = await showFDialog<bool>(
                          context: context,
                          builder: (context, _, animation) => AppDialog(
                            title: '上传方案',
                            animation: animation,
                            actions: [
                              ActionButton(
                                '取消',
                                onPress: () => Navigator.pop(context, false),
                              ),
                              ActionButton(
                                '确认上传',
                                primary: true,
                                onPress: () => Navigator.pop(context, true),
                              ),
                            ],
                            child: Column(
                              mainAxisSize: MainAxisSize.min,
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(
                                  '${files.length} 个文件 · ${size.toStringAsFixed(2)} MB',
                                ),
                                const SizedBox(height: 8),
                                ConstrainedBox(
                                  constraints: const BoxConstraints(
                                    maxHeight: 280,
                                  ),
                                  child: SingleChildScrollView(
                                    child: SelectableText(files.join('\n')),
                                  ),
                                ),
                              ],
                            ),
                          ),
                        );
                        if (confirmed != true) return false;
                        return c.uploadCloudPackage(
                          schemeKey,
                          expectedSha256: preview['sha256'] as String,
                        );
                      }),
              ),
            if (update != null)
              ActionButton(
                '更新',
                primary: !needsDeploy,
                icon: FLucideIcons.arrowUpCircle,
                onPress: c.busy ? null : () => c.updateSchemePackage(schemeKey),
              ),
            if (isLocal)
              ActionButton(
                '重新导入',
                icon: FLucideIcons.folderOpen,
                onPress: c.busy
                    ? null
                    : () => c.importLocalScheme(schemeKey: schemeKey),
              ),
            if (!isLocal && (package == null || activeLegacy))
              ActionButton(
                activeLegacy ? '重新下载' : '下载',
                primary: !needsDeploy,
                icon: FLucideIcons.download,
                onPress: c.busy
                    ? null
                    : () => c.installSchemePackage(schemeKey),
              )
            else if (package != null && !active) ...[
              if (package.hasPackageZip || !isLocal)
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
              ActionButton(
                '删除',
                icon: FLucideIcons.trash2,
                onPress: c.busy ? null : () => c.removeSchemePackage(schemeKey),
              ),
            ],
            if (isLocal && active)
              ActionButton('删除', icon: FLucideIcons.trash2),
            if (needsDeploy)
              ActionButton(
                AppStrings.deploy,
                primary: true,
                icon: FLucideIcons.play,
                onPress: c.canDeploy ? c.deploy : null,
              ),
          ],
        ),
        if (package != null && c.accountUser != null) ...[
          const SizedBox(height: 10),
          SwitchSetting(
            '随账号同步',
            value: c.syncedSchemes.contains(schemeKey),
            onChanged: c.busy
                ? null
                : (value) => c.setSchemeAccountSync(schemeKey, value),
          ),
        ],
      ],
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
                  onPress: () => showFDialog<void>(
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
                          c.changelogBody!,
                          style: context.theme.typography.body.sm.copyWith(
                            fontFamily: 'monospace',
                          ),
                        ),
                      ),
                    ),
                  ),
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
      String? description,
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
        if (description != null) ...[
          const SizedBox(height: 12),
          Text(
            description,
            style: context.theme.typography.body.xs.copyWith(
              height: 1.5,
              color: context.theme.colors.mutedForeground,
            ),
          ),
        ],
        if (active) ...[const SizedBox(height: 12), progress],
        if (error != null) ...[const SizedBox(height: 8), ErrorMessage(error)],
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
          status: c.wanxiangStatusText,
          label: c.wanxiangButtonLabel,
          tooltip: c.wanxiangTooltip,
          showInstall: true,
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
          description: AppStrings.wanxiangDescription,
        ),
      ],
    );
  }
}
