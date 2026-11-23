import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'progress.dart';
import 'widgets.dart';

void showGitSchemeDialog(
  BuildContext context,
  AppController controller, {
  InstalledPackageDto? existing,
}) {
  showFDialog<void>(
    context: context,
    builder: (_, _, animation) => ListenableBuilder(
      listenable: controller,
      builder: (_, _) => _GitSchemeDialog(
        controller: controller,
        animation: animation,
        existing: existing,
      ),
    ),
  );
}

class _GitSchemeDialog extends StatefulWidget {
  const _GitSchemeDialog({
    required this.controller,
    required this.animation,
    this.existing,
  });

  final AppController controller;
  final Animation<double> animation;
  final InstalledPackageDto? existing;

  @override
  State<_GitSchemeDialog> createState() => _GitSchemeDialogState();
}

class _GitSchemeDialogState extends State<_GitSchemeDialog> {
  late final TextEditingController _urlController;
  late final TextEditingController _branchController;
  late final TextEditingController _assetController;
  late final TextEditingController _nameController;
  late String _mode;
  bool _submitting = false;
  String? _localError;

  @override
  void initState() {
    super.initState();
    final git = widget.existing?.git;
    _urlController = TextEditingController(text: git?.repoUrl ?? '');
    _branchController = TextEditingController(text: git?.branch ?? '');
    _assetController = TextEditingController(text: git?.assetName ?? '');
    _nameController = TextEditingController(
      text: widget.existing?.displayName ?? '',
    );
    _mode = git?.mode == 'branch' ? 'branch' : 'release';
    _urlController.addListener(_onUrlChanged);
  }

  @override
  void dispose() {
    _urlController
      ..removeListener(_onUrlChanged)
      ..dispose();
    _branchController.dispose();
    _assetController.dispose();
    _nameController.dispose();
    super.dispose();
  }

  void _onUrlChanged() {
    final text = _urlController.text.trim();
    final uri = Uri.tryParse(text);
    if (uri != null &&
        (uri.host == 'github.com' || uri.host == 'gitee.com') &&
        uri.pathSegments.length >= 4) {
      final rest = uri.pathSegments.sublist(2);
      if (rest.first == 'tree' && _branchController.text.trim().isEmpty) {
        setState(() {
          _mode = 'branch';
          _branchController.text = rest.sublist(1).join('/');
        });
        return;
      }
      if (rest.length == 4 &&
          rest[0] == 'releases' &&
          rest[1] == 'latest' &&
          rest[2] == 'download' &&
          _assetController.text.trim().isEmpty) {
        setState(() {
          _mode = 'release';
          _assetController.text = rest[3];
        });
        return;
      }
    }
    setState(() {});
  }

  Future<void> _submit() async {
    final repoUrl = _urlController.text.trim();
    if (repoUrl.isEmpty) {
      setState(() => _localError = '请输入 Git 仓库链接');
      return;
    }
    setState(() {
      _submitting = true;
      _localError = null;
    });
    final ok = await widget.controller.importGitScheme(
      repoUrl: repoUrl,
      mode: _mode,
      branch: _mode == 'branch' ? _branchController.text.trim() : null,
      assetName: _mode == 'release' ? _assetController.text.trim() : null,
      displayName: _nameController.text.trim(),
      schemeKey: widget.existing?.schemeKey,
    );
    if (!mounted) return;
    if (ok) {
      Navigator.pop(context);
    } else {
      setState(() {
        _submitting = false;
        _localError = widget.controller.packageError;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final busy = c.busy || _submitting;
    final canSubmit = !busy && _urlController.text.trim().isNotEmpty;
    return AppDialog(
      title: widget.existing == null ? '从 Git 添加方案' : '配置 Git 方案',
      animation: widget.animation,
      actions: [
        if (_submitting && c.canCancelDownload)
          ActionButton(AppStrings.cancel, onPress: c.cancelDownload)
        else
          ActionButton(
            AppStrings.cancel,
            onPress: busy ? null : () => Navigator.pop(context),
          ),
        ActionButton(
          widget.existing == null ? '下载并添加' : '保存并同步',
          key: const ValueKey('git-scheme-submit'),
          primary: true,
          icon: FLucideIcons.download,
          onPress: canSubmit ? _submit : null,
        ),
      ],
      child: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 12,
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 6,
              children: [
                const Text('Git 链接'),
                FTextField(
                  key: const ValueKey('git-scheme-url'),
                  control: FTextFieldControl.managed(
                    controller: _urlController,
                  ),
                  hint: 'https://github.com/owner/repo 或 Gitee 链接',
                  enabled: !busy,
                  onSubmit: (_) {
                    if (canSubmit) _submit();
                  },
                ),
              ],
            ),
            ChoiceSetting<String>(
              label: '下载方式',
              value: _mode,
              options: const {
                'release': 'Release 发布包',
                'branch': '仓库分支 Archive',
              },
              onChanged: busy
                  ? null
                  : (value) => setState(() => _mode = value),
            ),
            if (_mode == 'release')
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                spacing: 6,
                children: [
                  const Text('固定文件名（可选）'),
                  FTextField(
                    key: const ValueKey('git-scheme-asset'),
                    control: FTextFieldControl.managed(
                      controller: _assetController,
                    ),
                    hint: '留空自动选择发布包，或填如 full.zip',
                    enabled: !busy,
                    onSubmit: (_) {
                      if (canSubmit) _submit();
                    },
                  ),
                ],
              )
            else
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                spacing: 6,
                children: [
                  const Text('分支名称（可选）'),
                  FTextField(
                    key: const ValueKey('git-scheme-branch'),
                    control: FTextFieldControl.managed(
                      controller: _branchController,
                    ),
                    hint: '留空使用默认分支（如 main / master）',
                    enabled: !busy,
                    onSubmit: (_) {
                      if (canSubmit) _submit();
                    },
                  ),
                ],
              ),
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 6,
              children: [
                const Text('方案名称（可选）'),
                FTextField(
                  key: const ValueKey('git-scheme-name'),
                  control: FTextFieldControl.managed(
                    controller: _nameController,
                  ),
                  hint: '留空自动识别方案名称',
                  enabled: !busy,
                  onSubmit: (_) {
                    if (canSubmit) _submit();
                  },
                ),
              ],
            ),
            if (_submitting && (c.isInstalling || c.isDeploying))
              InstallProgressView(
                active: true,
                progress: c.installProgress,
                fraction: c.installFraction,
                message: c.progress,
              ),
            if (_localError case final error? when error.isNotEmpty)
              ErrorMessage(error),
          ],
        ),
      ),
    );
  }
}
