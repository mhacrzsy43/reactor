import 'package:flutter_test/flutter_test.dart';

import 'package:reactor_flutter/main.dart';

void main() {
  testWidgets('exposes the shared ready markers and list workload', (
    WidgetTester tester,
  ) async {
    await tester.pumpWidget(const ReactorApp());

    expect(find.text('Reactor ready'), findsOneWidget);
    expect(find.text('List scenario'), findsOneWidget);
    expect(find.text('Update scenario'), findsOneWidget);
    expect(find.text('Animation scenario'), findsOneWidget);

    await tester.tap(find.text('List scenario'));
    await tester.pumpAndSettle();

    expect(find.text('List ready'), findsOneWidget);
    expect(find.text('Item 0'), findsOneWidget);
    expect(find.text('Deterministic value ${deterministicValue(0)}'), findsOneWidget);
  });

  test('uses the shared 32-bit deterministic data contract', () {
    expect(deterministicValue(0), 818);
    expect(deterministicValue(1), 6063);
    expect(deterministicValue(499, 80), 7561);
  });
}
