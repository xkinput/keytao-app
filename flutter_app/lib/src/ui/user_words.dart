import 'dart:async';

import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import 'widgets.dart';

bool _hasConflict(UserWordComparisonDto comparison) =>
    comparison.status == 'code_taken' || comparison.status == 'word_has_code';

Future<void> openUserWords(
  BuildContext context,
  AppController controller, {
  String? schemeKey,
  bool conflictsOnly = false,
}) => Navigator.of(context).push<void>(
  MaterialPageRoute(
    builder: (_) => UserWordsPage(
      controller: controller,
      schemeKey: schemeKey ?? controller.activeSchemeKey,
      conflictsOnly: conflictsOnly,
    ),
  ),
);

class UserWordsNotice extends StatelessWidget {
  const UserWordsNotice({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final report = c.userWordsReport;
    final error = c.userWordsReportError;
    if (report == null && error == null) return const SizedBox.shrink();
    return Section(
      title: c.userWordsReportScheme == null
          ? '自造词'
          : '${schemeDisplayName(c.userWordsReportScheme!)} · 自造词',
      icon: FLucideIcons.bookOpen,
      trailing: FButton.icon(
        variant: FButtonVariant.ghost,
        onPress: c.dismissUserWordsReport,
        child: const Icon(FLucideIcons.x, size: 16, semanticLabel: '关闭'),
      ),
      children: [
        if (report != null) ...[
          Text(
            '已合并 ${report.removed.length} 条，冲突 ${report.conflicts.length} 条',
          ),
          ActionRow(
            children: [
              ActionButton(
                report.conflicts.isEmpty ? '查看自造词' : '查看冲突',
                onPress: c.busy
                    ? null
                    : () => openUserWords(
                        context,
                        c,
                        schemeKey: c.userWordsReportScheme,
                        conflictsOnly: report.conflicts.isNotEmpty,
                      ),
              ),
            ],
          ),
        ],
        if (error != null) ErrorMessage(error),
      ],
    );
  }
}

class UserWordsPage extends StatefulWidget {
  const UserWordsPage({
    super.key,
    required this.controller,
    required this.schemeKey,
    this.conflictsOnly = false,
  });
  final AppController controller;
  final String schemeKey;
  final bool conflictsOnly;

