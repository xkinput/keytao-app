import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart' show AppStrings;
import '../rust/api/types.dart';
import 'widgets.dart';

const selectConversionSchemaLabel = '选择转换方案';

Future<void> showPackageSchemaSelection(
  BuildContext context,
  AppController controller,
  String packageKey, {
  bool activateOnConfirm = false,
  bool convert = false,
}) async {
  final schemaId = await showFDialog<String>(
    context: context,
    barrierDismissible: false,
    builder: (_, _, animation) => _SchemaSelectionDialog(
      controller: controller,
      packageKey: packageKey,
      animation: animation,
      activateOnConfirm: activateOnConfirm,
      convert: convert,
    ),
  );
  if (convert && schemaId != null) {
    await controller.convertSchemePackage(packageKey, schemaId);
  }
}

class _SchemaSelectionDialog extends StatefulWidget {
  const _SchemaSelectionDialog({
    required this.controller,
    required this.packageKey,
    required this.animation,
    this.activateOnConfirm = false,
    required this.convert,
  });

  final AppController controller;
  final String packageKey;
  final Animation<double> animation;
  final bool activateOnConfirm;
  final bool convert;

  @override
  State<_SchemaSelectionDialog> createState() => _SchemaSelectionDialogState();
}

class _SchemaSelectionDialogState extends State<_SchemaSelectionDialog> {
  List<PackageSchemaDto> _schemas = [];
  final Set<String> _selected = {};
  String? _conversionSchema;
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
        if (widget.convert) {
          final controller = widget.controller;
          final key = widget.packageKey;
          final activeConversion = controller.usesLibjd(key)
              ? controller.isolatedPackages
                    .where((package) => package.packageKey == key)
                    .firstOrNull
                    ?.schemas
                    .firstOrNull
                    ?.id
              : null;
          _conversionSchema =
              schemas
                  .where((schema) => schema.id == activeConversion)
                  .firstOrNull
                  ?.id ??
              schemas.where((schema) => schema.selected).firstOrNull?.id ??
              schemas.firstOrNull?.id;
          return;
        }
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
    if (widget.convert) {
      Navigator.pop(context, _conversionSchema);
      return;
    }
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
      activate: widget.activateOnConfirm,
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
      title: widget.convert
          ? selectConversionSchemaLabel
          : AppStrings.selectSchemas,
      animation: widget.animation,
      actions: [
        ActionButton(
          AppStrings.cancel,
          onPress: _saving ? null : () => Navigator.pop(context),
        ),
        ActionButton(
          widget.convert
              ? '转换词库'
              : (_saving ? AppStrings.deploying : AppStrings.deploy),
          primary: true,
          onPress:
              _loading ||
                  _saving ||
                  (widget.convert
                      ? _conversionSchema == null
                      : _selected.isEmpty)
              ? null
              : _save,
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
            if (widget.convert && _conversionSchema != null)
              BigTextRadioGroup<String>(
                label: selectConversionSchemaLabel,
                value: _conversionSchema!,
                options: {
                  for (final schema in _schemas)
                    schema.id:
                        _schemas.any(
                          (other) =>
                              other.id != schema.id &&
                              other.name == schema.name,
                        )
                        ? '${schema.name} (${schema.id})'
                        : schema.name,
                },
                onChanged: (id) => setState(() => _conversionSchema = id),
              ),
            if (widget.convert &&
                !_loading &&
                _error == null &&
                _schemas.isEmpty)
              const Text('此方案包没有可转换的子方案'),
            if (!widget.convert)
              for (final schema in _schemas)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  child: FCheckbox(
                    value: _selected.contains(schema.id),
                    enabled:
                        !_saving &&
                        !(_selected.length == 1 &&
                            _selected.contains(schema.id)),
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
