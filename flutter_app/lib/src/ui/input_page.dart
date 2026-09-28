import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'settings.dart';
import 'progress.dart';
import 'widgets.dart';

class InputPage extends StatelessWidget {
  const InputPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => AppPageBody(
    controller: controller,
    page: AppPage.input,
    children: [
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
      trailing: FBadge(
        variant: FBadgeVariant.outline,
        child: Text(
          c.windowsRegistrationLabel,
          style: TextStyle(
            color: !c.windowsRegistrationBusy && s?.registered == true
                ? successColor(context)
                : s?.registrationState == 'failed'
                ? context.theme.colors.destructive
                : null,
          ),
        ),
      ),
      children: [
        if (c.windowsRegistrationBusy) const FProgress(),
        if (s == null) const StatusMessage(AppStrings.windowsChecking),
        if (s != null) ...[
          ActionRow(
            children: [
              FBadge(
                variant: s.packaged
                    ? FBadgeVariant.primary
                    : FBadgeVariant.outline,
                child: Text(AppStrings.windowsPackage(s.packaged)),
              ),
              FBadge(
                variant: s.registered
                    ? FBadgeVariant.primary
                    : FBadgeVariant.outline,
                child: Text(
                  AppStrings.windowsRegistration(c.windowsRegistrationLabel),
                ),
              ),
              FBadge(
                variant: s.registeredDll
                    ? FBadgeVariant.primary
                    : FBadgeVariant.outline,
                child: Text(AppStrings.windowsDll(s.registeredDll)),
              ),
              FBadge(
                variant: s.profileEnabled
                    ? FBadgeVariant.primary
                    : FBadgeVariant.outline,
                child: Text(AppStrings.windowsProfile(s.profileEnabled)),
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
      trailing: FBadge(
        variant: FBadgeVariant.outline,
        child: Text(
          s?.running == true ? AppStrings.running : AppStrings.notRunning,
          style: TextStyle(
            color: s?.running == true ? successColor(context) : null,
          ),
        ),
      ),
      children: [
        if (s != null) ...[
          ValueRow(AppStrings.command, s.command),
          ActionRow(
            children: [
              FBadge(
                variant: FBadgeVariant.secondary,
                child: Text(
                  s.kdeSession
                      ? AppStrings.kdeSession
                      : AppStrings.nonKdeSession,
                ),
              ),
              FBadge(
                variant: s.kdeConfigured
                    ? FBadgeVariant.primary
                    : FBadgeVariant.outline,
                child: Text(AppStrings.kdeConfiguration(s.kdeConfigured)),
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
    title: AppStrings.iosOnboarding,
    icon: FLucideIcons.keyboard,
    children: [
      const Text(AppStrings.iosOpenApp),
      const Text(AppStrings.iosAddKeyboard),
      const Text(AppStrings.iosChooseKeyboard),
      const Text(AppStrings.iosEnableFullAccess),
      const Text(AppStrings.iosFullAccessDescription),
      const Text(AppStrings.iosInstallDeploy),
      const Text(AppStrings.iosGlobe),
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
    trailing: FBadge(
      variant: FBadgeVariant.outline,
      child: Text(
        controller.macosImeBadgeText,
        style: TextStyle(
          color: controller.macosImeBadge == ImeBadgeState.ready
              ? successColor(context)
              : null,
        ),
      ),
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
        const SizedBox(width: 10),
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
              tooltip: AppStrings.recheck,
              onPress: c.busy ? null : c.refreshState,
            ),
          ],
        ),
      ],
    );
  }
}