  @override
  State<UserWordsPage> createState() => _UserWordsPageState();
}

class _UserWordsPageState extends State<UserWordsPage> {
  final _search = TextEditingController();
  late String _scheme = widget.schemeKey;
  late bool _conflictsOnly = widget.conflictsOnly;
  List<UserWordComparisonDto> _entries = [];
  bool _loading = true;
  bool _saving = false;
  UserWordDto? _pendingDelete;
  String? _error;
  int _request = 0;
  int _loadedRevision = -1;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_controllerChanged);
    unawaited(_load());
  }

  @override
  void dispose() {
    widget.controller.removeListener(_controllerChanged);
    _search.dispose();
    super.dispose();
  }

  void _controllerChanged() {
    if (!widget.controller.busy &&
        !_loading &&
        _pendingDelete == null &&
        _loadedRevision != widget.controller.userWordsRevision) {
      unawaited(_load());
    }
  }

  Future<void> _load() async {
    if (widget.controller.busy) {
      setState(() => _loading = false);
      return;
    }
    final request = ++_request;
    final revision = widget.controller.userWordsRevision;
    setState(() {
      _loading = true;
      _error = null;
      _entries = [];
    });
    try {
      final words = await core.listUserWords(schemeKey: _scheme);
      var entries = [
        for (final word in words)
          UserWordComparisonDto(
            entry: word,
            status: 'none',
            words: const [],
            codes: const [],
          ),
      ];
      String? comparisonError;
      try {
        entries = await core.compareUserWords(schemeKey: _scheme);
      } catch (failure) {
        comparisonError = errorText(failure);
      }
      if (mounted && request == _request) {
        setState(() {
          _entries = entries;
          _error = comparisonError;
        });
      }
    } catch (failure) {
      if (mounted && request == _request) {
        setState(() => _error = errorText(failure));
      }
    } finally {
      if (mounted && request == _request) {
        setState(() {
          _loadedRevision = revision;
          _loading = false;
        });
        _controllerChanged();
      }
    }
  }

  Future<void> _edit([UserWordDto? entry]) async {
    final scheme = _scheme;
    final editor = _UserWordEditor(
      controller: widget.controller,
      schemeKey: scheme,
      entry: entry,
      onChanged: () => widget.controller.markUserWordsChanged(scheme),
    );
    final bool? changed;
    if (MediaQuery.sizeOf(context).width >= 640) {
      changed = await showFDialog<bool>(
        context: context,
        builder: (_, _, animation) => _UserWordEditor(
          controller: widget.controller,
          schemeKey: scheme,
          entry: entry,
          animation: animation,
          onChanged: () => widget.controller.markUserWordsChanged(scheme),
        ),
      );
    } else {
      changed = await showFSheet<bool>(
        context: context,
        side: FLayout.btt,
        mainAxisMaxRatio: 1,
        useSafeArea: true,
        builder: (_) => editor,
      );
    }
    if (!mounted) return;
    if (changed == true) _controllerChanged();
    if (changed == false) {
      await _load();
      if (mounted) setState(() => _error = '词条已变更，已刷新');
    }
  }

  Future<void> _delete(UserWordDto entry) async {
    final c = widget.controller;
    final scheme = _scheme;
    var undone = false;
    setState(() => _pendingDelete = entry);
    c.setUserWordDeletePending(scheme, true);
    String? failureMessage;
    try {
      final snackbar = ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('已删除 ${entry.word}'),
          action: SnackBarAction(label: '撤销', onPressed: () => undone = true),
        ),
      );
      // Commit after undo expires so unknown columns remain intact on undo.
      await snackbar.closed;
      if (!undone) {
        await core.deleteUserWord(
          schemeKey: scheme,
          id: entry.id,
          expectedWord: entry.word,
          expectedCode: entry.code,
        );
        c.markUserWordsChanged(scheme);
      }
    } catch (failure) {
      final message = errorText(failure);
      failureMessage = message.contains('Stale user word')
          ? '词条已变更，已刷新'
          : message;
      c.error = failureMessage;
      c.addOperationLogs(['[ERROR] $failureMessage']);
    } finally {
      c.setUserWordDeletePending(scheme, false);
    }
    if (mounted) {
      setState(() => _pendingDelete = null);
      if (!undone) await _load();
      if (mounted && failureMessage != null) {
        setState(() => _error = failureMessage);
      }
    }
  }

  Future<void> _deploy() async {
    setState(() => _saving = true);
    final result = await widget.controller.deploy();
    if (!mounted) return;
    setState(() {
      _saving = false;
      if (!result) _error = widget.controller.error ?? AppStrings.deployFailed;
    });
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      final blocked = c.busy || _loading || _saving || _pendingDelete != null;
      final query = _search.text.toLowerCase();
      final entries = _entries.where((comparison) {
        final entry = comparison.entry;
        return entry.id != _pendingDelete?.id &&
            (!_conflictsOnly || _hasConflict(comparison)) &&
            (entry.word.toLowerCase().contains(query) ||
                entry.code.toLowerCase().contains(query));
      }).toList();
      final schemeKeys = {
        _scheme,
        for (final package in c.installedPackages) package.schemeKey,
      };
      return Scaffold(
        backgroundColor: context.theme.colors.background,
        body: FScaffold(
          childPad: false,
          header: SafeArea(
            bottom: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(12, 16, 20, 12),
              child: Row(
                children: [
                  FButton.icon(
                    variant: FButtonVariant.ghost,
                    onPress: () => Navigator.pop(context),
                    child: const Icon(
                      FLucideIcons.arrowLeft,
                      semanticLabel: '返回',
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      '自造词',
                      style: context.theme.typography.display.xl2.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  ActionButton(
                    '添加',
                    icon: FLucideIcons.plus,
                    onPress: blocked ? null : _edit,
                  ),
                ],
              ),
            ),
          ),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 720),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      spacing: 12,
                      children: [
                        SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          child: Row(
                            spacing: 6,
                            children: [
                              for (final key in schemeKeys)
                                FButton(
                                  variant: key == _scheme
                                      ? FButtonVariant.outline
                                      : FButtonVariant.ghost,
                                  onPress: blocked
                                      ? null
                                      : () {
                                          setState(() => _scheme = key);
                                          unawaited(_load());
                                        },
                                  child: Text(schemeDisplayName(key)),
                                ),
                            ],
                          ),
                        ),
                        FTextField(
                          hint: '搜索词语或编码',
                          control: FTextFieldControl.managed(
                            controller: _search,
                            onChange: (_) => setState(() {}),
                          ),
                        ),
                        ActionRow(
                          children: [
                            ActionButton(
                              _conflictsOnly ? '查看全部' : '仅看冲突',
                              onPress: () => setState(
                                () => _conflictsOnly = !_conflictsOnly,
                              ),
                            ),
                            if (c.userWordsAffectActiveScheme(_scheme) &&
                                c.userWordsPendingDeploy.contains(
                                  c.activeSchemeKey,
                                ))
                              ActionButton(
                                '部署生效',
                                onPress: blocked ? null : _deploy,
                              ),
                          ],
                        ),
                        if (_loading) const FProgress(),
                        if (_error case final error?) ErrorMessage(error),
                      ],
                    ),
                  ),
                  Expanded(
                    child: !_loading && entries.isEmpty
                        ? Center(
                            child: Text(
                              _search.text.isNotEmpty || _conflictsOnly
                                  ? '没有匹配的词语'
                                  : '暂无自造词',
                              style: TextStyle(
                                color: context.theme.colors.mutedForeground,
                              ),
                            ),
                          )
                        : ListView.separated(
                            padding: const EdgeInsets.fromLTRB(20, 0, 20, 28),
                            itemCount: entries.length,
                            separatorBuilder: (_, _) => Divider(
                              height: 1,
                              color: context.theme.colors.border,
                            ),
                            itemBuilder: (context, index) {
                              final comparison = entries[index];
                              final entry = comparison.entry;
                              return Padding(
                                padding: const EdgeInsets.symmetric(
                                  vertical: 4,
                                ),
                                child: Row(
                                  children: [
                                    Expanded(
                                      child: FTappable(
                                        onPress: blocked
                                            ? null
                                            : () => _edit(entry),
                                        child: Padding(
                                          padding: const EdgeInsets.symmetric(
                                            vertical: 10,
                                          ),
                                          child: Row(
                                            children: [
                                              Expanded(child: Text(entry.word)),
                                              if (_hasConflict(comparison)) ...[
                                                FBadge(
                                                  variant:
                                                      FBadgeVariant.outline,
                                                  child: const Text('冲突'),
                                                ),
                                                const SizedBox(width: 12),
                                              ],
                                              ConstrainedBox(
                                                constraints:
                                                    const BoxConstraints(
                                                      maxWidth: 200,
                                                    ),
                                                child: Text(
                                                  entry.code,
                                                  overflow:
                                                      TextOverflow.ellipsis,
                                                  style: TextStyle(
                                                    color: context
                                                        .theme
                                                        .colors
                                                        .mutedForeground,
                                                  ),
                                                ),
                                              ),
                                            ],
                                          ),
                                        ),
                                      ),
                                    ),
                                    const SizedBox(width: 10),
                                    FButton.icon(
                                      variant: FButtonVariant.ghost,
                                      onPress: blocked
                                          ? null
                                          : () => _delete(entry),
                                      child: Icon(
                                        FLucideIcons.trash2,
                                        size: 16,
                                        semanticLabel: '删除 ${entry.word}',
                                      ),
                                    ),
                                  ],
                                ),
                              );
                            },
                          ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
    },
  );
}

