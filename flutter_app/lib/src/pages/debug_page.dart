import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/widgets.dart';
import '../rust/api/types.dart';

class DebugPage extends StatelessWidget {
  const DebugPage({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    final settings = c.logSettings;
    final lines = c.runtimeLog?.lines ?? const <String>[];
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: ContentWidth(
        child: SectionCard(
          title: '运行日志',
          children: [
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('记录运行日志'),
              value: settings?.enabled ?? false,
              onChanged: c.busy || settings == null
                  ? null
                  : (value) => c.saveLogSettings(value, settings.level),
            ),
            const SizedBox(height: 16),
            ChoiceSetting<RuntimeLogLevelDto>(
              label: '日志级别',
              value: settings?.level ?? RuntimeLogLevelDto.info,
              options: const {
                RuntimeLogLevelDto.info: '常规',
                RuntimeLogLevelDto.verbose: '详细',
              },
              onChanged: c.busy || settings == null
                  ? null
                  : (value) => c.saveLogSettings(settings.enabled, value),
            ),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                FilledButton.tonalIcon(
                  onPressed: c.busy ? null : c.refreshLogs,
                  icon: const Icon(Icons.refresh_rounded),
                  label: const Text('刷新'),
                ),
                OutlinedButton(
                  onPressed: c.busy ? null : c.clearLogs,
                  child: const Text('清空'),
                ),
                if (c.isAndroid)
                  OutlinedButton(
                    onPressed: c.busy ? null : c.shareLogs,
                    child: const Text('分享日志'),
                  ),
              ],
            ),
            if (c.sharedLogPath != null) ...[
              const SizedBox(height: 16),
              SelectableText('已保存：${c.sharedLogPath}'),
            ],
            const SizedBox(height: 24),
            if (c.busy) const LinearProgressIndicator(),
            if (c.runtimeLog?.truncated == true) ...[
              Text('最近 ${lines.length} 行'),
              const SizedBox(height: 8),
            ],
            SizedBox(
              height: 360,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: Theme.of(context).colorScheme.surface,
                  borderRadius: BorderRadius.circular(16),
                ),
                child: lines.isEmpty
                    ? const Center(child: Text('暂无日志'))
                    : ListView.builder(
                        key: const PageStorageKey('runtimeLogs'),
                        padding: const EdgeInsets.all(16),
                        itemCount: lines.length,
                        itemBuilder: (context, index) => Padding(
                          padding: const EdgeInsets.only(bottom: 8),
                          child: SelectableText(
                            lines[index],
                            style: Theme.of(context).textTheme.bodySmall
                                ?.copyWith(fontFamily: 'monospace'),
                          ),
                        ),
                      ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
