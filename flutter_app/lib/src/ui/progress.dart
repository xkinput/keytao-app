import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class InstallProgressView extends StatelessWidget {
  const InstallProgressView({
    super.key,
    required this.active,
    this.progress,
    this.fraction,
    this.message,
  });
  final bool active;
  final InstallProgressDto? progress;
  final double? fraction;
  final String? message;
  @override
  Widget build(BuildContext context) {
    if (!active) return const SizedBox.shrink();
    final value = progress == null
        ? fraction
        : progress!.percent.clamp(0, 100) / 100;
    final text = progress?.message ?? message;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (value == null)
          const FProgress()
        else
          FDeterminateProgress(value: value.clamp(0, 1)),
        if (text != null) ...[
          const SizedBox(height: 8),
          Semantics(liveRegion: true, child: Text(text)),
        ],
      ],
    );
  }
}

class DeploySteps extends StatelessWidget {
  const DeploySteps({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (final step in controller.deploySteps)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 5),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (step.state == DeployStepState.neutral &&
                  controller.isDeploying &&
                  identical(step, controller.deploySteps.last))
                const SizedBox.square(dimension: 18, child: FCircularProgress())
              else
                Icon(
                  switch (step.state) {
                    DeployStepState.ok => FLucideIcons.circleCheck,
                    DeployStepState.failed => FLucideIcons.circleX,
                    DeployStepState.neutral => FLucideIcons.refreshCw,
                  },
                  size: 18,
                  color: _color(context, step.state),
                ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  step.message,
                  style: TextStyle(color: _color(context, step.state)),
                ),
              ),
            ],
          ),
        ),
    ],
  );
  Color _color(BuildContext context, DeployStepState state) => switch (state) {
    DeployStepState.ok => successColor(context),
    DeployStepState.failed => context.theme.colors.destructive,
    DeployStepState.neutral => context.theme.colors.mutedForeground,
  };
}

class OperationLogButton extends StatelessWidget {
  const OperationLogButton({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) => ActionButton(
    AppStrings.operationLogCount(controller.operationLogCount),
    icon: FLucideIcons.scrollText,
    onPress: () => showFDialog<void>(
      context: context,
      builder: (context, _, animation) => ListenableBuilder(
        listenable: controller,
        builder: (context, _) => AppDialog(
          title: AppStrings.operationLogs,
          animation: animation,
          actions: [
            ActionButton(
              AppStrings.clearOperationLogs,
              onPress: controller.isAndroid && controller.operationLogs.isEmpty
                  ? null
                  : controller.clearOperationLogs,
            ),
            ActionButton(
              AppStrings.close,
              primary: true,
              onPress: () => Navigator.pop(context),
            ),
          ],
          child: SizedBox(
            width: 600,
            child: LogLines(
              lines: controller.operationLogs,
              storageKey: 'operationLogs',
              height: (MediaQuery.sizeOf(context).height * .5).clamp(160, 400),
              colorCoded: true,
            ),
          ),
        ),
      ),
    ),
  );
}
