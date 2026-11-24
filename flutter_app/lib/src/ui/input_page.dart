import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'settings.dart';
import 'progress.dart';
import 'theme.dart';
import 'user_words.dart';
import 'widgets.dart';

class _StatusBadge extends StatelessWidget {
  const _StatusBadge(this.label, {required this.ready, this.failed = false});
  final String label;
  final bool ready;
  final bool failed;

  @override
  Widget build(BuildContext context) => FBadge(
    variant: failed ? FBadgeVariant.destructive : FBadgeVariant.outline,
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(
            color: failed
                ? context.theme.colors.destructive
                : ready
                ? successColor(context)
                : context.theme.colors.mutedForeground,
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 8),
        Text(label),
      ],
    ),
  );
}

class InputPage extends StatelessWidget {
  const InputPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => AppPageBody(
    controller: controller,
    page: AppPage.input,
    fullWidth: true,
    spacing: 0,
    children: [
      if (controller.userWordsReport != null ||
          controller.userWordsReportError != null)
        UserWordsNotice(controller: controller),
      SettingsCard(
        controller: controller,
        hero: _CurrentSchemeHero(controller: controller),
      ),
      if (controller.isWindows && controller.deploySteps.isNotEmpty)
        HairlineGridCell(child: DeploySteps(controller: controller)),
      HairlineGridSection(
        children: [
          HairlineGridCell(child: AccountCard(controller: controller)),
          if (controller.operationLogs.any(
            (line) =>
                line.contains('[DEPLOY]') || line.contains('[DEPLOY ERROR]'),
          ))
            HairlineGridCell(
              child: Section(
                title: '刚刚',
                children: [
                  for (final line
                      in controller.operationLogs.reversed
                          .where(
                            (line) =>
                                line.contains('[DEPLOY]') ||
                                line.contains('[DEPLOY ERROR]'),
                          )
                          .take(4))
                    _RecentOperation(line),
                ],
              ),
            ),
          HairlineGridCell(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 18,
              children: [
                if (controller.isAndroid)
                  AndroidImeCard(controller: controller),
                if (controller.info.platform == BridgePlatform.macOs)
                  MacosImeCard(controller: controller),
                if (controller.isWindows)
                  WindowsImeCard(controller: controller),
                if (controller.isLinux) LinuxImeCard(controller: controller),
                if (controller.isIos) IosImeCard(controller: controller),
                SwitchSetting(
                  AppStrings.algorithmEngine,
                  hint: switch (controller.inputEngineTransition) {
                    true => AppStrings.enablingAlgorithmEngine,
                    false => AppStrings.disablingAlgorithmEngine,
                    null => controller.info.platform == BridgePlatform.macOs
                        ? AppStrings.macosAlgorithmEngineHint
                        : AppStrings.algorithmEngineHint,
                  },
                  value: controller.inputEngineEnabled,
                  onChanged: controller.busy
                      ? null
                      : controller.setInputEngineEnabled,
                ),
                Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: ActionButton(
                    controller.isDeploying
                        ? AppStrings.deploying
                        : AppStrings.redeploy,
                    onPress:
                        controller.busy || !controller.activeSchemeDownloaded
                        ? null
                        : controller.deploy,
                  ),
                ),
                ValueRow('版本', controller.info.appVersion),
                if (!controller.isMobile &&
                    controller.uiSettings?.themePath != null)
                  ValueRow(
                    AppStrings.theme,
                    controller.uiSettings!.themePath!,
                    trailing: FTooltip(
                      tipBuilder: (_, _) => const Text(AppStrings.openTheme),
                      child: FButton.icon(
                        onPress: controller.canOpenTheme
                            ? controller.openTheme
                            : null,
                        variant: FButtonVariant.ghost,
                        child: const Icon(
                          FLucideIcons.folderOpen,
                          size: 16,
                          semanticLabel: AppStrings.openTheme,
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    ],
  );
}

class _RecentOperation extends StatelessWidget {
  const _RecentOperation(this.line);
  final String line;

  @override
  Widget build(BuildContext context) {
    final log = parseOperationLog(line);
    final palette = AppPalette.of(context);
    final failed = operationLogKind(line) == LogLineKind.error;
    final style = context.theme.typography.body.sm.copyWith(
      color: failed ? palette.error : palette.success,
    );
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      spacing: 12,
      children: [
        if (log.time case final time?)
          SizedBox(
            width: 44,
            child: Text(
              time,
              style: style.copyWith(
                color: failed ? palette.error : palette.muted,
              ),
            ),
          ),
        Expanded(child: Text(log.message, style: style)),
      ],
    );
  }
}

class _SchemaMenuChoice {
  const _SchemaMenuChoice({
    required this.activePackage,
    required this.packageKey,
    required this.schemaId,
    required this.title,
    required this.subtitle,
    required this.checked,
  });

  final bool activePackage;
  final String? packageKey;
  final String schemaId;
  final String title;
  final String? subtitle;
  final bool checked;
}

class _CurrentSchemeHero extends StatelessWidget {
  const _CurrentSchemeHero({required this.controller});
  final AppController controller;

  List<_SchemaMenuChoice> _buildChoices(IsolatedPackageDto? activePackage) {
    final c = controller;
    final pendingLabel = c.prefersLibjd ? '转换后可用' : '部署后可用';
    if (c.isolatedPackages.isEmpty) {
      return [
        for (final schema in c.schemas)
          if (!c.prefersLibjd || schema.id != 'easy_en')
            _SchemaMenuChoice(
              activePackage: true,
              packageKey: null,
              schemaId: schema.id,
              title: schema.name,
              subtitle: null,
              checked: schema.id == c.currentSchemaId,
            ),
      ];
    }

    final raw =
        <
          ({
            bool activePkg,
            String packageKey,
            String packageName,
            String schemaId,
            String name,
            bool ready,
          })
        >[];

    final activeSchemas = c.schemas.isNotEmpty
        ? c.schemas
        : (activePackage?.schemas ?? const <SchemaDto>[]);
    if (activePackage != null) {
      for (final schema in activeSchemas) {
        if (c.prefersLibjd && schema.id == 'easy_en') continue;
        raw.add((
          activePkg: true,
          packageKey: activePackage.packageKey,
          packageName: activePackage.displayName,
          schemaId: schema.id,
          name: schema.name,
          ready: c.prefersLibjd
              ? c.usesImportedLibjd(activePackage.packageKey)
              : c.schemas.isNotEmpty || activePackage.ready,
        ));
      }
    } else {
      for (final schema in c.schemas) {
        if (c.prefersLibjd && schema.id == 'easy_en') continue;
        raw.add((
          activePkg: true,
          packageKey: '',
          packageName: '',
          schemaId: schema.id,
          name: schema.name,
          ready: true,
        ));
      }
    }

    for (final pkg in c.isolatedPackages.where((pkg) => !pkg.active)) {
      if (pkg.schemas.isEmpty) {
        raw.add((
          activePkg: false,
          packageKey: pkg.packageKey,
          packageName: pkg.displayName,
          schemaId: pkg.packageKey,
          name: pkg.displayName,
          ready: c.prefersLibjd
              ? c.usesImportedLibjd(pkg.packageKey)
              : pkg.ready,
        ));
      } else {
        for (final schema in pkg.schemas) {
          if (schema.id == 'easy_en') continue;
          raw.add((
            activePkg: false,
            packageKey: pkg.packageKey,
            packageName: pkg.displayName,
            schemaId: schema.id,
            name: schema.name,
            ready: c.prefersLibjd
                ? c.usesImportedLibjd(pkg.packageKey)
                : pkg.ready,
          ));
        }
      }
    }

    final counts = <String, int>{};
    for (final item in raw) {
      counts[item.name] = (counts[item.name] ?? 0) + 1;
    }

    return [
      for (final item in raw)
        _SchemaMenuChoice(
          activePackage: item.activePkg,
          packageKey: item.packageKey.isEmpty ? null : item.packageKey,
          schemaId: item.schemaId,
          title: item.name,
          subtitle: switch ((
            (counts[item.name] ?? 0) > 1 &&
                item.packageName.isNotEmpty &&
                item.packageName != item.name,
            !item.ready,
          )) {
            (true, true) => '${item.packageName} · $pendingLabel',
            (true, false) => item.packageName,
            (false, true) => pendingLabel,
            (false, false) => null,
          },
          checked: item.activePkg && item.schemaId == c.currentSchemaId,
        ),
    ];
  }

  Widget _schemeMenu(
    List<_SchemaMenuChoice> choices,
    Widget Function(FPopoverController) builder,
  ) {
    final c = controller;
    return FPopoverMenu(
      menuAnchor: Alignment.topLeft,
      childAnchor: Alignment.bottomLeft,
      menuBuilder: (_, popover, _) => [
        FItemGroup(
          enabled: !c.busy,
          children: [
            for (final choice in choices)
              FItem(
                title: Text(choice.title),
                subtitle: choice.subtitle == null
                    ? null
                    : Text(choice.subtitle!),
                semanticsChecked: choice.checked,
                suffix: choice.checked ? const Icon(FLucideIcons.check) : null,
                onPress: c.busy
                    ? null
                    : () {
                        popover.hide();
                        if (choice.packageKey == null ||
                            (choice.activePackage && !c.prefersLibjd)) {
                          c.selectSchema(choice.schemaId);
                        } else {
                          c.selectPackageSchema(
                            choice.packageKey!,
                            choice.schemaId,
                          );
                        }
                      },
              ),
          ],
        ),
      ],
      builder: (_, popover, _) => builder(popover),
    );
  }

  Widget _engineMenu(String engine, Color color) {
    final c = controller;
    return FPopoverMenu(
      menuAnchor: Alignment.topLeft,
      childAnchor: Alignment.bottomLeft,
      menuBuilder: (_, popover, _) => [
        FItemGroup(
          enabled: !c.busy,
          children: [
            for (final (id, name) in [
              ('rime', 'Rime'),
              ('libjd_engine', 'ime-jd'),
            ])
              FItem(
                title: Text(name),
                semanticsChecked: c.inputEngine == id,
                suffix: c.inputEngine == id
                    ? const Icon(FLucideIcons.check)
                    : null,
                onPress: () {
                  popover.hide();
                  if (c.inputEngine != id) c.setInputEngine(id);
                },
              ),
          ],
        ),
      ],
      builder: (context, popover, _) => FButton.raw(
        key: const ValueKey('current-engine-selector'),
        variant: FButtonVariant.ghost,
        semanticsLabel: '切换引擎，$engine',
        onPress: c.busy ? null : popover.toggle,
        child: ConstrainedBox(
          constraints: context.platformVariant.touch
              ? const BoxConstraints(minWidth: 44, minHeight: 44)
              : const BoxConstraints(),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 3),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              spacing: 4,
              children: [
                EyebrowLabel(engine),
                Icon(FLucideIcons.chevronDown, size: 14, color: color),
              ],
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final palette = AppPalette.of(context);
    final package = c.installedPackageFor(c.activeSchemeKey);
    final addon = switch (c.activeSchemeKey) {
      'easy_en' => c.addon,
      'wanxiang' => c.wanxiang,
      'triple_pinyin_lssp' => c.rimeLssp,
      _ => null,
    };
    final local = c.local;
    final isolatedPackage = c.isolatedPackages
        .where((package) => package.active)
        .firstOrNull;
    final choices = _buildChoices(isolatedPackage);
    final installed = c.activeSchemeDownloaded;
    final version =
        package?.version ??
        addon?.version ??
        (local == null
            ? null
            : displayedSchemeVersion(
                local.schemas,
                local.version,
                local.installedScheme,
                local.installedSchemeVersion,
              ));
    final deployment = c.packageDeployLabel(
      c.deployingSchemeKey ?? c.activeSchemeKey,
    );
    final status = switch (c) {
      _ when c.isImportingLibjd => '转换中',
      _ when deployment != null => deployment,
      _ when c.isSwitchingInput => '切换中',
      _ when c.isDeploying => AppStrings.packageDeploying,
      _ when c.isUsingLibjd => null,
      _ when local == null => AppStrings.checking,
      _ when !installed => AppStrings.schemeMissing,
      _ when c.prefersLibjd => '未转换',
      _ when !c.hasUsableBuild => '未部署',
      _ when c.autoDeployQueued => AppStrings.packageDeployQueued,
      _ => null,
    };
    final schemeName =
        c.currentSchemaName ??
        isolatedPackage?.displayName ??
        package?.displayName ??
        (installed ? schemeDisplayName(c.activeSchemeKey) : null);
    final title = schemeName ?? status ?? AppStrings.checking;
    final caption = schemeName == null ? null : status;
    final engine = c.prefersLibjd ? 'ime-jd' : 'Rime';
    final engineAvailable = c.libjd?.available == true;
    final platform = '${c.aboutValues['平台']} 系统输入法';
    final source = downloadSources[package?.source];
    final showDeploy = installed && !c.prefersLibjd && !c.hasUsableBuild;
    final showInstall =
        choices.isEmpty && (!c.prefersLibjd || c.hasConvertedSchemes);
    return Container(
      margin: desktop ? EdgeInsets.zero : const EdgeInsets.only(top: 4),
      padding: desktop
          ? const EdgeInsets.symmetric(horizontal: 24, vertical: 30)
          : const EdgeInsets.all(12),
      decoration: desktop
          ? null
          : BoxDecoration(
              color: palette.surface,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(18),
            ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: desktop ? 15 : 9,
        children: [
          if (engineAvailable)
            Wrap(
              spacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                EyebrowLabel('$platform ·'),
                _engineMenu(engine, palette.muted),
              ],
            )
          else
            EyebrowLabel('$platform · $engine'),
          Row(
            spacing: 12,
            children: [
              SchemaLogo(controller: c, size: desktop ? 40 : 30),
              Expanded(
                child: _schemeMenu(
                  choices,
                  (popover) => FButton.raw(
                    key: const ValueKey('current-schema-selector'),
                    variant: FButtonVariant.ghost,
                    semanticsLabel: caption == null
                        ? '方案，$title'
                        : '方案，$title，$caption',
                    onPress: c.busy || choices.isEmpty
                        ? null
                        : () {
                            c.refreshSchemas();
                            popover.toggle();
                          },
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      spacing: 12,
                      children: [
                        Flexible(
                          child: Wrap(
                            spacing: 12,
                            runSpacing: 4,
                            crossAxisAlignment: WrapCrossAlignment.center,
                            children: [
                              Text(
                                title,
                                style: context.theme.typography.body.sm
                                    .copyWith(
                                      color: palette.ink,
                                      fontSize: desktop ? 44 : 28,
                                      fontWeight: FontWeight.w700,
                                      height: 1.05,
                                      letterSpacing: desktop ? -1.12 : -.32,
                                    ),
                              ),
                              if (caption != null)
                                Text(
                                  caption,
                                  style: context.theme.typography.body.sm
                                      .copyWith(color: palette.muted),
                                ),
                            ],
                          ),
                        ),
                        if (choices.isNotEmpty)
                          Icon(
                            FLucideIcons.chevronDown,
                            size: desktop ? 20 : 18,
                            color: palette.muted,
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ],
          ),
          if (version?.isNotEmpty == true || source != null)
            Wrap(
              spacing: 12,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (version != null && version.isNotEmpty)
                  Text(version.startsWith('v') ? version : 'v$version'),
                if (source != null) Text('来自 $source'),
              ],
            ),
          if (c.prefersLibjd && !c.hasConvertedSchemes) ...[
            const Text('请前往方案页下载并转换你的方案。ime-jd 目前仅支持键道方案。'),
            ActionRow(
              children: [
                ActionButton(
                  '前往方案页',
                  onPress: () => c.selectPage(AppPage.scheme),
                ),
                ActionButton(
                  '了解 ime-jd',
                  onPress: () => c.open(AppController.libjdRepositoryUrl),
                ),
              ],
            ),
          ],
          if (c.packageError != null) ErrorMessage(c.packageError),
          if (c.isImportingLibjd)
            InstallProgressView(
              active: true,
              progress: c.installProgress,
              fraction: c.installFraction,
              message: c.progress,
            ),
          if (showDeploy || showInstall)
            ActionRow(
              children: [
                if (showDeploy)
                  ActionButton(
                    AppStrings.deploy,
                    primary: true,
                    tooltip: c.deployTooltip,
                    onPress: c.canDeploy ? c.deploy : null,
                  ),
                if (showInstall)
                  ActionButton(
                    '前往安装方案',
                    onPress: () => c.selectPage(AppPage.scheme),
                  ),
              ],
            ),
        ],
      ),
    );
  }
}

class WindowsImeCard extends StatelessWidget {
  const WindowsImeCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final s = c.windowsIme;
    return Section(
      title: AppStrings.windowsIme,
      icon: FLucideIcons.keyboard,
      trailing: _StatusBadge(
        c.windowsRegistrationLabel,
        ready: !c.windowsRegistrationBusy && s?.registered == true,
        failed: s?.registrationState == 'failed',
      ),
      children: [
        if (c.windowsRegistrationBusy) const FProgress(),
        if (s == null) const StatusMessage(AppStrings.windowsChecking),
        if (s != null) ...[
          ActionRow(
            children: [
              _StatusBadge(
                AppStrings.windowsPackage(s.packaged),
                ready: s.packaged,
              ),
              _StatusBadge(
                AppStrings.windowsRegistration(c.windowsRegistrationLabel),
                ready: s.registered,
                failed: s.registrationState == 'failed',
              ),
              _StatusBadge(
                AppStrings.windowsDll(s.registeredDll),
                ready: s.registeredDll,
              ),
              _StatusBadge(
                AppStrings.windowsProfile(s.profileEnabled),
                ready: s.profileEnabled,
              ),
            ],
          ),
          if (s.runtimeDir?.isNotEmpty == true)
            ValueRow(AppStrings.runtime, s.runtimeDir!),
          if (s.registeredPath?.isNotEmpty == true)
            ValueRow(AppStrings.registeredPath, s.registeredPath!),
          if (s.profileStatus.isNotEmpty)
            ValueRow(AppStrings.profile, s.profileStatus),
          if (s.message.isNotEmpty) StatusMessage(s.message),
        ],
        ActionRow(
          children: [
            ActionButton(
              AppStrings.refresh,
              icon: FLucideIcons.refreshCw,
              onPress: c.windowsRegistrationBusy ? null : c.refreshWindowsIme,
            ),
          ],
        ),
        if (c.windowsImeError case final error?) ErrorMessage(error),
      ],
    );
  }
}

class LinuxImeCard extends StatelessWidget {
  const LinuxImeCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final s = c.linuxIme;
    return Section(
      title: AppStrings.linuxIme,
      icon: FLucideIcons.cpu,
      trailing: _StatusBadge(
        s?.running == true ? AppStrings.running : AppStrings.notRunning,
        ready: s?.running == true,
      ),
      children: [
        if (s != null) ...[
          ValueRow(AppStrings.command, s.command),
          ActionRow(
            children: [
              FBadge(
                variant: FBadgeVariant.outline,
                child: Text(
                  s.kdeSession
                      ? AppStrings.kdeSession
                      : AppStrings.nonKdeSession,
                ),
              ),
              _StatusBadge(
                AppStrings.kdeConfiguration(s.kdeConfigured),
                ready: s.kdeConfigured,
              ),
              if (s.managedPid != null && s.managedPid != 0)
                FBadge(
                  variant: FBadgeVariant.outline,
                  child: Text(AppStrings.managedPid(s.managedPid!)),
                ),
              if (s.kdeNativeProcesses > 0)
                FBadge(
                  variant: FBadgeVariant.outline,
                  child: Text(AppStrings.kdeProcesses(s.kdeNativeProcesses)),
                ),
              if (s.fallbackProcesses > 0)
                FBadge(
                  variant: FBadgeVariant.outline,
                  child: Text(
                    AppStrings.fallbackProcesses(s.fallbackProcesses),
                  ),
                ),
            ],
          ),
          if (s.message.isNotEmpty) StatusMessage(s.message),
        ],
        if (c.linuxImeError case final error?) ErrorMessage(error),
      ],
    );
  }
}

class IosImeCard extends StatelessWidget {
  const IosImeCard({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) => Section(
    title:
        controller.iosKeyboardEnabled &&
            controller.iosKeyboardFullAccess == true
        ? AppStrings.iosKeyboardReady
        : AppStrings.iosOnboarding,
    icon: FLucideIcons.keyboard,
    children: [
      if (!controller.iosKeyboardEnabled) ...[
        const Text(AppStrings.iosAddKeyboard),
        const Text(AppStrings.iosChooseKeyboard),
      ] else if (controller.iosKeyboardFullAccess != true) ...[
        const Text(AppStrings.iosEnableFullAccess),
        const Text(AppStrings.iosFullAccessDescription),
        const Text(AppStrings.iosKeyboardStatusRefresh),
      ] else ...[
        const Text(AppStrings.iosGlobe),
      ],
      if (!controller.iosKeyboardEnabled ||
          controller.iosKeyboardFullAccess != true)
        ActionRow(
          children: [
            ActionButton(
              AppStrings.openSystemSettings,
              icon: FLucideIcons.settings,
              onPress: controller.busy
                  ? null
                  : controller.openInputMethodSettings,
            ),
          ],
        ),
    ],
  );
}

class MacosImeCard extends StatelessWidget {
  const MacosImeCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => Section(
    title: AppStrings.macosIme,
    icon: FLucideIcons.keyboard,
    trailing:
        controller.prefersLibjd &&
            !controller.isUsingLibjd &&
            controller.macosIme?.installed == true
        ? null
        : _StatusBadge(
            controller.macosImeBadgeText,
            ready: controller.macosImeBadge == ImeBadgeState.ready,
          ),
    children: [
      if (controller.macosIme?.appPath case final path?)
        ValueRow(AppStrings.systemLocation, path),
      if (controller.macosImeError case final error?) ErrorMessage(error),
    ],
  );
}

class AndroidImeCard extends StatelessWidget {
  const AndroidImeCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    Widget status(String label, bool done) => Row(
      children: [
        Icon(
          done ? FLucideIcons.circleCheck : FLucideIcons.circle,
          size: 18,
          color: done ? successColor(context) : null,
        ),
        const SizedBox(width: 8),
        Expanded(child: Text(label)),
      ],
    );
    return Section(
      title: AppStrings.imeSettings,
      icon: FLucideIcons.keyboard,
      children: [
        ValueRow(AppStrings.directory, c.defaultDir),
        if (c.storage != null && c.storage!['granted'] != true)
          ActionRow(
            children: [
              ActionButton(
                AppStrings.openStoragePermission,
                icon: FLucideIcons.folderOpen,
                onPress: c.canOpenStorageSettings
                    ? c.openStoragePermissionSettings
                    : null,
              ),
            ],
          ),
        status(AppStrings.enableKeytao, c.ime?['enabled'] == true),
        status(AppStrings.selectKeytao, c.ime?['selected'] == true),
        if (c.imeMessage?.isNotEmpty == true) StatusMessage(c.imeMessage!),
        ActionRow(
          children: [
            ActionButton(
              AppStrings.openSystemSettings,
              icon: FLucideIcons.settings,
              onPress: c.busy ? null : c.openInputMethodSettings,
            ),
            ActionButton(
              AppStrings.chooseKeytao,
              primary: true,
              icon: FLucideIcons.keyboard,
              onPress: c.busy || c.ime?['canShowPicker'] != true
                  ? null
                  : c.showInputMethodPicker,
            ),
            ActionButton(
              AppStrings.recheck,
              icon: FLucideIcons.refreshCw,
              onPress: c.busy ? null : c.refreshState,
            ),
          ],
        ),
      ],
    );
  }
}
