import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/strings.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class PackageOrphanDirectory {
  PackageOrphanDirectory(this.path);

  final String path;
  final directories = <String, PackageOrphanDirectory>{};
  final files = <PackageOrphanFileDto>[];

  Iterable<PackageOrphanFileDto> get descendantFiles sync* {
    yield* files;
    for (final directory in directories.values) {
      yield* directory.descendantFiles;
    }
  }

  BigInt get totalSize =>
      descendantFiles.fold(BigInt.zero, (total, file) => total + file.size);

  bool? selectionState(Set<String> selected) {
    var count = 0;
    var selectedCount = 0;
    for (final file in descendantFiles) {
      count++;
      if (selected.contains(file.path)) selectedCount++;
    }
    if (selectedCount == 0) return false;
    return selectedCount == count ? true : null;
  }
}

PackageOrphanDirectory buildPackageOrphanTree(
  Iterable<PackageOrphanFileDto> files,
) {
  final root = PackageOrphanDirectory('');
  for (final file in files) {
    final segments = file.path.split('/');
    var directory = root;
    for (final segment in segments.take(segments.length - 1)) {
      final path = directory.path.isEmpty
          ? segment
          : '${directory.path}/$segment';
      directory = directory.directories.putIfAbsent(
        segment,
        () => PackageOrphanDirectory(path),
      );
    }
    directory.files.add(file);
  }
  return root;
}

void showPackageOrphans(BuildContext context, AppController controller) {
  showFDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_, _, animation) =>
        _PackageOrphanDialog(controller: controller, animation: animation),
  );
}

class _PackageOrphanDialog extends StatefulWidget {
  const _PackageOrphanDialog({
    required this.controller,
    required this.animation,
  });

  final AppController controller;
  final Animation<double> animation;

  @override
  State<_PackageOrphanDialog> createState() => _PackageOrphanDialogState();
}

class _PackageOrphanDialogState extends State<_PackageOrphanDialog> {
  PackageOrphanReportDto? _report;
  final Set<String> _selected = {};
  final Set<String> _selectedBackups = {};
  final Set<String> _expanded = {};
  bool _loading = true;
  bool _moving = false;
  String? _error;
  String? _notice;

  int get _selectedCount => _selected.length + _selectedBackups.length;

  int _availableCount(PackageOrphanReportDto report) =>
      report.files.length + report.keptBackups.length;

  bool _allSelected(PackageOrphanReportDto report) {
    final total = _availableCount(report);
    return total > 0 && _selectedCount == total;
  }

