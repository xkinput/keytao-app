import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show SystemUiOverlayStyle;

import 'controller.dart';
import 'options.dart';

class AndroidScaffold extends StatelessWidget {
  const AndroidScaffold({
    super.key,
    required this.body,
    this.bottomNavigationBar,
  });
  final Widget body;
  final Widget? bottomNavigationBar;

  @override
  Widget build(BuildContext context) => AnnotatedRegion<SystemUiOverlayStyle>(
    value:
        (Theme.of(context).brightness == Brightness.dark
                ? SystemUiOverlayStyle.light
                : SystemUiOverlayStyle.dark)
            .copyWith(
              statusBarColor: Colors.transparent,
              systemNavigationBarColor: Colors.transparent,
              systemNavigationBarContrastEnforced: false,
            ),
    child: Scaffold(body: body, bottomNavigationBar: bottomNavigationBar),
  );
}

class ContentWidth extends StatelessWidget {
  const ContentWidth({super.key, required this.child});
  final Widget child;
  @override
  Widget build(BuildContext context) => Align(
    alignment: Alignment.topCenter,
    child: ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 720),
      child: child,
    ),
  );
}

class SectionCard extends StatelessWidget {
  const SectionCard({
    super.key,
    required this.title,
    required this.children,
    this.icon,
  });
  final String title;
  final List<Widget> children;
  final IconData? icon;
  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (icon != null) ...[
                Icon(icon, color: Theme.of(context).colorScheme.primary),
                const SizedBox(width: 12),
              ],
              Expanded(
                child: Text(
                  title,
                  style: Theme.of(context).textTheme.titleLarge,
                ),
              ),
            ],
          ),
          const SizedBox(height: 20),
          ...children,
        ],
      ),
    ),
  );
}

class ChoiceSetting<T> extends StatelessWidget {
  const ChoiceSetting({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    this.onChanged,
    this.disabled = const {},
    this.currentLabel,
    this.hint,
    this.icon,
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  final String? currentLabel;
  final String? hint;
  final IconData? icon;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 24),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            if (icon != null) ...[
              Icon(icon, size: 20),
              const SizedBox(width: 8),
            ],
            Expanded(
              child: Text(label, style: Theme.of(context).textTheme.titleSmall),
            ),
            if (currentLabel != null) Text(currentLabel!),
          ],
        ),
        const SizedBox(height: 8),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final entry in options.entries)
              ChoiceChip(
                label: Text(entry.value),
                selected: value == entry.key,
                onSelected: onChanged == null || disabled.contains(entry.key)
                    ? null
                    : (_) => onChanged!(entry.key),
              ),
          ],
        ),
        if (hint != null) ...[
          const SizedBox(height: 8),
          Text(hint!, style: Theme.of(context).textTheme.bodySmall),
        ],
      ],
    ),
  );
}

class SliderSetting extends StatefulWidget {
  const SliderSetting({
    super.key,
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    this.unit = '',
    this.onChanged,
    this.semanticLabel,
  });
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String unit;
  final ValueChanged<double>? onChanged;
  final String? semanticLabel;
  @override
  State<SliderSetting> createState() => _SliderSettingState();
}

class _SliderSettingState extends State<SliderSetting> {
  double? _draft;
  @override
  void didUpdateWidget(covariant SliderSetting oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value || widget.onChanged == null) {
      _draft = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final value = (_draft ?? widget.value).clamp(widget.min, widget.max);
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        children: [
          Row(
            children: [
              const Icon(Icons.tune, size: 20),
              const SizedBox(width: 8),
              Expanded(child: Text(widget.label)),
              Text('${value.round()}${widget.unit}'),
            ],
          ),
          Slider(
            value: value,
            min: widget.min,
            max: widget.max,
            divisions: widget.divisions,
            label: '${value.round()}${widget.unit}',
            semanticFormatterCallback: (v) =>
                '${widget.semanticLabel ?? widget.label} ${v.round()}${widget.unit}',
            onChanged: widget.onChanged == null
                ? null
                : (v) => setState(() => _draft = v),
            onChangeEnd: widget.onChanged == null
                ? null
                : (v) {
                    widget.onChanged!(v);
                    setState(() => _draft = null);
                  },
          ),
        ],
      ),
    );
  }
}

class ValueRow extends StatelessWidget {
  const ValueRow(this.label, this.value, {super.key});
  final String label;
  final String value;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 8),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 96,
          child: Text(label, style: Theme.of(context).textTheme.labelLarge),
        ),
        Expanded(child: SelectableText(value)),
      ],
    ),
  );
}

