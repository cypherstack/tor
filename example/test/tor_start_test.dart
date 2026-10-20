import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:tor_ffi_plugin_example/main.dart';
import 'package:socks_socket/socks_socket.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin.dart';

class _ExampleTorNative extends TorNative {
  int freeCalls = 0;
  int freeFailures = 0;
  Completer<void>? freeGate;

  @override
  Future<TorHandles> start(int port, String stateDir, String cacheDir) async =>
      (
        client: Pointer<Void>.fromAddress(1),
        proxy: Pointer<Void>.fromAddress(2),
        port: 19050,
      );

  @override
  Future<void> bootstrap(Pointer<Void> client, Duration timeout) async {}

  @override
  void startProxy(Pointer<Void> proxy) {}

  @override
  Future<void> stopProxy(Pointer<Void> proxy) async {}

  @override
  Future<void> freeClient(Pointer<Void> client) async {
    if (client == nullptr) return;
    freeCalls++;
    final gate = freeGate;
    freeGate = null;
    await gate?.future;
    if (freeFailures > 0) {
      freeFailures--;
      throw Exception('client release failed');
    }
  }
}

class _FailedWriteSocket implements SOCKSSocket {
  bool readStarted = false;
  bool closed = false;

  @override
  Future<void> connect() async {}

  @override
  Future<void> connectTo(String host, int port) async {}

  @override
  Future<void> write(Object? object, {bool newline = false}) async {
    throw Exception('request write failed');
  }

  @override
  Stream<List<int>> get inputStream {
    readStarted = true;
    return const Stream.empty();
  }

  @override
  Future<void> close() async {
    closed = true;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('plugins.flutter.io/path_provider');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  Future<(Tor, _ExampleTorNative)> pumpRunningTor(
    WidgetTester tester, {
    Future<SOCKSSocket> Function(int)? moneroSocketFactory,
  }) async {
    final directory = Directory.systemTemp.createTempSync('tor-example-stop-');
    final native = _ExampleTorNative();
    final tor = Tor.withNative(native);
    addTearDown(() => directory.deleteSync(recursive: true));
    messenger.setMockMethodCallHandler(channel, (_) async => directory.path);
    await tester.pumpWidget(
      MaterialApp(
        home: Home(tor: tor, moneroSocketFactory: moneroSocketFactory),
      ),
    );
    for (
      var attempt = 0;
      tor.status != TorStatus.on && attempt < 200;
      attempt++
    ) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 5)),
      );
      await tester.pump();
    }
    expect(tor.status, TorStatus.on);
    await tester.pumpAndSettle();
    return (tor, native);
  }

  testWidgets('a failed Monero request write reaches the error handler', (
    tester,
  ) async {
    final socket = _FailedWriteSocket();
    await pumpRunningTor(tester, moneroSocketFactory: (_) async => socket);
    final messages = <String>[];
    final originalDebugPrint = debugPrint;
    debugPrint = (message, {wrapWidth}) {
      if (message != null) messages.add(message);
    };
    try {
      final button = find.text('Test Monero onion node connection');
      await tester.ensureVisible(button);
      await tester.tap(button);
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(socket.readStarted, isFalse);
      expect(socket.closed, isTrue);
      expect(
        messages,
        contains(
          contains(
            'Error connecting to Monero onion node: Exception: request write failed',
          ),
        ),
      );
    } finally {
      debugPrint = originalDebugPrint;
    }
  });

  testWidgets('a stop failure is shown and can be retried', (tester) async {
    final (tor, native) = await pumpRunningTor(tester);
    native.freeFailures = 1;

    await tester.tap(find.text('Stop tor'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(tor.status, TorStatus.on);
    expect(
      find.text('Unable to stop Tor: Exception: client release failed'),
      findsOneWidget,
    );
    final requestButton = find.widgetWithText(
      TextButton,
      'Test Monero onion node connection',
    );
    expect(tester.widget<TextButton>(requestButton).onPressed, isNull);
    final stopButton = find.widgetWithText(TextButton, 'Stop tor');
    expect(tester.widget<TextButton>(stopButton).onPressed, isNotNull);

    await tester.tap(stopButton);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(native.freeCalls, 2);
    expect(find.textContaining('Unable to stop Tor:'), findsNothing);
    expect(tester.widget<TextButton>(stopButton).onPressed, isNull);
  });

  testWidgets('a stop failure after disposal does not update state', (
    tester,
  ) async {
    final (_, native) = await pumpRunningTor(tester);
    final gate = native.freeGate = Completer<void>();

    await tester.tap(find.text('Stop tor'));
    await tester.pump();
    await tester.pumpWidget(const SizedBox());
    gate.completeError(Exception('client release failed after disposal'));
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
  });

  testWidgets('a start failure is shown and the retry dialog closes', (
    tester,
  ) async {
    final error = PlatformException(
      code: 'support-directory-unavailable',
      message: 'Application support directory is unavailable',
    );
    messenger.setMockMethodCallHandler(channel, (_) async => throw error);

    await tester.pumpWidget(const MyApp());
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(find.text('Unable to start Tor: $error'), findsOneWidget);

    await tester.tap(find.text('Start tor'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(find.byType(Dialog), findsNothing);
    expect(find.text('Unable to start Tor: $error'), findsOneWidget);
  });

  testWidgets('a start failure after disposal does not update state', (
    tester,
  ) async {
    final directory = Completer<String?>();
    messenger.setMockMethodCallHandler(channel, (_) => directory.future);

    await tester.pumpWidget(const MyApp());
    await tester.pumpWidget(const SizedBox());
    directory.completeError(
      PlatformException(code: 'support-directory-unavailable'),
    );
    await tester.pump();

    expect(tester.takeException(), isNull);
  });
}
