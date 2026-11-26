import 'dart:async' show unawaited;

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import 'widgets.dart';

Future<void>? _activeAccountDialog;

Future<void> showAccountDialog(
  BuildContext context,
  AppController controller,
) async {
  await controller.reloadAccountSession(checkSchemeProfile: false);
  if (!context.mounted) return;
  if (_activeAccountDialog case final active?) return active;
  controller.accountError = null;
  FocusManager.instance.primaryFocus?.unfocus();
  final result = showFDialog<void>(
    context: context,
    builder: (_, _, animation) =>
        _LoginDialog(controller: controller, animation: animation),
  );
  _activeAccountDialog = result;
  try {
    return await result;
  } finally {
    _activeAccountDialog = null;
  }
}

class _LoginDialog extends StatefulWidget {
  const _LoginDialog({required this.controller, required this.animation});
  final AppController controller;
  final Animation<double> animation;

  @override
  State<_LoginDialog> createState() => _LoginDialogState();
}

class _LoginDialogState extends State<_LoginDialog> {
  final _name = TextEditingController();
  final _password = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    _password.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (widget.controller.loggingIn) return;
    final success = await widget.controller.login(_name.text, _password.text);
    if (mounted && success) {
      _password.clear();
      FocusManager.instance.primaryFocus?.unfocus();
    }
  }

  void _register() {
    if (widget.controller.loggingIn) return;
    Navigator.pop(context);
    unawaited(widget.controller.openWebsitePath('/register'));
  }

  Future<void> _logout() async {
    await widget.controller.logout();
    if (mounted && widget.controller.accountUser == null) {
      Navigator.pop(context);
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      if (c.accountUser case final user?) {
        return AppDialog(
          title: '我的',
          animation: widget.animation,
          actions: [
            ActionButton(
              '退出登录',
              onPress: c.busy || c.loggingIn ? null : _logout,
            ),
            ActionButton(
              AppStrings.close,
              onPress: () => Navigator.pop(context),
            ),
          ],
          child: SizedBox(
            width: 360,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 14,
              children: [
                ValueRow(
                  user.nickname == null ? '账号' : '昵称',
                  c.accountName ?? '未设置',
                ),
                _AccountStats(key: ValueKey(user.id), controller: c),
              ],
            ),
          ),
        );
      }
      return PopScope(
        canPop: !c.loggingIn,
        child: AppDialog(
          title: AppStrings.login,
          animation: widget.animation,
          actions: [
            ActionButton(
              '注册',
              key: const ValueKey('login-register-button'),
              onPress: c.loggingIn ? null : _register,
            ),
            ActionButton(
              AppStrings.cancel,
              onPress: c.loggingIn ? null : () => Navigator.pop(context),
            ),
            ActionButton(
              c.loggingIn ? AppStrings.loggingIn : AppStrings.login,
              primary: true,
              onPress: c.loggingIn ? null : _submit,
            ),
          ],
          child: SizedBox(
            width: 360,
            child: AutofillGroup(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                spacing: 14,
                children: [
                  FTextField(
                    label: const Text(AppStrings.username),
                    control: FTextFieldControl.managed(controller: _name),
                    enabled: !c.loggingIn,
                    autofocus: true,
                    autofillHints: const [AutofillHints.username],
                    textInputAction: TextInputAction.done,
                    onSubmit: (_) => _submit(),
                  ),
                  FTextField.password(
                    label: const Text(AppStrings.password),
                    control: FTextFieldControl.managed(controller: _password),
                    enabled: !c.loggingIn,
                    autofillHints: const [AutofillHints.password],
                    textInputAction: TextInputAction.done,
                    onSubmit: (_) => _submit(),
                  ),
                  if (c.loggingIn) const FProgress(),
                  if (c.accountError case final error?) ErrorMessage(error),
                ],
              ),
            ),
          ),
        ),
      );
    },
  );
}

class _AccountStats extends StatefulWidget {
  const _AccountStats({super.key, required this.controller});

  final AppController controller;

  @override
  State<_AccountStats> createState() => _AccountStatsState();
}

class _AccountStatsState extends State<_AccountStats> {
  late Future<AccountStatsDto> _stats = _load();

  Future<AccountStatsDto> _load() async {
    try {
      return await core.getAccountStats();
    } catch (_) {
      if (mounted) {
        await widget.controller.reloadAccountSession(checkSchemeProfile: false);
      }
      rethrow;
    }
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<AccountStatsDto>(
    future: _stats,
    builder: (context, snapshot) {
      final stats = snapshot.data;
      return Section(
        title: '数据统计',
        children: [
          if (snapshot.connectionState != ConnectionState.done)
            Semantics(label: '正在加载数据统计', child: const FProgress())
          else if (stats == null)
            Row(
              children: [
                const Expanded(child: ErrorMessage('统计加载失败')),
                ActionButton(
                  '重试',
                  onPress: () => setState(() => _stats = _load()),
                ),
              ],
            )
          else ...[
            ValueRow('提交批次', '${stats.batchesCount}'),
            ValueRow('修改提议', '${stats.pullRequestsCount}'),
            ValueRow('待审核批次', '${stats.submittedBatches}'),
            ValueRow('已发布批次', '${stats.publishedBatches}'),
          ],
        ],
      );
    },
  );
}
