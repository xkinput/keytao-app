import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../scheme/operation_progress.dart';

class OnboardingPage extends StatefulWidget {
  const OnboardingPage({
    super.key,
    required this.controller,
    required this.onFinished,
  });
  final AppController controller;
  final VoidCallback onFinished;

  @override
  State<OnboardingPage> createState() => _OnboardingPageState();
}

class _OnboardingPageState extends State<OnboardingPage> {
  bool _completionScheduled = false;
  bool _finished = false;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_checkCompletion);
    _checkCompletion();
  }

  @override
  void didUpdateWidget(covariant OnboardingPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_checkCompletion);
      widget.controller.addListener(_checkCompletion);
    }
    _checkCompletion();
  }

  void _checkCompletion() {
    final c = widget.controller;
    if (_finished ||
        _completionScheduled ||
        c.busy ||
        !c.setupReady ||
        !c.onboardingCompleted) {
      return;
    }
    _completionScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _completionScheduled = false;
      if (!mounted ||
          _finished ||
          !widget.controller.setupReady ||
          !widget.controller.onboardingCompleted) {
        return;
      }
      _finished = true;
      widget.onFinished();
    });
  }

  @override
  void dispose() {
    widget.controller.removeListener(_checkCompletion);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final enabled = c.ime?['enabled'] == true;
    final selected = c.ime?['selected'] == true;
    final storageReady = c.migrationReady;
    final installed = c.local?.installed == true;
    final deployed = c.local?.deployed == true;
    final steps = [
      (
        AppStrings.enableKeytao,
        AppStrings.enableDescription,
        enabled,
        Icons.keyboard_outlined,
      ),
      (
        AppStrings.selectKeytao,
        AppStrings.selectDescription,
        selected,
        Icons.keyboard_outlined,
      ),
      (
        AppStrings.grantStorage,
        c.permissionDescription,
        storageReady,
        Icons.folder_open,
      ),
      (
        AppStrings.installKeytao,
        AppStrings.installDescription,
        installed,
        Icons.download_outlined,
      ),
      (
        AppStrings.deployKeytao,
        AppStrings.deployDescription,
        deployed,
        Icons.play_arrow_outlined,
      ),
    ];
    final active = steps.indexWhere((step) => !step.$3);
    final (label, icon, action) = !enabled
        ? (
            AppStrings.openSystemSettings,
            Icons.settings_outlined,
            c.openInputMethodSettings,
          )
        : !selected
        ? (
            AppStrings.chooseKeytao,
            Icons.keyboard_outlined,
            c.ime?['canShowPicker'] == true ? c.showInputMethodPicker : null,
          )
        : !storageReady
        ? (
            c.storage?['granted'] == true
                ? AppStrings.retryMigration
                : AppStrings.authorizeStorage,
            Icons.folder_open,
            c.canOpenStorageSettings ? c.openStoragePermissionSettings : null,
          )
        : !installed
        ? (
            c.isInstalling ? AppStrings.installing : AppStrings.installKeytao,
            Icons.download_outlined,
            c.canInstall ? c.install : null,
          )
        : !deployed
        ? (
            c.isDeploying ? AppStrings.deploying : AppStrings.deployKeytao,
            Icons.play_arrow_outlined,
            c.canDeploy ? c.deploy : null,
          )
        : (AppStrings.recheck, Icons.refresh, c.refreshState);
    final progress = c.setupReady
        ? 1.0
        : installed
        ? .9
        : storageReady
        ? .72
        : selected
        ? .55
        : enabled
        ? .36
        : c.ime != null
        ? .18
        : .08;
    final errors = <String>{
      if (c.error?.isNotEmpty == true) c.error!,
      if (c.migrationError?.isNotEmpty == true) c.migrationError!,
      if (c.installError?.isNotEmpty == true) c.installError!,
      if (c.releaseError?.isNotEmpty == true) c.releaseError!,
      if ((c.storage?['deployError'] as String?)?.isNotEmpty == true)
        c.storage!['deployError'] as String,
    };
    return AndroidScaffold(
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 480),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 16),
                    child: Row(
                      children: [
                        Image.asset(
                          logoAsset,
                          width: 44,
                          height: 44,
                          semanticLabel: 'KeyTao',
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(
                                AppStrings.title,
                                style: Theme.of(context).textTheme.titleMedium,
                              ),
                              Text(
                                AppStrings.androidOnboarding,
                                style: Theme.of(context).textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 8),
                        Chip(
                          label: Text(
                            c.setupReady
                                ? AppStrings.ready
                                : AppStrings.needsSetup,
                          ),
                        ),
                      ],
                    ),
                  ),
                  SectionCard(
                    title: AppStrings.imeSettings,
                    icon: Icons.settings_outlined,
                    children: [
                      LinearProgressIndicator(value: progress),
                      const SizedBox(height: 16),
                      for (var index = 0; index < steps.length; index++)
                        OnboardingStep(
                          title: steps[index].$1,
                          description: steps[index].$2,
                          done: steps[index].$3,
                          icon: steps[index].$4,
                          active: index == active,
                        ),
                      if (c.imeMessage?.isNotEmpty == true)
                        StatusMessage(c.imeMessage!),
                      if ((c.storage?['message'] as String?)?.isNotEmpty ==
                          true)
                        StatusMessage(c.storage!['message'] as String),
                      for (final error in errors) ErrorMessage(error),
                      InstallProgressView(
                        active: c.isInstalling,
                        progress: c.installProgress,
                      ),
                      DeploySteps(controller: c),
                      const SizedBox(height: 16),
                      Row(
                        children: [
                          Expanded(
                            child: FilledButton.icon(
                              onPressed: c.busy ? null : action,
                              icon: c.isInstalling || c.isDeploying
                                  ? const SizedBox.square(
                                      dimension: 18,
                                      child: CircularProgressIndicator(
                                        strokeWidth: 2,
                                      ),
                                    )
                                  : Icon(icon),
                              label: Text(label),
                            ),
                          ),
                          const SizedBox(width: 8),
                          IconButton.outlined(
                            tooltip: AppStrings.recheck,
                            onPressed: c.busy ? null : c.refreshState,
                            icon: const Icon(Icons.refresh),
                          ),
                        ],
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class OnboardingStep extends StatelessWidget {
  const OnboardingStep({
    super.key,
    required this.title,
    required this.description,
    required this.done,
    required this.active,
    required this.icon,
  });
  final String title;
  final String description;
  final bool done;
  final bool active;
  final IconData icon;

  @override
  Widget build(BuildContext context) => Container(
    margin: const EdgeInsets.only(bottom: 8),
    padding: const EdgeInsets.all(12),
    decoration: BoxDecoration(
      color: active
          ? Theme.of(context).colorScheme.primaryContainer
          : Theme.of(context).colorScheme.surfaceContainer,
      borderRadius: BorderRadius.circular(12),
    ),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(
          done ? Icons.check_circle_outline : icon,
          size: 22,
          color: done
              ? successColor(context)
              : active
              ? Theme.of(context).colorScheme.primary
              : null,
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(title, style: Theme.of(context).textTheme.titleSmall),
              const SizedBox(height: 4),
              Text(description, style: Theme.of(context).textTheme.bodySmall),
            ],
          ),
        ),
      ],
    ),
  );
}
