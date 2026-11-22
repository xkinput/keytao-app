import 'dart:async';

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

class _OnboardingPageState extends State<OnboardingPage>
    with WidgetsBindingObserver {
  int _index = 0;
  bool _completionAttempted = false;
  bool _finished = false;
  bool _retargetPending = false;
  Set<SetupStep> _detectedSteps = {};
  Timer? _iosAccessCheck;
  bool _checkingIosAccess = false;
  late final AppLifecycleListener _lifecycle;
  List<SetupStep> get _steps => widget.controller.setupSteps;
  bool _done(SetupStep step) => step.isComplete(widget.controller);
  int get _firstIncomplete => _steps.indexWhere((step) => !_done(step));
  int get _initialIndex {
    final first = _firstIncomplete;
    return first < 0 ? _steps.length - 1 : first;
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _index = _initialIndex;
    _detectedSteps = _steps.where(_done).toSet();
    _retargetPending = widget.controller.busy;
    widget.controller.addListener(_onControllerChanged);
    _lifecycle = AppLifecycleListener(
      onResume: () {
        if (widget.controller.isMobile) return;
        _retargetPending = true;
        _retargetWhenIdle();
      },
    );
    widget.controller.setWindowTitle(_steps[_index].title);
    _checkCompletion();
  }

  @override
  void didChangeLocales(List<Locale>? locales) {
    widget.controller.setWindowTitle(_steps[_index].title);
  }

  @override
  void didUpdateWidget(covariant OnboardingPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_onControllerChanged);
      widget.controller.addListener(_onControllerChanged);
      _index = _initialIndex;
      _detectedSteps = _steps.where(_done).toSet();
      _retargetPending = widget.controller.busy;
      _completionAttempted = false;
      _finished = false;
      _iosAccessCheck?.cancel();
      widget.controller.setWindowTitle(_steps[_index].title);
    }
    _checkCompletion();
  }

  void _onControllerChanged() {
    setState(() {});
    final c = widget.controller;
    if (c.isMobile && !c.busy) {
      final detected = _steps.where(_done).toSet();
      final advanced = detected.any((step) => !_detectedSteps.contains(step));
      _detectedSteps = detected;
      if (advanced && _index != _initialIndex) _move(_initialIndex);
    } else if (!c.isMobile && _retargetPending) {
      _retargetWhenIdle();
    }
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
    if (!c.isMobile || _finished || c.busy) return;
    if (!c.setupReady) {
      _completionAttempted = false;
      return;
    }
    if (_completionAttempted) return;
    _completionAttempted = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _finished || c != widget.controller) return;
      if (c.busy || !c.setupReady) {
        _completionAttempted = false;
        return;
      }
      _finish();
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  Future<void> _finish() async {
    final c = widget.controller;
    final completed = c.onboardingCompleted || await c.finishOnboarding();
    if (!mounted || _finished || c != widget.controller) return;
    if (completed) {
      _finished = true;
      FocusScope.of(context).unfocus();
      widget.onFinished();
    } else if (_firstIncomplete >= 0) {
      _move(_firstIncomplete);
    }
  }

  void _onIosInputFocus(bool focused) {
    _iosAccessCheck?.cancel();
    if (!focused) return;
    _iosAccessCheck = Timer.periodic(
      const Duration(milliseconds: 500),
      (_) => _checkIosAccess(),
    );
  }

  Future<void> _checkIosAccess() async {
    if (_checkingIosAccess) return;
    _checkingIosAccess = true;
    try {
      await widget.controller.refreshIosKeyboardStatus();
    } finally {
      _checkingIosAccess = false;
    }
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
      await _finish();
    } else {
      _move(_index + 1);
    }
  }

  @override
  void dispose() {
    _iosAccessCheck?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    _lifecycle.dispose();
    widget.controller.removeListener(_onControllerChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final steps = _steps;
    final step = steps[_index];
    final title = step == SetupStep.storage
        ? AppStrings.grantStorage
        : step.title;
    final description = switch (step) {
      SetupStep.enable =>
        c.isIos
            ? '${AppStrings.iosAddKeyboard}\n${AppStrings.iosChooseKeyboard}'
            : AppStrings.enableDescription,
      SetupStep.fullAccess =>
        '${AppStrings.iosEnableFullAccess}\n${AppStrings.iosFullAccessDescription}',
      SetupStep.select => AppStrings.selectDescription,
      SetupStep.storage => c.permissionDescription,
      SetupStep.install => AppStrings.installDescription,
      SetupStep.deploy => AppStrings.deployDescription,
      SetupStep.component => '安装 KeyTao 输入法组件后，重新检测安装状态。',
      SetupStep.finish =>
        c.setupReady ? '设置已就绪，点击完成开始使用 KeyTao。' : AppStrings.finishSetupFirst,
    };
    final errors = <String>{
      if (c.isMobile) ...[
        if (c.error?.isNotEmpty == true) c.error!,
      ] else ...[
        if (step == SetupStep.component && c.macosImeError != null)
          c.macosImeError!,
        if (step == SetupStep.install && c.installError != null)
          c.installError!,
      ],
    };
    final progress =
        (c.isMobile
            ? steps.where(_done).length
            : _index + (_done(step) ? 1 : 0)) /
        steps.length;
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
                padding: EdgeInsets.symmetric(
                  horizontal: MediaQuery.sizeOf(context).width >= 720 ? 32 : 16,
                  vertical: 24,
                ),
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
                                    c.isIos
                                        ? AppStrings.iosOnboarding
                                        : c.isAndroid
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
                          title: c.isMobile ? AppStrings.imeSettings : '设置状态',
                          children: [
                            Text(_done(step) ? '已完成' : '待完成'),
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
                            if (step == SetupStep.enable ||
                                step == SetupStep.fullAccess)
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
                                    AppStrings.authorizeStorage,
                                    primary: true,
                                    onPress: c.canOpenStorageSettings
                                        ? c.openStoragePermissionSettings
                                        : null,
                                  ),
                                ],
                              ),
                            if (step == SetupStep.fullAccess &&
                                c.iosKeyboardEnabled &&
                                !_done(step)) ...[
                              const Text(AppStrings.iosGlobe),
                              Focus(
                                onFocusChange: _onIosInputFocus,
                                child: FTextField(
                                  label: const Text(AppStrings.testInput),
                                  onSubmit: (_) => _checkIosAccess(),
                                ),
                              ),
                            ],
                            if (step == SetupStep.install) ...[
                              SchemeSelection(controller: c),
                              VersionPicker(controller: c),
                              ActionRow(
                                children: [
                                  ActionButton(
                                    c.installButtonTitle,
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
                                    AppStrings.deployScheme,
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
                            if (c.checkingStorage)
                              InstallProgressView(
                                active: true,
                                progress: c.installProgress,
                                fraction: c.installFraction,
                                message: c.progress,
                              ),
                            if (step == SetupStep.install ||
                                step == SetupStep.deploy) ...[
                              if (c.isInstalling)
                                InstallProgressView(
                                  active: true,
                                  progress: c.installProgress,
                                  fraction: c.installFraction,
                                  message: c.progress,
                                ),
                              if (c.deploySteps.isNotEmpty)
                                DeploySteps(controller: c),
                              if (c.operationLogCount > 0)
                                ActionRow(
                                  children: [OperationLogButton(controller: c)],
                                ),
                            ],
                            ActionRow(
                              children: [
                                ActionButton(
                                  AppStrings.recheck,
                                  icon: FLucideIcons.refreshCw,
                                  onPress: c.busy ? null : c.refreshState,
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
            Padding(
              padding: EdgeInsets.symmetric(
                horizontal: MediaQuery.sizeOf(context).width >= 720 ? 32 : 16,
                vertical: 24,
              ),
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
