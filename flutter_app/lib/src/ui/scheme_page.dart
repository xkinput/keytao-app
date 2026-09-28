import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'progress.dart';
import 'widgets.dart';

class SchemePage extends StatelessWidget {
  const SchemePage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return AppPageBody(
      controller: c,
      page: AppPage.scheme,
      children: [
        Section(
          title: AppStrings.schemeTitle,
          icon: FLucideIcons.download,
          children: [
            SchemeSelection(controller: c),
            VersionPicker(controller: c),
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
                ActionButton(
                  c.installButtonLabel,
                  primary: true,
                  icon: FLucideIcons.download,
                  onPress: c.canInstall ? c.install : null,
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
        AddonCard(controller: c),
        const FTextField(hint: AppStrings.testInput, minLines: 3, maxLines: 5),
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
                        !c.isAndroid && c.scheme == entry.key
                            ? c.selectedSchemeAsset
                            : schemeAssets[entry.key]!,
                      ),
                      child: Semantics(
                        selected: c.scheme == entry.key,
                        child: FButton(
                          variant: c.scheme == entry.key
                              ? FButtonVariant.primary
                              : FButtonVariant.outline,
                          onPress: c.busy || (c.isAndroid && c.releaseLoading)
                              ? null
                              : () => c.selectScheme(entry.key),
                          child: Column(
                            children: [
                              Text(entry.value),
                              const SizedBox(height: 3),
                              Text(
                                c.scheme == entry.key
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
        const SizedBox(height: 10),
        StatusMessage(c.schemeSelectionText, icon: FLucideIcons.download),
      ],
    );
  }
}

class VersionPicker extends StatelessWidget {
  const VersionPicker({super.key, required this.controller});
  final AppController controller;
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
          '最新版本',
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
