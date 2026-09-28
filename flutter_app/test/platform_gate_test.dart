import 'package:keytao/src/rust/api/types.dart';

import 'unified_gate_cases.dart';

void main() {
  for (final platform in [
    BridgePlatform.windows,
    BridgePlatform.linux,
    BridgePlatform.ios,
  ]) {
    unifiedGateCases(platform);
  }
}
