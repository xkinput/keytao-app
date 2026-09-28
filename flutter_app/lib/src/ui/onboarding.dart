import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'progress.dart';
import 'scheme_page.dart';
import 'widgets.dart';

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
  int _index = 0;
  bool _completionScheduled = false;
  bool _finished = false;
  bool _retargetPending = false;
  late final AppLifecycleListener _lifecycle;
  List<SetupStep> get _steps => widget.controller.isAndroid
      ? const [
          SetupStep.enable,
          SetupStep.select,
          SetupStep.storage,
          SetupStep.install,
          SetupStep.deploy,
        ]
      : widget.controller.setupSteps;
  bool _done(SetupStep step) =>
      step == SetupStep.storage && widget.controller.isAndroid
      ? widget.controller.migrationReady
      : step.isComplete(widget.controller);
  int get _firstIncomplete => _steps.indexWhere((step) => !_done(step));
  int get _initialIndex {
    final first = _firstIncomplete;
    return first < 0 ? _steps.length - 1 : first;
  }

  @override
  void initState() {
    super.initState();
    _index = _initialIndex;
    _retargetPending = widget.controller.busy;
    widget.controller.addListener(_onControllerChanged);
    _lifecycle = AppLifecycleListener(
      onResume: () {
        _retargetPending = true;
        _retargetWhenIdle();
      },
    );
    widget.controller.setWindowTitle(_steps[_index].title);
    _checkCompletion();
  }

  @override
  void didUpdateWidget(covariant OnboardingPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_onControllerChanged);
      widget.controller.addListener(_onControllerChanged);
      _index = _initialIndex;
      _retargetPending = widget.controller.busy;
      widget.controller.setWindowTitle(_steps[_index].title);
    }
    _checkCompletion();
  }

  void _onControllerChanged() {
    setState(() {});
    if (_retargetPending) _retargetWhenIdle();
    _checkCompletion();
  }

  void _retargetWhenIdle() {
    // The app owns refreshState; wait for its resume refresh (including any
    // refresh queued behind a running operation) before choosing the page.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_retargetPending || widget.controller.busy) return;
      _retargetPending = false;
      _move(_initialIndex);
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  void _checkCompletion() {
    final c = widget.controller;
    if (!c.isAndroid ||
        _finished ||
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

  void _move(int index) {
    FocusScope.of(context).unfocus();
    setState(() => _index = index);
    widget.controller.setWindowTitle(_steps[index].title);
  }

  Future<void> _next() async {
    final first = _firstIncomplete;
    if (first >= 0 && first < _index) {
      _move(first);
      return;
    }
    if (_index == _steps.length - 1) {
      final completed = await widget.controller.finishOnboarding();
      if (!mounted) return;
      if (completed) {
        widget.onFinished();
      } else if (_firstIncomplete >= 0) {
        _move(_firstIncomplete);
      }
    } else {
      _move(_index + 1);
    }
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    widget.controller.removeListener(_onControllerChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final steps = _steps;
    final step = steps[_index];
    final title = !c.isAndroid
        ? step.title
        : switch (step) {
            SetupStep.storage => AppStrings.grantStorage,
            SetupStep.install => AppStrings.installKeytao,
            SetupStep.deploy => AppStrings.deployKeytao,
            _ => step.title,
          };
    final description = switch (step) {
      SetupStep.enable => AppStrings.enableDescription,
      SetupStep.select => AppStrings.selectDescription,
      SetupStep.storage || SetupStep.migration => c.permissionDescription,
      SetupStep.install => AppStrings.installDescription,
      SetupStep.deploy => AppStrings.deployDescription,
      SetupStep.component => '安装 KeyTao 输入法组件后，重新检测安装状态。',
      SetupStep.finish =>
        c.setupReady ? '设置已就绪，点击完成开始使用 KeyTao。' : AppStrings.finishSetupFirst,
    };
    final errors = <String>{
      if (c.isAndroid) ...[
        if (c.error?.isNotEmpty == true) c.error!,
        if (c.migrationError?.isNotEmpty == true) c.migrationError!,
        if (c.installError?.isNotEmpty == true) c.installError!,
        if (c.releaseError?.isNotEmpty == true) c.releaseError!,
        if ((c.storage?['deployError'] as String?)?.isNotEmpty == true)
          c.storage!['deployError'] as String,
      ] else ...[
        if (step == SetupStep.component && c.macosImeError != null)
          c.macosImeError!,
        if (step == SetupStep.install && c.installError != null)
          c.installError!,
      ],
    };
    double progress;
    if (!c.isAndroid) {
      progress = (_index + (_done(step) ? 1 : 0)) / steps.length;
    } else if (c.setupReady) {
      progress = 1;
    } else if (c.local?.installed == true) {
      progress = .9;
    } else if (c.migrationReady) {
      progress = .72;
    } else if (c.ime?['selected'] == true) {
      progress = .55;
    } else if (c.ime?['enabled'] == true) {
      progress = .36;
    } else {
      progress = c.ime != null ? .18 : .08;
    }
    final onboarding = FScaffold(
      scaffoldStyle: FScaffoldStyleDelta.delta(
        systemOverlayStyle: c.isAndroid ? AppSystemBars.styleOf(context) : null,
      ),
      childPad: false,
      child: SafeArea(
        child: Column(
          children: [
            Expanded(
              child: SingleChildScrollView(
                key: ValueKey('setup-${step.name}'),
                padding: const EdgeInsets.all(24),
                child: Align(
                  alignment: Alignment.topCenter,
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 640),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      spacing: 24,
                      children: [
                        Row(
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
                                    style: context.theme.typography.display.lg
                                        .copyWith(fontWeight: FontWeight.w600),
                                  ),
                                  Text(
                                    c.isAndroid
                                        ? AppStrings.androidOnboarding
                                        : 'KeyTao ${c.info.appVersion}',
                                    style: context.theme.typography.body.xs,
                                  ),
                                ],
                              ),
                            ),
                            if (c.isAndroid)
                              FBadge(
                                child: Text(
                                  c.setupReady
                                      ? AppStrings.ready
                                      : AppStrings.needsSetup,
                                ),
                              ),
                          ],
                        ),
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          spacing: 8,
                          children: [
                            for (var i = 0; i < steps.length; i++)
                              Expanded(
                                child: Column(
                                  crossAxisAlignment:
                                      CrossAxisAlignment.stretch,
                                  spacing: 6,
                                  children: [
                                    FButton(
                                      key: ValueKey(
                                        'setup-nav-${steps[i].name}',
                                      ),
                                      variant: i == _index
                                          ? FButtonVariant.primary
                                          : FButtonVariant.outline,
                                      selected: i == _index,
                                      semanticsLabel:
                                          '${i + 1}. ${steps[i].title}，${_done(steps[i]) ? '已完成' : '待完成'}',
                                      onPress: () => _move(i),
                                      child: _done(steps[i])
                                          ? const Icon(
                                              FLucideIcons.check,
                                              size: 16,
                                            )
                                          : Text('${i + 1}'),
                                    ),
                                    Text(
                                      steps[i].title,
                                      textAlign: TextAlign.center,
                                      style: context.theme.typography.body.xs,
                                    ),
                                  ],
                                ),
                              ),
                          ],
                        ),
                        Row(
                          children: [
                            Expanded(
                              child: Text(
                                title,
                                style: context.theme.typography.display.xl2
                                    .copyWith(fontWeight: FontWeight.w600),
                              ),
                            ),
                            Text('${_index + 1} / ${steps.length}'),
                          ],
                        ),
                        FDeterminateProgress(value: progress),
                        Section(
                          title: c.isAndroid ? AppStrings.imeSettings : '设置状态',
                          children: [
                            SettingRow(
                              _done(step) ? '已完成' : '待完成',
                              child: ActionButton(
                                AppStrings.recheck,
                                icon: FLucideIcons.refreshCw,
                                onPress: c.busy ? null : c.refreshState,
                              ),
                            ),
                            Text(description),
                            if (step == SetupStep.component &&
                                !_done(step)) ...[
                              ValueRow(
                                '输入法组件',
                                c.macosIme?.message ?? '未检测到 KeyTao 输入法组件',
                              ),
                              ActionRow(
                                children: [
                                  ActionButton(
                                    '下载安装包',
                                    primary: true,
                                    onPress: c.busy
                                        ? null
                                        : () =>
                                              c.open('$repositoryUrl/releases'),
                                  ),
                                ],
                              ),
                            ],
                            if (step == SetupStep.enable)
                              ActionRow(
                                children: [
                                  ActionButton(
                                    AppStrings.openSystemSettings,
                                    primary: true,
                                    onPress: c.busy
                                        ? null
                                        : c.openInputMethodSettings,
                                  ),
                                ],
                              ),
                            if (step == SetupStep.select)
                              ActionRow(
                                children: [
                                  ActionButton(
                                    AppStrings.chooseKeytao,
                                    primary: true,
                                    onPress:
                                        c.busy ||
                                            c.ime?['canShowPicker'] != true
                                        ? null
                                        : c.showInputMethodPicker,
                                  ),
                                ],
                              ),
                            if (step == SetupStep.storage)
                              ActionRow(
                                children: [
                                  ActionButton(
                                    c.storage?['granted'] == true
                                        ? AppStrings.retryMigration
                                        : AppStrings.authorizeStorage,
                                    primary: true,
                                    onPress: c.canOpenStorageSettings
                                        ? c.openStoragePermissionSettings
                                        : null,
                                  ),
                                ],
                              ),
                            if (step == SetupStep.install) ...[
                              if (!c.isAndroid) ...[
                                SchemeSelection(controller: c),
                                VersionPicker(controller: c),
                              ],
                              ActionRow(
                                children: [
                                  ActionButton(
                                    c.isAndroid
                                        ? c.isInstalling
                                              ? AppStrings.installing
                                              : AppStrings.installKeytao
                                        : c.installButtonTitle,
                                    primary: true,
                                    onPress: c.canInstallDuringSetup
                                        ? c.install
                                        : null,
                                  ),
                                ],
                              ),
                            ],
                            if (step == SetupStep.deploy)
                              ActionRow(
                                children: [
                                  ActionButton(
                                    c.isAndroid
                                        ? c.isDeploying
                                              ? AppStrings.deploying
                                              : AppStrings.deployKeytao
                                        : '部署方案',
                                    primary: true,
                                    onPress: c.canDeploy ? c.deploy : null,
                                  ),
                                ],
                              ),
                            if (c.isAndroid && c.imeMessage?.isNotEmpty == true)
                              StatusMessage(c.imeMessage!),
                            if (c.isAndroid &&
                                (c.storage?['message'] as String?)
                                        ?.isNotEmpty ==
                                    true)
                              StatusMessage(c.storage!['message'] as String),
                            for (final error in errors) ErrorMessage(error),
                            if (c.isAndroid ||
                                step == SetupStep.install ||
                                step == SetupStep.deploy) ...[
                              if (c.isInstalling || c.checkingStorage)
                                InstallProgressView(
                                  active: true,
                                  progress: c.installProgress,
                                  fraction: c.installFraction,
                                  message: c.progress,
                                ),
                              if (c.deploySteps.isNotEmpty)
                                DeploySteps(controller: c),
                              if (!c.isAndroid && c.operationLogCount > 0)
                                ActionRow(
                                  children: [OperationLogButton(controller: c)],
                                ),
                            ],
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(24),
              child: Align(
                alignment: Alignment.bottomCenter,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 640),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: [
                      ActionButton(
                        '返回',
                        onPress: _index == 0 || c.busy
                            ? null
                            : () => _move(_index - 1),
                      ),
                      const SizedBox(width: 8),
                      ActionButton(
                        _index == steps.length - 1 ? '完成' : '继续',
                        primary: true,
                        onPress:
                            !c.busy &&
                                (_done(step) ||
                                    (_firstIncomplete >= 0 &&
                                        _firstIncomplete < _index))
                            ? _next
                            : null,
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
    return c.isAndroid ? AppSystemBars(child: onboarding) : onboarding;
  }
}
