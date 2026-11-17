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
              spacing: 24,
              children: [
                if (controller.isAndroid)
                  AndroidImeCard(controller: controller),
                if (controller.info.platform == BridgePlatform.macOs)
                  MacosImeCard(controller: controller),
                if (controller.isWindows)
                  WindowsImeCard(controller: controller),
                if (controller.isLinux) LinuxImeCard(controller: controller),
                if (controller.isIos) IosImeCard(controller: controller),
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

class _CurrentSchemeHero extends StatelessWidget {
  const _CurrentSchemeHero({required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    final palette = AppPalette.of(context);
    final package = c.installedPackageFor(c.activeSchemeKey);
    final local = c.local;
    final version =
        package?.version ??
        (local == null
            ? null
            : displayedSchemeVersion(
                local.schemas,
                local.version,
                local.installedScheme,
                local.installedSchemeVersion,
              ));
    final deployed = local?.deployed == true;
    final deployLabel = local == null
        ? AppStrings.checking
        : !local.installed
        ? AppStrings.notInstalled
        : deployed
        ? AppStrings.deployed
        : AppStrings.pendingDeploy;
    final title =
        package?.displayName ??
        (local?.installed == true
            ? schemeDisplayName(c.activeSchemeKey)
            : local == null
            ? AppStrings.checking
            : AppStrings.notInstalled);
    final source = downloadSources[package?.source];
    return Container(
      margin: desktop ? EdgeInsets.zero : const EdgeInsets.only(top: 4),
      padding: desktop
          ? const EdgeInsets.symmetric(horizontal: 32, vertical: 40)
          : const EdgeInsets.all(16),
      decoration: desktop
          ? null
          : BoxDecoration(
              color: palette.surface,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(18),
            ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: desktop ? 20 : 12,
        children: [
          EyebrowLabel(
            desktop ? '当前方案 · ${c.aboutValues['平台']} 系统输入法' : '当前方案',
          ),
          Row(
            spacing: 16,
            children: [
              SchemaLogo(controller: c, size: desktop ? 52 : 36),
              Expanded(
                child: Text(
                  title,
                  style: context.theme.typography.body.sm.copyWith(
                    color: palette.ink,
                    fontSize: desktop ? 56 : 32,
                    fontWeight: FontWeight.w700,
                    height: 1.05,
                    letterSpacing: desktop ? -1.12 : -.32,
                  ),
                ),
              ),
            ],
          ),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              if (version?.isNotEmpty == true)
                Text(version!.startsWith('v') ? version : 'v$version'),
              AppBadge(
                deployLabel,
                background: deployed
                    ? palette.successBackground
                    : palette.controlSurface,
                foreground: deployed
                    ? palette.successForeground
                    : palette.muted,
              ),
              if (source != null) Text('来自 $source'),
            ],
          ),
          ActionRow(
            children: [
              ActionButton(
                AppStrings.redeploy,
                primary: true,
                tooltip: c.deployTooltip,
                onPress: c.canDeploy ? c.deploy : null,
              ),
              ActionButton('切换方案', onPress: () => c.selectPage(AppPage.scheme)),
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
    trailing: _StatusBadge(
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
