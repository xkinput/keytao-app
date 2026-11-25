import '../rust/api/timing.dart' as timing;

bool _enabled = false;

void enableAppOperationTiming() => _enabled = true;

timing.AppOperationSpan? beginAppOperation(timing.AppOperationName operation) =>
    _enabled ? timing.startAppOperation(operation: operation) : null;

Future<T> measureAppOperation<T>(
  timing.AppOperationName operation,
  Future<T> Function() action,
) {
  final span = beginAppOperation(operation);
  if (span == null) return action();
  return Future<T>.sync(action).whenComplete(span.dispose);
}
