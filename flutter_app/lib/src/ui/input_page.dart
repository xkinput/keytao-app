import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'settings.dart';
import 'progress.dart';
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
    children: [
      if (controller.userWordsReport != null ||
          controller.userWordsReportError != null)
        UserWordsNotice(controller: controller),
      if (controller.isAndroid) ...[
        AppBrand(version: controller.info.appVersion),
        AndroidImeCard(controller: controller),
      ],
      if (controller.info.platform == BridgePlatform.macOs)
        MacosImeCard(controller: controller),
      if (controller.isWindows) WindowsImeCard(controller: controller),
      if (controller.isLinux) LinuxImeCard(controller: controller),
      if (controller.isIos) IosImeCard(controller: controller),
      if (controller.isWindows && controller.deploySteps.isNotEmpty)
        DeploySteps(controller: controller),
      SettingsCard(controller: controller),
      AccountCard(controller: controller),
    ],
  );
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
