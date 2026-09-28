import 'package:flutter/widgets.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import 'app.dart';
import 'pages.dart';
import 'widgets.dart';

class MacosOnboarding extends StatefulWidget {
  const MacosOnboarding({
    super.key,
    required this.controller,
    required this.onFinished,
  });
  final AppController controller;
  final VoidCallback onFinished;

  @override
  State<MacosOnboarding> createState() => _MacosOnboardingState();
}

class _MacosOnboardingState extends State<MacosOnboarding> {
  int _index = 0;

  Future<void> _next() => widget.controller.setupSteps[_index].advance(
    widget.controller,
    onNext: () => setState(() => _index++),
    onFinished: () {
      if (mounted) widget.onFinished();
    },
  );

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final steps = c.setupSteps;
    final step = steps[_index];
    return MacosWindowTitle(
      controller: c,
      title: step.title,
      child: MacosWindow(
        child: MacosScaffold(
          toolBar: ToolBar(
            title: Text('KeyTao ${c.info.appVersion}'),
            automaticallyImplyLeading: false,
            titleWidth: 240,
          ),
          children: [
            ContentArea(
              builder: (context, scrollController) => Column(
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(32, 24, 32, 8),
                    child: Row(
                      children: [
                        Image.asset(logoAsset, width: 44, height: 44),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              const Text(AppStrings.title),
                              Text(
                                step.title,
                                style: MacosTheme.of(context).typography.title1,
                              ),
                            ],
                          ),
                        ),
                        Text('${_index + 1} / ${steps.length}'),
                      ],
                    ),
                  ),
                  Expanded(
                    child: MacosForm(
                      scrollController: scrollController,
                      children: [
                        MacosGroup(
                          title: '设置状态',
                          children: [
                            MacosSettingRow(
                              step.isComplete(c) ? '已完成' : '待完成',
                              child: PushButton(
                                controlSize: ControlSize.regular,
                                secondary: true,
                                onPressed: c.busy ? null : c.refreshState,
                                child: const Text('重新检测'),
                              ),
                            ),
                            ...switch (step) {
                              SetupStep.component => [
                                if (!step.isComplete(c)) ...[
                                  MacosValueRow(
                                    '输入法组件',
                                    c.macosIme?.message ?? '未检测到 KeyTao 输入法组件',
                                  ),
                                  MacosActionRow(
                                    children: [
                                      PushButton(
                                        controlSize: ControlSize.regular,
                                        onPressed: c.busy
                                            ? null
                                            : () => c.open(
                                                '$repositoryUrl/releases',
                                              ),
                                        child: const Text('下载安装包'),
                                      ),
                                    ],
                                  ),
                                ],
                              ],
                              SetupStep.install => [
                                MacosSchemeSelection(controller: c),
                                MacosActionRow(
                                  children: [
                                    PushButton(
                                      controlSize: ControlSize.regular,
                                      onPressed: c.canInstallDuringSetup
                                          ? c.install
                                          : null,
                                      child: Text(c.installButtonTitle),
                                    ),
                                  ],
                                ),
                                MacosOperationProgress(controller: c),
                              ],
                              SetupStep.deploy => [
                                MacosActionRow(
                                  children: [
                                    PushButton(
                                      controlSize: ControlSize.regular,
                                      onPressed: c.canDeploy ? c.deploy : null,
                                      child: const Text('部署方案'),
                                    ),
                                  ],
                                ),
                                MacosOperationProgress(controller: c),
                              ],
                              _ => <Widget>[],
                            },
                          ],
                        ),
                      ],
                    ),
                  ),
                  Padding(
                    padding: const EdgeInsets.all(24),
                    child: Row(
                      mainAxisAlignment: MainAxisAlignment.end,
                      children: [
                        PushButton(
                          controlSize: ControlSize.regular,
                          secondary: true,
                          onPressed: _index == 0 || c.busy
                              ? null
                              : () => setState(() => _index--),
                          child: const Text('返回'),
                        ),
                        const SizedBox(width: 8),
                        PushButton(
                          controlSize: ControlSize.regular,
                          onPressed: step.canContinue(c) ? _next : null,
                          child: Text(step == SetupStep.finish ? '完成' : '继续'),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
