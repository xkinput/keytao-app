import 'package:flutter/material.dart';

import '../app/controller.dart';
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

  List<({String title, bool done, Widget content})> _steps(AppController c) => [
    if (c.isAndroid) ...[
      (
        title: '文件访问权限',
        done: c.storageReady,
        content: Column(
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
      ),
      (
        title: '数据迁移',
        done: c.migrationReady,
        content: Column(
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
      ),
    ] else
      (
        title: '输入法组件',
        done: c.macosIme?.installed == true,
        content: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (c.macosIme?.installed != true) ...[
              Text(c.macosIme?.message ?? '未检测到 KeyTao 输入法组件'),
              const SizedBox(height: 24),
              FilledButton(
                onPressed: c.busy
                    ? null
                    : () => c.open('$repositoryUrl/releases'),
                child: const Text('下载安装包'),
              ),
            ],
          ],
        ),
      ),
    (
      title: '安装方案',
      done: c.local?.installed == true,
      content: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SchemeSelection(controller: c),
          FilledButton(
            onPressed: c.busy || (c.isAndroid && !c.migrationReady)
                ? null
                : c.install,
            child: Text(c.local?.installed == true ? '重新安装' : '安装方案'),
          ),
          OperationProgress(controller: c),
        ],
      ),
    ),
    (
      title: '部署方案',
      done: c.local?.deployed == true,
      content: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (c.storage?['deployError'] case final String message
              when message.isNotEmpty) ...[
            SelectableText(message),
            const SizedBox(height: 16),
          ],
          FilledButton(
            onPressed: c.busy || c.local?.installed != true ? null : c.deploy,
            child: const Text('部署方案'),
          ),
          OperationProgress(controller: c),
        ],
      ),
    ),
    if (c.isAndroid) ...[
      (
        title: '启用 KeyTao',
        done: c.ime?['enabled'] == true,
        content: FilledButton(
          onPressed: c.busy
              ? null
              : () => c.run(c.android.openInputMethodSettings),
          child: const Text('打开输入法设置'),
        ),
      ),
      (
        title: '切换到 KeyTao',
        done: c.ime?['selected'] == true,
        content: FilledButton(
          onPressed: c.busy || c.ime?['canShowPicker'] != true
              ? null
              : () => c.run(c.android.showInputMethodPicker),
          child: const Text('选择 KeyTao'),
        ),
      ),
    ],
    (
      title: '完成',
      done: c.setupReady,
      content: c.isAndroid
          ? const SizedBox.shrink()
          : const Text('注销后，在系统设置 › 键盘 › 输入法中添加 KeyTao。'),
    ),
  ];

  Future<void> _next(int count) async {
    if (_index == count - 1) {
      final finished = await widget.controller.finishOnboarding();
      if (mounted && finished) widget.onFinished();
    } else {
      await _go(_index + 1);
    }
  }

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
    final steps = _steps(c);
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
                                step.done
                                    ? Icons.check_circle_rounded
                                    : Icons.radio_button_unchecked_rounded,
                                color: Theme.of(context).colorScheme.primary,
                              ),
                              const SizedBox(width: 8),
                              Text(step.done ? '已完成' : '待完成'),
                              const Spacer(),
                              IconButton(
                                tooltip: '重新检测',
                                onPressed: c.busy ? null : c.refreshState,
                                icon: const Icon(Icons.refresh_rounded),
                              ),
                            ],
                          ),
                          const SizedBox(height: 24),
                          step.content,
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
                      onPressed: c.busy || !steps[_index].done
                          ? null
                          : () => _next(steps.length),
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
