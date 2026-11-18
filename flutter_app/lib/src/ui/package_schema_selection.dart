import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

void showPackageSchemaSelection(
  BuildContext context,
  AppController controller,
  String packageKey,
) {
  showFDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_, _, animation) => _SchemaSelectionDialog(
      controller: controller,
      packageKey: packageKey,
      animation: animation,
    ),
  );
}

class _SchemaSelectionDialog extends StatefulWidget {
  const _SchemaSelectionDialog({
    required this.controller,
    required this.packageKey,
    required this.animation,
  });

  final AppController controller;
  final String packageKey;
  final Animation<double> animation;

  @override
  State<_SchemaSelectionDialog> createState() => _SchemaSelectionDialogState();
}

class _SchemaSelectionDialogState extends State<_SchemaSelectionDialog> {
  List<PackageSchemaDto> _schemas = [];
  final Set<String> _selected = {};
  bool _loading = true;
  bool _saving = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final schemas = await widget.controller.packageSchemas(widget.packageKey);
      if (!mounted) return;
      setState(() {
        _schemas = schemas;
        _selected.addAll(
          schemas.where((schema) => schema.selected).map((schema) => schema.id),
        );
      });
    } catch (failure) {
      if (mounted) setState(() => _error = failure.toString());
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _error = null;
    });
    final ok = await widget.controller.setPackageSchemas(
      widget.packageKey,
      _schemas
          .where((schema) => _selected.contains(schema.id))
          .map((schema) => schema.id)
          .toList(),
    );
    if (!mounted) return;
    if (ok) {
      Navigator.pop(context);
    } else {
      setState(() {
        _saving = false;
        _error = widget.controller.packageError ?? widget.controller.error;
      });
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !_saving,
    child: AppDialog(
      title: AppStrings.selectSchemas,
      animation: widget.animation,
      actions: [
        ActionButton(
          AppStrings.cancel,
          onPress: _saving ? null : () => Navigator.pop(context),
        ),
        ActionButton(
          _saving ? AppStrings.deploying : AppStrings.deploy,
          primary: true,
          onPress: _loading || _saving || _selected.isEmpty ? null : _save,
        ),
      ],
      child: SizedBox(
        width: 480,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (_loading || _saving) const FProgress(),
            if (_error case final error?) ErrorMessage(error),
            for (final schema in _schemas)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: FCheckbox(
                  value: _selected.contains(schema.id),
                  enabled:
                      !_saving &&
                      !(_selected.length == 1 && _selected.contains(schema.id)),
                  label: Text(schema.name),
                  description: Text(schema.id),
                  onChange: (value) => setState(() {
                    if (value) {
                      _selected.add(schema.id);
                    } else if (_selected.length > 1) {
                      _selected.remove(schema.id);
                    }
                  }),
                ),
              ),
          ],
        ),
      ),
    ),
  );
}
