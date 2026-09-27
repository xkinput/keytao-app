import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/widgets.dart';

class SchemeCard extends StatelessWidget {
  const SchemeCard({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    final local = c.local;
    return SectionCard(
      title: '输入方案',
      children: [
        SchemeSelection(controller: c),
        ValueRow('KeyTao 目录', c.info.userRoot),
        ValueRow(
          '本地状态',
          local == null
              ? '未读取'
              : '${local.installed ? '已安装' : '未安装'} · ${local.deployed ? '已部署' : '未部署'}',
        ),
        ValueRow('本地版本', local?.version ?? '—'),
        if (local?.schemas.isNotEmpty == true)
          ValueRow('本地方案', local!.schemas.join('、')),
        if (c.isAndroid && !c.storageReady) ...[
          const SizedBox(height: 8),
          Text(c.storage?['message'] as String? ?? '请先授权文件访问'),
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              onPressed: c.busy || c.storage?['canOpenSettings'] != true
                  ? null
                  : () => c.run(c.android.openStoragePermissionSettings),
              child: const Text('授权文件访问'),
            ),
          ),
        ],
        const SizedBox(height: 16),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            FilledButton.icon(
              onPressed: c.busy ? null : c.install,
              icon: const Icon(Icons.download_rounded),
              label: const Text('安装方案'),
            ),
            FilledButton.tonalIcon(
              onPressed: c.busy || local?.installed != true ? null : c.deploy,
              icon: const Icon(Icons.play_arrow_rounded),
              label: const Text('部署'),
            ),
            OutlinedButton(
              onPressed: c.busy ? null : c.refreshState,
              child: const Text('检查本地'),
            ),
            if (!c.isAndroid)
              OutlinedButton(
                onPressed: c.busy ? null : () => c.open(c.info.userRoot),
                child: const Text('打开目录'),
              ),
          ],
        ),
        OperationProgress(controller: c),
        const SizedBox(height: 24),
        const TextField(
          minLines: 2,
          maxLines: 5,
          decoration: InputDecoration(labelText: '输入测试'),
        ),
      ],
    );
  }
}

class SchemeSelection extends StatelessWidget {
  const SchemeSelection({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ChoiceSetting(
          label: '方案',
          value: c.scheme,
          options: schemes,
          onChanged: c.busy ? null : c.selectScheme,
        ),
        if (c.scheme == 'keytao')
          ChoiceSetting(
            label: '下载来源',
            value: c.source,
            options: const {'github': 'GitHub', 'gitee': 'Gitee'},
            onChanged: c.busy ? null : c.selectSource,
          ),
        Row(
          children: [
            Expanded(child: Text('最新版本  ${c.releaseVersion ?? '—'}')),
            IconButton(
              tooltip: '刷新版本',
              onPressed: c.busy ? null : c.refreshRelease,
              icon: const Icon(Icons.refresh_rounded),
            ),
          ],
        ),
        const SizedBox(height: 16),
      ],
    );
  }
}

class OperationProgress extends StatelessWidget {
  const OperationProgress({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (c.busy) ...[
          const SizedBox(height: 16),
          LinearProgressIndicator(
            value: c.installFraction,
            borderRadius: BorderRadius.circular(8),
          ),
        ],
        if (c.progress.isNotEmpty) ...[
          const SizedBox(height: 16),
          Semantics(liveRegion: true, child: Text(c.progress)),
        ],
        if (c.operationLogs.isNotEmpty || c.verification.isNotEmpty)
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton.icon(
              icon: const Icon(Icons.receipt_long_rounded),
              label: const Text('操作日志'),
              onPressed: () => showDialog<void>(
                context: context,
                builder: (context) => ListenableBuilder(
                  listenable: c,
                  builder: (context, _) => AlertDialog(
                    title: const Text('操作日志'),
                    content: SizedBox(
                      width: 600,
                      height: 400,
                      child: ListView.builder(
                        itemCount:
                            c.operationLogs.length + c.verification.length,
                        itemBuilder: (context, index) {
                          if (index < c.operationLogs.length) {
                            return Padding(
                              padding: const EdgeInsets.only(bottom: 8),
                              child: SelectableText(c.operationLogs[index]),
                            );
                          }
                          final entry =
                              c.verification[index - c.operationLogs.length];
                          return ListTile(
                            contentPadding: EdgeInsets.zero,
                            leading: Icon(
                              entry.ok
                                  ? Icons.check_circle_outline
                                  : Icons.error_outline,
                              color: entry.ok
                                  ? null
                                  : Theme.of(context).colorScheme.error,
                            ),
                            title: SelectableText(entry.path),
                            subtitle: Text(entry.note),
                          );
                        },
                      ),
                    ),
                    actions: [
                      TextButton(
                        onPressed: () => Navigator.pop(context),
                        child: const Text('关闭'),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}
