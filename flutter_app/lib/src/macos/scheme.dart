import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart' show SelectableText;
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'widgets.dart';

class MacosSchemeSelection extends StatelessWidget {
  const MacosSchemeSelection({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosRows(
      children: [
        for (final entry in schemes.entries)
          MacosTooltip(
            message: entry.key == c.scheme
                ? c.selectedSchemeAsset
                : schemeAssets[entry.key]!,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: c.busy ? null : () => c.selectScheme(entry.key),
              child: Padding(
                padding: const EdgeInsets.all(12),
                child: Row(
                  children: [
                    MacosRadioButton<String>(
                      value: entry.key,
                      groupValue: c.scheme,
                      semanticLabel: entry.value,
                      onChanged: c.busy
                          ? null
                          : (value) {
                              if (value != null) c.selectScheme(value);
                            },
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(entry.value),
                          const SizedBox(height: 3),
                          Text(
                            entry.key == c.scheme
                                ? c.releaseVersion ?? c.selectedSchemeAsset
                                : schemeAssets[entry.key]!,
                            style: TextStyle(
                              color: MacosDynamicColor.resolve(
                                MacosColors.secondaryLabelColor,
                                context,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        MacosMessage(c.schemeSelectionText),
        MacosVersionPicker(controller: c),
      ],
    );
  }
}

class MacosVersionPicker extends StatelessWidget {
  const MacosVersionPicker({super.key, required this.controller});
  final AppController controller;

  void _showChangelog(BuildContext context) {
    final title = controller.changelogTitle!;
    final body = controller.changelogBody!;
    showMacosSheet<void>(
      context: context,
      builder: (context) => MacosSheet(
        insetPadding: const EdgeInsets.all(32),
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(title, style: MacosTheme.of(context).typography.headline),
              const SizedBox(height: 16),
              Expanded(
                child: SingleChildScrollView(
                  child: SelectableText(
                    body,
                    style: MacosTheme.of(context).typography.body
                        .copyWith(fontFamily: 'Menlo'),
                  ),
                ),
              ),
              const SizedBox(height: 16),
              Align(
                alignment: Alignment.centerRight,
                child: PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: () => Navigator.pop(context),
                  child: const Text(AppStrings.close),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final sources = c.sourceVersions;
    return MacosRows(
      children: [
        if (c.scheme == 'keytao' && sources.length > 1)
          MacosChoice<String>(
            label: '下载来源',
            value: c.source,
            options: {
              for (final entry in sources.entries)
                entry.key: '${downloadSources[entry.key]} ${entry.value}',
            },
            onChanged: c.selectSource,
          ),
        MacosSettingRow(
          '最新版本',
          stacked: true,
          child: Wrap(
            spacing: 10,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              if (c.releaseLoading) const ProgressCircle(radius: 7),
              if (c.scheme == 'keytao' &&
                  sources.length == 1 &&
                  sources.containsKey('github'))
                MacosIconButton(
                  onPressed: () => c.selectSource(sources.keys.single),
                  boxConstraints: const BoxConstraints(minHeight: 20),
                  padding: EdgeInsets.zero,
                  icon: MacosBadge(
                    '${downloadSources[sources.keys.single]} ${sources.values.single}',
                  ),
                )
              else
                MacosBadge(
                  c.scheme != 'keytao' && c.releaseVersion != null
                      ? '${c.releaseVersion} · ${c.selectedSchemeAsset}'
                      : c.latestRelease?.github == null
                      ? c.latestRelease?.version ?? '—'
                      : c.releaseVersion ?? '—',
                ),
              MacosTooltip(
                message: AppStrings.checkUpdate,
                child: MacosIconButton(
                  semanticLabel: AppStrings.checkUpdate,
                  onPressed: c.busy || c.releaseLoading
                      ? null
                      : c.refreshRelease,
                  icon: const MacosIcon(CupertinoIcons.refresh),
                ),
              ),
              if (c.changelogBody != null)
                MacosTooltip(
                  message: AppStrings.changelog,
                  child: MacosIconButton(
                    semanticLabel: AppStrings.changelog,
                    boxConstraints: const BoxConstraints(minHeight: 20),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 4,
                      vertical: 2,
                    ),
                    mouseCursor: SystemMouseCursors.click,
                    onPressed: () => _showChangelog(context),
                    icon: Text(
                      AppStrings.changelog,
                      style: TextStyle(
                        fontSize: 13,
                        color: MacosTheme.of(context).primaryColor,
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
        if (c.releaseError case final error?)
          MacosMessage(
            error,
            color: MacosColors.systemRedColor,
            icon: CupertinoIcons.exclamationmark_triangle,
          ),
      ],
    );
  }
}

class MacosSchemePage extends StatelessWidget {
  const MacosSchemePage({
    super.key,
    required this.controller,
    this.scrollController,
  });
  final AppController controller;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosForm(
      scrollController: scrollController,
      children: [
        MacosUpdateBanner(controller: c),
        MacosGroup(
          title: AppStrings.schemeTitle,
          icon: CupertinoIcons.arrow_down_circle,
          children: [
            MacosSchemeSelection(controller: c),
            MacosValueRow(AppStrings.directory, c.defaultDir),
            if (c.local != null)
              MacosMessage(
                '${c.localStatus}${c.localSchemaIds == null ? '' : ' ${c.localSchemaIds}'}',
                color: c.local?.deployed == true
                    ? MacosColors.systemGreenColor
                    : c.local?.installed == true
                    ? MacosColors.systemOrangeColor
                    : null,
                icon: c.local?.deployed == true
                    ? CupertinoIcons.checkmark_circle
                    : CupertinoIcons.info_circle,
              ),
            MacosActionRow(
              children: [
                PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: c.canInstall ? c.install : null,
                  child: Text(c.installButtonLabel),
                ),
                MacosTooltip(
                  message: c.deployTooltip,
                  child: PushButton(
                    controlSize: ControlSize.regular,
                    secondary: true,
                    onPressed: c.canDeploy ? c.deploy : null,
                    child: const Text(AppStrings.deploy),
                  ),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.refreshState,
                  child: const Text(AppStrings.checkLocal),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.busy ? null : c.openDefaultDirectory,
                  child: const Text(AppStrings.openDirectory),
                ),
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: c.canOpenDictionaryManager
                      ? c.openDictionaryManager
                      : null,
                  child: const Text(AppStrings.dictManager),
                ),
              ],
            ),
            if (c.installError case final error?)
              MacosMessage(
                error,
                color: MacosColors.systemRedColor,
                icon: CupertinoIcons.exclamationmark_triangle,
              ),
            MacosOperationProgress(controller: c),
          ],
        ),
        MacosAddons(controller: c),
        const MacosTextField(
          placeholder: AppStrings.testInput,
          minLines: 3,
          maxLines: 5,
        ),
      ],
    );
  }
}

class MacosAddons extends StatelessWidget {
  const MacosAddons({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return MacosGroup(
      title: AppStrings.addonTitle,
      icon: CupertinoIcons.book,
      children: [
        MacosValueRow(AppStrings.easyEnglish, c.addonStatusText),
        MacosActionRow(
          children: [
            if (c.showAddonInstall)
              MacosTooltip(
                message: c.addonTooltip,
                child: PushButton(
                  controlSize: ControlSize.regular,
                  onPressed: c.canInstallAddon ? c.installAddon : null,
                  child: Text(c.addonButtonLabel),
                ),
              ),
            if (c.addon?.installed == true)
              PushButton(
                controlSize: ControlSize.regular,
                secondary: true,
                onPressed: c.canUninstallAddon ? c.uninstallAddon : null,
                child: const Text(AppStrings.uninstall),
              ),
          ],
        ),
        if (c.isManagingAddon)
          MacosInstallProgress(active: true, progress: c.addonProgress),
        if (c.addonError case final error?)
          MacosMessage(
            error,
            color: MacosColors.systemRedColor,
            icon: CupertinoIcons.exclamationmark_triangle,
          ),
        MacosValueRow(AppStrings.wanxiang, c.wanxiangStatusText),
        MacosActionRow(
          children: [
            MacosTooltip(
              message: c.wanxiangTooltip,
              child: PushButton(
                controlSize: ControlSize.regular,
                onPressed: c.canInstallWanxiang
                    ? () => c.manageWanxiang(true)
                    : null,
                child: Text(c.wanxiangButtonLabel),
              ),
            ),
            if (c.wanxiang?.installed == true)
              PushButton(
                controlSize: ControlSize.regular,
                secondary: true,
                onPressed: c.canUninstallWanxiang
                    ? () => c.manageWanxiang(false)
                    : null,
                child: const Text(AppStrings.uninstall),
              ),
          ],
        ),
        const MacosMessage(AppStrings.wanxiangDescription),
        if (c.isManagingWanxiang)
          MacosInstallProgress(active: true, progress: c.wanxiangProgress),
        if (c.wanxiangError case final error?)
          MacosMessage(
            error,
            color: MacosColors.systemRedColor,
            icon: CupertinoIcons.exclamationmark_triangle,
          ),
      ],
    );
  }
}