class AppPageBody extends StatelessWidget {
  const AppPageBody({
    super.key,
    required this.controller,
    required this.page,
    required this.children,
  });
  final AppController controller;
  final AppPage page;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return SingleChildScrollView(
      key: PageStorageKey(page.id),
      primary: false,
      padding: const EdgeInsets.all(16),
      child: ContentWidth(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 16,
          children: [
            if (c.appUpdate?.hasUpdate == true) AppUpdateBanner(controller: c),
            if (c.unhandledError case final error?) ErrorMessage(error),
            ...children,
          ],
        ),
      ),
    );
  }
}

class AppUpdateBanner extends StatelessWidget {
  const AppUpdateBanner({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final update = controller.appUpdate!;
    return Card.filled(
      color: Theme.of(context).colorScheme.primaryContainer,
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        leading: const Icon(Icons.system_update_outlined),
        title: const Text(AppStrings.appUpdate),
        subtitle: Wrap(
          spacing: 12,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            Chip(label: Text('v${update.latestVersion}')),
            Text(AppStrings.currentVersion(update.currentVersion)),
          ],
        ),
        trailing: const Icon(Icons.open_in_new, size: 20),
        onTap: controller.openAppUpdate,
      ),
    );
  }
}

class AppBrand extends StatelessWidget {
  const AppBrand({super.key, required this.version});
  final String version;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 12),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Image.asset(logoAsset, width: 48, height: 48, semanticLabel: 'KeyTao'),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Wrap(
                spacing: 12,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Text(
                    AppStrings.title,
                    style: Theme.of(context).textTheme.titleLarge,
                  ),
                  Text(
                    'v$version',
                    style: Theme.of(context).textTheme.labelMedium,
                  ),
                ],
              ),
              const SizedBox(height: 4),
              Text(
                AppStrings.tagline,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          ),
        ),
      ],
    ),
  );
}

Color successColor(BuildContext context) =>
    Theme.of(context).brightness == Brightness.dark
    ? Colors.green.shade300
    : Colors.green.shade800;

Color warningColor(BuildContext context) =>
    Theme.of(context).brightness == Brightness.dark
    ? Colors.amber.shade300
    : Colors.amber.shade900;

class StatusMessage extends StatelessWidget {
  const StatusMessage(
    this.message, {
    super.key,
    this.icon = Icons.info_outline,
    this.color,
  });
  final String message;
  final IconData icon;
  final Color? color;

  @override
  Widget build(BuildContext context) => Semantics(
    liveRegion: true,
    child: Container(
      margin: const EdgeInsets.symmetric(vertical: 6),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color:
            color?.withValues(alpha: 0.08) ??
            Theme.of(context).colorScheme.surfaceContainer,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 20, color: color),
          const SizedBox(width: 8),
          Expanded(
            child: SelectableText(message, style: TextStyle(color: color)),
          ),
        ],
      ),
    ),
  );
}

class ErrorMessage extends StatelessWidget {
  const ErrorMessage(this.message, {super.key});
  final String? message;

  @override
  Widget build(BuildContext context) => message?.isNotEmpty == true
      ? StatusMessage(
          message!,
          icon: Icons.error_outline,
          color: Theme.of(context).colorScheme.error,
        )
      : const SizedBox.shrink();
}

class LogLines extends StatelessWidget {
  const LogLines({
    super.key,
    required this.lines,
    required this.storageKey,
    this.height = 240,
    this.colorCoded = false,
  });
  final List<String> lines;
  final String storageKey;
  final double height;
  final bool colorCoded;

  @override
  Widget build(BuildContext context) => Container(
    height: height,
    decoration: BoxDecoration(
      color: Theme.of(context).colorScheme.surfaceContainer,
      borderRadius: BorderRadius.circular(12),
    ),
    child: lines.isEmpty
        ? const Center(child: Text(AppStrings.noLogs))
        : ListView.builder(
            key: PageStorageKey(storageKey),
            primary: false,
            padding: const EdgeInsets.all(12),
            itemCount: lines.length,
            itemBuilder: (context, index) => Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: SelectableText(
                lines[index],
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                  fontFamily: 'monospace',
                  color: !colorCoded
                      ? null
                      : switch (operationLogKind(lines[index])) {
                          LogLineKind.error => Theme.of(
                            context,
                          ).colorScheme.error,
                          LogLineKind.warning => warningColor(context),
                          LogLineKind.deploy => successColor(context),
                          LogLineKind.merged => Theme.of(
                            context,
                          ).colorScheme.primary,
                          LogLineKind.normal => Theme.of(
                            context,
                          ).colorScheme.onSurface,
                        },
                ),
              ),
            ),
          ),
  );
}