class _UserWordEditor extends StatefulWidget {
  const _UserWordEditor({
    required this.controller,
    required this.schemeKey,
    required this.onChanged,
    this.entry,
    this.animation,
  });
  final AppController controller;
  final String schemeKey;
  final VoidCallback onChanged;
  final UserWordDto? entry;
  final Animation<double>? animation;

  @override
  State<_UserWordEditor> createState() => _UserWordEditorState();
}

class _UserWordEditorState extends State<_UserWordEditor> {
  late final _word = TextEditingController(text: widget.entry?.word);
  late final _code = TextEditingController(text: widget.entry?.code);
  Timer? _debounce;
  int _request = 0;
  bool _submitted = false;
  bool _saving = false;
  String? _error;
  String? _hint;

  String? get _wordError => _word.text.trim().isEmpty
      ? '请输入词语'
      : RegExp(r'[\t\r\n]').hasMatch(_word.text)
      ? '词语不能包含制表符或换行'
      : null;
  String? get _codeError => _code.text.isEmpty
      ? '请输入编码'
      : RegExp(r'\s').hasMatch(_code.text)
      ? '编码不能包含空白字符'
      : null;

  @override
  void initState() {
    super.initState();
    _word.addListener(_lookup);
    _code.addListener(_lookup);
    _lookup();
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _word.dispose();
    _code.dispose();
    super.dispose();
  }

