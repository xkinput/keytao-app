import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart';
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
    if (mounted && success) Navigator.pop(context);
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
      if (c.accountUser != null) {
        return AppDialog(
          title: '账号',
          animation: widget.animation,
          actions: [
            ActionButton(
              '退出登录',
              onPress: c.busy || c.loggingIn ? null : _logout,
            ),
          ],
          child: SizedBox(width: 360, child: Text(c.accountName ?? '账号')),
        );
      }
      return PopScope(
        canPop: !c.loggingIn,
        child: AppDialog(
          title: AppStrings.login,
          animation: widget.animation,
          actions: [
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
