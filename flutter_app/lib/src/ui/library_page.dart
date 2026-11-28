// ignore: deprecated_member_use
import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'settings.dart';
import 'user_words.dart';
import 'widgets.dart';

class LibraryPage extends StatelessWidget {
  const LibraryPage({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return AppPageBody(
      controller: c,
      page: AppPage.library,
      fullWidth: true,
      spacing: 0,
      children: [
        HairlineGridSection(
          children: [
            HairlineGridCell(
              child: Section(
                title: '自造词',
                trailing: ActionButton(
                  '查看自造词',
                  icon: FLucideIcons.bookOpen,
                  onPress:
                      c.busy ||
                          (c.installedPackages.isEmpty &&
                              c.local?.installed != true)
                      ? null
                      : () => openUserWords(context, c),
                ),
                children: [],
              ),
            ),
            if (c.isMobile)
              HairlineGridCell(
                child: Section(
                  title: '自定义短语',
                  children: [
                    SettingRow(
                      AppStrings.phrases,
                      child: ActionButton(
                        '编辑',
                        onPress: !c.busy && c.androidSettings != null
                            ? () => showFDialog<void>(
                                context: context,
                                barrierDismissible: false,
                                builder: (_, _, animation) => _PhrasesEditor(
                                  controller: c,
                                  animation: animation,
                                ),
                              )
                            : null,
                      ),
                    ),
                  ],
                ),
              ),
            if (c.info.platform != BridgePlatform.macOs)
              HairlineGridCell(child: AccountCard(controller: c)),
            HairlineGridCell(
              child: Section(
                title: '应用设置',
                children: [
                  SwitchSetting(
                    '显示网站',
                    value: !c.websiteHidden,
                    onChanged: c.canEditWebsiteSettings
                        ? (visible) => c.saveWebsiteSettings(hidden: !visible)
                        : null,
                  ),
                  SwitchSetting(
                    '减少动画',
                    value: c.reduceMotion,
                    onChanged: c.canSaveReduceMotion
                        ? c.saveReduceMotion
                        : null,
                  ),
                ],
              ),
            ),
            if (c.userWordsReport != null || c.userWordsReportError != null)
              UserWordsNotice(controller: c),
          ],
        ),
      ],
    );
  }
}

class _PhrasesEditor extends StatefulWidget {
  const _PhrasesEditor({required this.controller, required this.animation});
  final AppController controller;
  final Animation<double> animation;

  @override
  State<_PhrasesEditor> createState() => _PhrasesEditorState();
}

class _PhrasesEditorState extends State<_PhrasesEditor> {
  late final _phrases = [...widget.controller.androidSettings!.phrases];
  final _text = TextEditingController();
  bool _saving = false;
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  void _add() {
    final phrase = _text.text.trim();
    if (phrase.isEmpty) return;
    setState(() => _phrases.add(phrase));
    _text.clear();
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    final saved = await widget.controller.saveAndroid({'phrases': _phrases});
    if (!mounted) return;
    if (saved) {
      Navigator.pop(context);
    } else {
      setState(() {
        _saving = false;
        _error = widget.controller.error ?? '保存失败';
      });
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !_saving,
    child: AppDialog(
      title: AppStrings.phrases,
      animation: widget.animation,
      actions: [
        ActionButton(
          AppStrings.cancel,
          onPress: _saving ? null : () => Navigator.pop(context),
        ),
        ActionButton('保存', primary: true, onPress: _saving ? null : _save),
      ],
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          SizedBox(
            height: 220,
            child: ReorderableListView.builder(
              itemCount: _phrases.length,
              onReorderItem: (from, to) {
                if (_saving) return;
                setState(() {
                  _phrases.insert(to, _phrases.removeAt(from));
                });
              },
              itemBuilder: (context, index) => Padding(
                key: ValueKey('$index:${_phrases[index]}'),
                padding: const EdgeInsets.symmetric(vertical: 4),
                child: Row(
                  spacing: 8,
                  children: [
                    const Icon(
                      FLucideIcons.gripVertical,
                      size: 18,
                      semanticLabel: '拖动排序',
                    ),
                    Expanded(child: Text(_phrases[index])),
                    FButton.icon(
                      variant: FButtonVariant.ghost,
                      onPress: _saving
                          ? null
                          : () => setState(() => _phrases.removeAt(index)),
                      child: const Icon(
                        FLucideIcons.trash2,
                        size: 18,
                        semanticLabel: '移除短语',
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          FTextField(
            label: const Text('添加短语'),
            control: FTextFieldControl.managed(controller: _text),
            enabled: !_saving,
            onSubmit: (_) => _add(),
          ),
          ActionButton('添加', onPress: _saving ? null : _add),
          if (_error case final error?) ErrorMessage(error),
        ],
      ),
    ),
  );
}
