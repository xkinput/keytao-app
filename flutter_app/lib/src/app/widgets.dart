import 'package:flutter/material.dart';

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
  const SectionCard({super.key, required this.title, required this.children});
  final String title;
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(title, style: Theme.of(context).textTheme.titleLarge),
          const SizedBox(height: 24),
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
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 24),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: Theme.of(context).textTheme.titleSmall),
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
  });
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String unit;
  final ValueChanged<double>? onChanged;
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
                '${widget.label} ${v.round()}${widget.unit}',
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
