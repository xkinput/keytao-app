import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../scheme/scheme_card.dart';

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
  final _pages = PageController();
  int _index = 0;
  @override
  void dispose() {
    _pages.dispose();
    super.dispose();
  }

  Widget _content(SetupStep step, AppController c) => switch (step) {
    SetupStep.storage => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SelectableText(c.storage?['path'] as String? ?? c.info.userRoot),
        if (!c.storageReady) ...[
          const SizedBox(height: 16),
          Text(c.storage?['message'] as String? ?? '请授权 KeyTao 访问文件'),
          const SizedBox(height: 24),
          FilledButton(
            onPressed: c.busy || c.storage?['canOpenSettings'] != true
                ? null
                : () => c.run(c.android.openStoragePermissionSettings),
            child: const Text('授权文件访问'),
          ),
        ],
      ],
    ),
    SetupStep.migration => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (c.storage?['migrationError'] case final String message
            when message.isNotEmpty)
          SelectableText(message),
        if (!c.storageReady) const Text('请先授权文件访问'),
        if (!c.migrationReady) ...[
          const SizedBox(height: 16),
          OutlinedButton(
            onPressed: c.busy
                ? null
                : () => c.run(c.android.openStoragePermissionSettings),
            child: const Text('打开存储设置'),
          ),
        ],
      ],
    ),
    SetupStep.component => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (!step.isComplete(c)) ...[
          Text(c.macosIme?.message ?? '未检测到 KeyTao 输入法组件'),
          const SizedBox(height: 24),
          FilledButton(
            onPressed: c.busy ? null : () => c.open('$repositoryUrl/releases'),
            child: const Text('下载安装包'),
          ),
        ],
      ],
    ),
    SetupStep.install => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SchemeSelection(controller: c),
        FilledButton(
          onPressed: c.canInstallDuringSetup ? c.install : null,
          child: Text(c.installButtonTitle),
        ),
        OperationProgress(controller: c),
      ],
    ),
    SetupStep.deploy => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (c.storage?['deployError'] case final String message
            when message.isNotEmpty) ...[
          SelectableText(message),
          const SizedBox(height: 16),
        ],
        FilledButton(
          onPressed: c.canDeploy ? c.deploy : null,
          child: const Text('部署方案'),
        ),
        OperationProgress(controller: c),
      ],
    ),
    SetupStep.enable => FilledButton(
      onPressed: c.busy ? null : () => c.run(c.android.openInputMethodSettings),
      child: const Text('打开输入法设置'),
    ),
    SetupStep.select => FilledButton(
      onPressed: c.busy || c.ime?['canShowPicker'] != true
          ? null
          : () => c.run(c.android.showInputMethodPicker),
      child: const Text('选择 KeyTao'),
    ),
    SetupStep.finish =>
      c.isAndroid
          ? const SizedBox.shrink()
          : const Text('注销后，在系统设置 › 键盘 › 输入法中添加 KeyTao。'),
  };

  Future<void> _next() => widget.controller.setupSteps[_index].advance(
    widget.controller,
    onNext: () => _go(_index + 1),
    onFinished: () {
      if (mounted) widget.onFinished();
    },
  );

  Future<void> _go(int index) => _pages.animateToPage(
    index,
    duration: MediaQuery.disableAnimationsOf(context)
        ? Duration.zero
        : const Duration(milliseconds: 280),
    curve: Curves.easeInOutCubic,
  );

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final steps = c.setupSteps;
    return Scaffold(
      appBar: AppBar(title: Text('KeyTao ${c.info.appVersion}')),
      body: SafeArea(
        child: ContentWidth(
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(24, 16, 24, 24),
                child: Row(
                  children: [
                    Expanded(
                      child: LinearProgressIndicator(
                        value: (_index + 1) / steps.length,
                        borderRadius: BorderRadius.circular(8),
                      ),
                    ),
                    const SizedBox(width: 16),
                    Text('${_index + 1} / ${steps.length}'),
                  ],
                ),
              ),
              Expanded(
                child: PageView.builder(
                  controller: _pages,
                  physics: const NeverScrollableScrollPhysics(),
                  itemCount: steps.length,
                  onPageChanged: (index) => setState(() => _index = index),
                  itemBuilder: (context, index) {
                    final step = steps[index];
                    return SingleChildScrollView(
                      padding: const EdgeInsets.symmetric(horizontal: 24),
                      child: SectionCard(
                        title: step.title,
                        children: [
                          Row(
                            children: [
                              Icon(
                                step.isComplete(c)
                                    ? Icons.check_circle_rounded
                                    : Icons.radio_button_unchecked_rounded,
                                color: Theme.of(context).colorScheme.primary,
                              ),
                              const SizedBox(width: 8),
                              Text(step.isComplete(c) ? '已完成' : '待完成'),
                              const Spacer(),
                              IconButton(
                                tooltip: '重新检测',
                                onPressed: c.busy ? null : c.refreshState,
                                icon: const Icon(Icons.refresh_rounded),
                              ),
                            ],
                          ),
                          const SizedBox(height: 24),
                          _content(step, c),
                          if (c.error != null) ...[
                            const SizedBox(height: 16),
                            Semantics(
                              liveRegion: true,
                              child: SelectableText(
                                c.error!,
                                style: TextStyle(
                                  color: Theme.of(context).colorScheme.error,
                                ),
                              ),
                            ),
                          ],
                        ],
                      ),
                    );
                  },
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(24),
                child: Row(
                  children: [
                    TextButton(
                      onPressed: _index == 0 || c.busy
                          ? null
                          : () => _go(_index - 1),
                      child: const Text('上一步'),
                    ),
                    const Spacer(),
                    FilledButton(
                      onPressed: !steps[_index].canContinue(c) ? null : _next,
                      child: Text(_index == steps.length - 1 ? '完成' : '下一步'),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