  void _toggleSelectAll(PackageOrphanReportDto report) {
    if (_allSelected(report)) {
      _selected.clear();
      _selectedBackups.clear();
    } else {
      _selected
        ..clear()
        ..addAll(report.files.map((file) => file.path));
      _selectedBackups
        ..clear()
        ..addAll(report.keptBackups.map((backup) => backup.backup));
    }
  }

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final report = await widget.controller.packageOrphanReport();
      if (!mounted) return;
      setState(() {
        _report = report;
        _selected.clear();
        _selectedBackups.clear();
        _expanded.clear();
      });
    } catch (failure) {
      if (mounted) setState(() => _error = failure.toString());
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _move() async {
    final files = _report!.files
        .where((file) => _selected.contains(file.path))
        .toList();
    final backups = _report!.keptBackups
        .where((backup) => _selectedBackups.contains(backup.backup))
        .toList();
    final total = files.length + backups.length;
    setState(() {
      _moving = true;
      _error = null;
      _notice = null;
    });
    final ok = await widget.controller.trashPackageOrphans(files, backups);
    if (!mounted) return;
    setState(() {
      _notice = ok ? '已移到废纸篓（$total 项）' : null;
      _error = ok ? null : widget.controller.error ?? '移动失败，请刷新后重试。';
    });
    await _load();
    if (mounted) setState(() => _moving = false);
  }

  Iterable<Widget> _treeRows(
    BuildContext context,
    PackageOrphanDirectory parent, [
    int depth = 0,
  ]) sync* {
    for (final entry in parent.directories.entries) {
      final directory = entry.value;
      final legacyBackups = directory.path == 'legacy-backup';
      final selected = directory.selectionState(_selected);
      final expanded = _expanded.contains(directory.path);
      yield Padding(
        padding: EdgeInsets.only(left: depth * 24, top: 8, bottom: 8),
        child: Row(
          children: [
            Material(
              type: MaterialType.transparency,
              child: Checkbox(
                value: selected,
                tristate: true,
                materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                visualDensity: VisualDensity.compact,
                activeColor: context.theme.colors.primary,
                checkColor: context.theme.colors.primaryForeground,
                side: BorderSide(color: context.theme.colors.border),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(4),
                ),
                semanticLabel: '选择 ${directory.path}/',
                onChanged: _moving
                    ? null
                    : (_) => setState(() {
                        final paths = directory.descendantFiles.map(
                          (file) => file.path,
                        );
                        if (selected == true) {
                          _selected.removeAll(paths);
                        } else {
                          _selected.addAll(paths);
                        }
                      }),
              ),
            ),
            Semantics(
              expanded: expanded,
              child: FButton.icon(
                variant: FButtonVariant.ghost,
                size: FButtonSizeVariant.xs,
                semanticsLabel: '${expanded ? '收起' : '展开'} ${directory.path}/',
                onPress: () => setState(() {
                  if (expanded) {
                    _expanded.remove(directory.path);
                  } else {
                    _expanded.add(directory.path);
                  }
                }),
                child: Icon(
                  expanded
                      ? FLucideIcons.chevronDown
                      : FLucideIcons.chevronRight,
                  size: 16,
                ),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(child: Text(legacyBackups ? '旧布局备份' : entry.key)),
            const SizedBox(width: 12),
            Text(
              legacyBackups
                  ? '${directory.descendantFiles.length} 份'
                  : '${directory.descendantFiles.length} 个文件 · '
                        '${formatLogBytes(directory.totalSize)}',
            ),
          ],
        ),
      );
      if (expanded) yield* _treeRows(context, directory, depth + 1);
    }
    for (final file in parent.files) {
      yield Padding(
        padding: EdgeInsets.only(left: depth * 24, top: 8, bottom: 8),
        child: Row(
          children: [
            FCheckbox(
              value: _selected.contains(file.path),
              enabled: !_moving,
              semanticsLabel: '选择 ${file.path}',
              onChange: (value) => setState(() {
                if (value) {
                  _selected.add(file.path);
                } else {
                  _selected.remove(file.path);
                }
              }),
            ),
            const SizedBox(width: 12),
            Expanded(child: Text(file.path.split('/').last)),
            if (parent.path != 'legacy-backup') Text(formatLogBytes(file.size)),
          ],
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final report = _report;
    final hasItems = report != null && _availableCount(report) > 0;
    return PopScope(
      canPop: !_moving,
      child: AppDialog(
        title: '清理残留文件',
        animation: widget.animation,
        actions: [
          if (hasItems)
            ActionButton(
              _allSelected(report) ? '取消全选' : '全选',
              onPress: _loading || _moving
                  ? null
                  : () => setState(() => _toggleSelectAll(report)),
            ),
          ActionButton(
            AppStrings.close,
            onPress: _moving ? null : () => Navigator.pop(context),
          ),
          ActionButton(
            '移到废纸篓 ($_selectedCount)',
            icon: FLucideIcons.trash2,
            primary: true,
            onPress: _loading || _moving || _selectedCount == 0 ? null : _move,
          ),
        ],
        child: SizedBox(
          width: 560,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (_loading || _moving) const FProgress(),
              if (_error case final error?) ErrorMessage(error),
              if (_notice case final notice?) Text(notice),
              if (report != null) ...[
                if (!hasItems) const Text('没有发现残留文件。'),
                ..._treeRows(context, buildPackageOrphanTree(report.files)),
                if (report.keptBackups.isNotEmpty) ...[
                  if (report.files.isNotEmpty) const SizedBox(height: 16),
                  const Text('已保留的文件备份'),
                  for (final backup in report.keptBackups)
                    Padding(
                      padding: const EdgeInsets.only(top: 8),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Padding(
                            padding: const EdgeInsets.only(top: 2),
                            child: FCheckbox(
                              value: _selectedBackups.contains(backup.backup),
                              enabled: !_moving,
                              semanticsLabel: '选择备份 ${backup.path}',
                              onChange: (value) => setState(() {
                                if (value) {
                                  _selectedBackups.add(backup.backup);
                                } else {
                                  _selectedBackups.remove(backup.backup);
                                }
                              }),
                            ),
                          ),
                          const SizedBox(width: 12),
                          Expanded(
                            child: SelectableText(
                              '${backup.path}\n备份：${backup.backup}',
                            ),
                          ),
                        ],
                      ),
                    ),
                ],
              ],
            ],
          ),
        ),
      ),
    );
  }
}
