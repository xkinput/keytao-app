import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';
import '../rust/api/types.dart';

class InstallProgressView extends StatelessWidget {
  const InstallProgressView({super.key, required this.active, this.progress});
  final bool active;
  final InstallProgressDto? progress;

  @override
  Widget build(BuildContext context) => !active
      ? const SizedBox.shrink()
      : Padding(
          padding: const EdgeInsets.symmetric(vertical: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              LinearProgressIndicator(
                value: progress == null
                    ? null
                    : progress!.percent.clamp(0, 100) / 100,
              ),
              if (progress != null) ...[
                const SizedBox(height: 8),
                Semantics(liveRegion: true, child: Text(progress!.message)),
              ],
            ],
          ),
        );
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
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (step.state == DeployStepState.running)
                const SizedBox.square(
                  dimension: 20,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else
                Icon(
                  step.state == DeployStepState.ok
                      ? Icons.check_circle_outline
                      : Icons.cancel_outlined,
                  size: 20,
                  color: step.state == DeployStepState.ok
                      ? successColor(context)
                      : Theme.of(context).colorScheme.error,
                ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  step.message,
                  style: TextStyle(
                    color: switch (step.state) {
                      DeployStepState.ok => successColor(context),
                      DeployStepState.failed => Theme.of(
                        context,
                      ).colorScheme.error,
                      DeployStepState.running => null,
                    },
                  ),
                ),
              ),
            ],
          ),
        ),
    ],
  );
}

class OperationLogButton extends StatelessWidget {
  const OperationLogButton({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) => TextButton.icon(
    icon: const Icon(Icons.receipt_long_outlined),
    label: Text(AppStrings.operationLogCount(controller.operationLogCount)),
    onPressed: () => showDialog<void>(
      context: context,
      builder: (context) => ListenableBuilder(
        listenable: controller,
        builder: (context, _) => AlertDialog(
          title: const Text(AppStrings.operationLogs),
          content: SizedBox(
            width: 600,
            child: LogLines(
              lines: controller.operationLogs,
              storageKey: 'operationLogs',
              height: (MediaQuery.sizeOf(context).height * .5).clamp(160, 400),
              colorCoded: true,
            ),
          ),
          actions: [
            TextButton(
              onPressed: controller.operationLogs.isEmpty
                  ? null
                  : controller.clearOperationLogs,
              child: const Text(AppStrings.clearOperationLogs),
            ),
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text(AppStrings.close),
            ),
          ],
        ),
      ),
    ),
  );
}