  void _lookup() {
    _debounce?.cancel();
    final request = ++_request;
    setState(() {
      _hint = null;
      _error = null;
    });
    if ((_word.text.isEmpty && _code.text.isEmpty) ||
        RegExp(r'[\t\r\n]').hasMatch(_word.text) ||
        RegExp(r'\s').hasMatch(_code.text)) {
      return;
    }
    _debounce = Timer(const Duration(milliseconds: 180), () async {
      try {
        final comparison = await core.lookupUserWord(
          schemeKey: widget.schemeKey,
          word: _word.text,
          code: _code.text,
        );
        if (!mounted || request != _request) return;
        setState(() {
          _hint = switch (comparison.status) {
            'identical' => '已在方案中',
            'code_taken' => '该编码已有：${comparison.words.join('、')}',
            'word_has_code' => '官方编码：${comparison.codes.join('、')}',
            _ => null,
          };
        });
      } catch (failure) {
        if (mounted && request == _request) {
          setState(() => _error = errorText(failure));
        }
      }
    });
  }

  Future<void> _submit() async {
    if (_saving) return;
    setState(() => _submitted = true);
    if (_wordError != null || _codeError != null) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      final entry = widget.entry;
      if (entry?.word == _word.text && entry?.code == _code.text) {
        if (mounted) Navigator.pop(context);
        return;
      }
      UserWordMutationDto? result;
      final saved = await widget.controller.run(() async {
        result = entry == null
            ? await core.addUserWord(
                schemeKey: widget.schemeKey,
                word: _word.text,
                code: _code.text,
              )
            : await core.updateUserWord(
                schemeKey: widget.schemeKey,
                id: entry.id,
                expectedWord: entry.word,
                expectedCode: entry.code,
                word: _word.text,
                code: _code.text,
                weight: entry.weight,
              );
        if (!result!.alreadyExists) widget.onChanged();
      });
      if (!saved) {
        throw StateError(widget.controller.error ?? '保存失败');
      }
      if (result!.alreadyExists) {
        if (mounted) setState(() => _error = '已存在');
        return;
      }
      if (mounted) Navigator.pop(context, true);
    } catch (failure) {
      if (!mounted) return;
      final message = errorText(failure);
      if (message.contains('Stale user word')) {
        Navigator.pop(context, false);
      } else {
        setState(() => _error = message);
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final title = widget.entry == null ? '添加自造词' : '编辑自造词';
    final fields = SizedBox(
      width: 420,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 14,
        children: [
          FTextField(
            label: const Text('词语'),
            control: FTextFieldControl.managed(controller: _word),
            autofocus: true,
            enabled: !_saving,
            textInputAction: TextInputAction.done,
            onSubmit: (_) => _submit(),
            error: _submitted && _wordError != null ? Text(_wordError!) : null,
          ),
          FTextField(
            label: const Text('编码'),
            control: FTextFieldControl.managed(controller: _code),
            autocorrect: false,
            enableSuggestions: false,
            enabled: !_saving,
            textInputAction: TextInputAction.done,
            onSubmit: (_) => _submit(),
            error: _submitted && _codeError != null ? Text(_codeError!) : null,
          ),
          if (_hint case final hint?)
            Text(
              hint,
              style: context.theme.typography.body.sm.copyWith(
                color: context.theme.colors.mutedForeground,
              ),
            ),
          if (_error case final error?) ErrorMessage(error),
        ],
      ),
    );
    final actions = [
      ActionButton(
        AppStrings.cancel,
        onPress: _saving ? null : () => Navigator.pop(context),
      ),
      ActionButton('保存', primary: true, onPress: _saving ? null : _submit),
    ];
    if (widget.animation case final animation?) {
      return AppDialog(
        title: title,
        animation: animation,
        actions: actions,
        child: fields,
      );
    }
    return Padding(
      padding: const EdgeInsets.all(24),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 20,
          children: [
            Text(title, style: context.theme.typography.display.lg),
            fields,
            Align(
              alignment: Alignment.centerRight,
              child: ActionRow(children: actions),
            ),
          ],
        ),
      ),
    );
  }
}
