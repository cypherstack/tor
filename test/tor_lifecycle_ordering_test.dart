import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin.dart';

/// Stands in for the Rust library. Each native start stays pending until the
/// test releases it, and the fake tracks which native instances are alive.
class FakeTorNative implements TorNative {
  final pendingStarts = <Completer<void>>[];
  final liveClients = <int>{};
  final liveProxies = <int>{};
  int maxLiveClients = 0;
  bool failBootstrap = false;
  int _nextAddress = 0x1000;

  /// Wait until [count] native starts have been requested.
  Future<void> waitForStarts(int count) async {
    while (pendingStarts.length < count) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
  }

  @override
  Future<TorHandles> start(int port, String stateDir, String cacheDir) async {
    final gate = Completer<void>();
    pendingStarts.add(gate);
    await gate.future;

    final client = _nextAddress += 0x10;
    final proxy = _nextAddress += 0x10;
    liveClients.add(client);
    liveProxies.add(proxy);
    if (liveClients.length > maxLiveClients) {
      maxLiveClients = liveClients.length;
    }
    return (
      client: Pointer<Void>.fromAddress(client),
      proxy: Pointer<Void>.fromAddress(proxy),
    );
  }

  @override
  void bootstrap(Pointer<Void> client) {
    if (failBootstrap) {
      throw Exception('bootstrap failed');
    }
  }

  @override
  void setDormant(Pointer<Void> client, bool softMode) {}

  @override
  void stopProxy(Pointer<Void> proxy) => liveProxies.remove(proxy.address);

  @override
  void freeClient(Pointer<Void> client) => liveClients.remove(client.address);
}

void main() {
  late Directory dataDir;
  late FakeTorNative native;
  late Tor tor;

  setUp(() async {
    dataDir = await Directory.systemTemp.createTemp('tor-ordering-');
    native = FakeTorNative();
    tor = Tor.withNative(native);
  });

  tearDown(() => dataDir.delete(recursive: true));

  test('stop during start tears down what the start created', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);

    final stopped = tor.stop();
    native.pendingStarts[0].complete();
    await started;
    await stopped;

    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });

  test('concurrent starts wait for the same attempt', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    final second = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);

    var secondDone = false;
    unawaited(second.then((_) => secondDone = true));
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(secondDone, isFalse);

    native.pendingStarts[0].complete();
    await Future.wait([first, second]);

    expect(tor.status, TorStatus.on);
    expect(native.pendingStarts, hasLength(1));
    await tor.stop();
  });

  test('concurrent starts both see a failed attempt', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    final second = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);

    native.pendingStarts[0].completeError(Exception('bootstrap failed'));

    await expectLater(first, throwsException);
    await expectLater(second, throwsException);
    expect(tor.status, TorStatus.off);
    expect(native.pendingStarts, hasLength(1));
  }, timeout: const Timeout(Duration(seconds: 10)));

  test('a failed bootstrap releases the native handles', () async {
    native.failBootstrap = true;
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);
    native.pendingStarts[0].complete();

    await expectLater(started, throwsException);
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });

  test('disable stops the proxy', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);
    native.pendingStarts[0].complete();
    await started;

    await tor.disable();
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);

    // Callers that follow disable() with stop() keep working.
    await tor.stop();
  });

  test('restart during a pending start never overlaps instances', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    await native.waitForStarts(1);

    final stopped = tor.stop();
    final second = tor.start(torDataDirPath: dataDir.path);
    native.pendingStarts[0].complete();
    await first;
    await stopped;

    await native.waitForStarts(2);
    native.pendingStarts[1].complete();
    await second;

    expect(tor.status, TorStatus.on);
    expect(native.liveClients, hasLength(1));
    expect(native.maxLiveClients, 1);

    await tor.stop();
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });
}
