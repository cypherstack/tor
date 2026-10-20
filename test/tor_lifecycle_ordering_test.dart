import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin.dart';

/// Stands in for the Rust library. Client creation is immediate, while each
/// network bootstrap stays pending until the test releases or cancels it.
class FakeTorNative implements TorNative {
  final pendingBootstraps = <Completer<void>>[];
  final startDirectories = <String>[];
  final bootstrapTimeouts = <Duration>[];
  final liveClients = <int>{};
  final liveProxies = <int>{};
  final createdProxies = <int>{};
  int maxLiveClients = 0;
  int cancelCalls = 0;
  int helloCalls = 0;
  int freeSpawnFailures = 0;
  int stopProxySpawnFailures = 0;
  Object? freeError;
  Completer<void>? startGate;
  Completer<void>? freeGate;

  /// Native teardown calls in the order they completed.
  final teardownOrder = <String>[];
  int _nextAddress = 0x1000;

  Future<void> waitForBootstraps(int count) async {
    while (pendingBootstraps.length < count) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
  }

  @override
  Future<TorHandles> start(int port, String stateDir, String cacheDir) async {
    startDirectories.add(stateDir);
    await startGate?.future;
    final client = _nextAddress += 0x10;
    final proxy = _nextAddress += 0x10;
    liveClients.add(client);
    createdProxies.add(proxy);
    if (liveClients.length > maxLiveClients) {
      maxLiveClients = liveClients.length;
    }
    return (
      client: Pointer<Void>.fromAddress(client),
      proxy: Pointer<Void>.fromAddress(proxy),
      port: 19050,
    );
  }

  @override
  Future<void> bootstrap(Pointer<Void> client, Duration timeout) async {
    bootstrapTimeouts.add(timeout);
    final gate = Completer<void>();
    pendingBootstraps.add(gate);
    await gate.future;
  }

  @override
  void cancelBootstrap(Pointer<Void> client) {
    cancelCalls++;
    for (final bootstrap in pendingBootstraps) {
      if (!bootstrap.isCompleted) {
        bootstrap.completeError(Exception('bootstrap cancelled'));
      }
    }
  }

  @override
  void hello() => helloCalls++;

  @override
  void setDormant(Pointer<Void> client, bool softMode) {}

  @override
  void startProxy(Pointer<Void> proxy) {
    if (createdProxies.contains(proxy.address)) {
      liveProxies.add(proxy.address);
    }
  }

  @override
  Future<void> stopProxy(Pointer<Void> proxy) async {
    if (proxy == nullptr) {
      return;
    }
    if (stopProxySpawnFailures > 0) {
      stopProxySpawnFailures--;
      throw Exception('stopProxy isolate failed to spawn');
    }
    // Complete later, like the native stop does once it joins the accept loop.
    await Future<void>.delayed(Duration.zero);
    liveProxies.remove(proxy.address);
    createdProxies.remove(proxy.address);
    teardownOrder.add('stopProxy');
  }

  @override
  Future<void> freeClient(Pointer<Void> client) async {
    if (client == nullptr) {
      return;
    }
    if (freeSpawnFailures > 0) {
      freeSpawnFailures--;
      throw Exception('freeClient isolate failed to spawn');
    }
    // Complete later, like the native free does, so tests catch callers that
    // do not wait for it.
    await Future<void>.delayed(Duration.zero);
    await freeGate?.future;
    if (freeError case final error?) {
      throw error;
    }
    liveClients.remove(client.address);
    teardownOrder.add('freeClient');
  }
}

/// Start [tor] and complete its bootstrap so it reports on.
Future<void> startAndBootstrap(
  Tor tor,
  FakeTorNative native,
  String dir,
) async {
  final started = tor.start(torDataDirPath: dir);
  final index = native.pendingBootstraps.length;
  await native.waitForBootstraps(index + 1);
  native.pendingBootstraps[index].complete();
  await started;
  expect(tor.status, TorStatus.on);
}

/// A skip reason when this machine cannot create symlinks, otherwise false.
///
/// Windows allows them only with Developer Mode or elevation.
final Object symlinkSkip = () {
  final probe = Directory.systemTemp.createTempSync('tor-symlink-');
  final sep = Platform.pathSeparator;
  try {
    final target = Directory('${probe.path}${sep}target')..createSync();
    Link('${probe.path}${sep}link').createSync(target.path);
    return false;
  } on FileSystemException {
    return 'creating symlinks needs Developer Mode or elevation';
  } finally {
    probe.deleteSync(recursive: true);
  }
}();

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

  test('hello uses the injected native layer', () {
    tor.hello();
    expect(native.helloCalls, 1);
    expect(native.liveClients, isEmpty);
  });

  test('stop cancels a pending bootstrap and tears down its handles', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    final startFailure = expectLater(started, throwsException);
    await native.waitForBootstraps(1);

    final stopped = tor.stop();
    await startFailure;
    await stopped;

    expect(native.cancelCalls, 1);
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });

  test('concurrent starts wait for the same attempt', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    final second = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);

    var secondDone = false;
    unawaited(second.then((_) => secondDone = true));
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(secondDone, isFalse);

    native.pendingBootstraps[0].complete();
    await Future.wait([first, second]);

    expect(tor.status, TorStatus.on);
    expect(native.pendingBootstraps, hasLength(1));
    await tor.stop();
  });

  test('the proxy is not active until bootstrap succeeds', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);

    expect(native.liveProxies, isEmpty);
    native.pendingBootstraps[0].complete();
    await started;
    expect(native.liveProxies, hasLength(1));

    await tor.stop();
  });

  test('concurrent starts both see a failed attempt', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    final second = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);

    native.pendingBootstraps[0].completeError(Exception('bootstrap failed'));

    await expectLater(first, throwsException);
    await expectLater(second, throwsException);
    expect(tor.status, TorStatus.off);
    expect(native.pendingBootstraps, hasLength(1));
  }, timeout: const Timeout(Duration(seconds: 10)));

  test(
    'a bootstrap timeout releases handles and reaches every caller',
    () async {
      const timeout = Duration(seconds: 3);
      final first = tor.start(
        torDataDirPath: dataDir.path,
        bootstrapTimeout: timeout,
      );
      final second = tor.start(
        torDataDirPath: dataDir.path,
        bootstrapTimeout: timeout,
      );
      await native.waitForBootstraps(1);

      native.pendingBootstraps[0].completeError(
        const TorBootstrapTimeout(
          timeout,
          'Tor bootstrap timed out after 3000 ms',
        ),
      );

      final isTimeout = isA<TorBootstrapTimeout>().having(
        (error) => error.timeout,
        'timeout',
        timeout,
      );
      await expectLater(first, throwsA(isTimeout));
      await expectLater(second, throwsA(isTimeout));
      expect(tor.status, TorStatus.off);
      expect(native.liveClients, isEmpty);
      expect(native.liveProxies, isEmpty);
      expect(native.pendingBootstraps, hasLength(1));
    },
  );

  test('a failed bootstrap releases the native handles', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].completeError(Exception('bootstrap failed'));

    await expectLater(started, throwsException);
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });

  test('disable stops the proxy', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].complete();
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
    final firstFailure = expectLater(first, throwsException);
    await native.waitForBootstraps(1);

    final stopped = tor.stop();
    final second = tor.start(torDataDirPath: dataDir.path);
    await firstFailure;
    await stopped;

    await native.waitForBootstraps(2);
    native.pendingBootstraps[1].complete();
    await second;

    expect(tor.status, TorStatus.on);
    expect(native.liveClients, hasLength(1));
    expect(native.maxLiveClients, 1);

    await tor.stop();
    expect(native.liveClients, isEmpty);
    expect(native.liveProxies, isEmpty);
  });

  test('stop then start on a running instance restarts Tor', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    final firstClient = native.liveClients.single;

    // Neither call is awaited before the next, as a toggle handler does.
    final stopped = tor.stop();
    final restarted = tor.start(torDataDirPath: dataDir.path);
    await stopped;
    // The queued restart begins as soon as the stop completes.
    expect(tor.status, isNot(TorStatus.on));

    await native.waitForBootstraps(2);
    native.pendingBootstraps[1].complete();
    await restarted;

    // Before the fix start() saw the stale "on" status, returned at once, and
    // the queued stop then left Tor off while the caller thought it was on.
    expect(tor.status, TorStatus.on);
    expect(tor.port, 19050);
    expect(native.liveClients.single, isNot(firstClient));
    expect(native.maxLiveClients, 1);
    await tor.stop();
  });

  test('a start queued before stop is cancelled by it', () async {
    await startAndBootstrap(tor, native, dataDir.path);

    final queued = tor.start(torDataDirPath: dataDir.path);
    final stopped = tor.stop();

    await expectLater(queued, throwsA(isA<TorStartCancelled>()));
    await stopped;
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    // Nothing was bootstrapping, so there was nothing to cancel natively.
    expect(native.cancelCalls, 0);
  });

  test(
    'a stop during a reusing start cancels it instead of reporting on',
    () async {
      await startAndBootstrap(tor, native, dataDir.path);

      final reused = tor.start(torDataDirPath: dataDir.path);
      // The queued start is now resolving the data directory, which cannot
      // finish until the event loop advances beyond this microtask.
      await Future<void>.value();
      final stopped = tor.stop();

      // Before the fix the start saw the running instance and returned success,
      // and the queued stop then turned Tor off right after.
      await expectLater(reused, throwsA(isA<TorStartCancelled>()));
      await stopped;
      expect(tor.status, TorStatus.off);
      expect(native.liveClients, isEmpty);
      expect(native.cancelCalls, 0);
    },
  );

  test('stop during failed-start cleanup leaves the client alone', () async {
    final gate = native.freeGate = Completer<void>();
    final started = tor.start(torDataDirPath: dataDir.path);
    final failure = expectLater(started, throwsException);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].completeError(Exception('bootstrap failed'));
    while (native.createdProxies.isNotEmpty) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }

    // The proxy is gone and the client is being freed; status is still
    // starting and the handle is still set.
    expect(tor.status, TorStatus.starting);
    expect(native.liveClients, hasLength(1));
    final stopped = tor.stop();
    // Cancelling a bootstrap on a client that is being freed would race the
    // free, so stop must not touch the handle.
    expect(native.cancelCalls, 0);

    gate.complete();
    await failure;
    await stopped;
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
  });

  test('a start cancelled by an immediate stop leaves Tor off', () async {
    final started = tor.start(torDataDirPath: dataDir.path);
    expect(tor.status, TorStatus.starting);
    final stopped = tor.stop();

    await expectLater(started, throwsA(isA<TorStartCancelled>()));
    await stopped;
    expect(tor.status, TorStatus.off);
    expect(native.startDirectories, isEmpty);
  });

  test('a running instance rejects a different data directory', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    final other = await Directory.systemTemp.createTemp('tor-ordering-other-');
    addTearDown(() => other.delete(recursive: true));

    await expectLater(
      tor.start(torDataDirPath: other.path),
      throwsA(isA<StateError>()),
    );

    // The rejection must not tear down the running instance.
    expect(tor.status, TorStatus.on);
    expect(native.liveClients, hasLength(1));
    expect(native.startDirectories, hasLength(1));
    await tor.stop();
  });

  test('status stays on until the native client is released', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    final gate = native.freeGate = Completer<void>();

    final stopped = tor.stop();
    while (native.liveProxies.isNotEmpty) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }

    // The proxy is gone but the client, and so its files, is still live.
    expect(native.liveClients, hasLength(1));
    expect(tor.status, TorStatus.on);
    expect(
      () => tor.port,
      throwsA(
        isA<Exception>().having(
          (e) => e.toString(),
          'message',
          'Exception: Tor proxy is not running',
        ),
      ),
    );
    await expectLater(
      tor.setClientDormant(true),
      throwsA(isA<ClientNotActive>()),
    );

    gate.complete();
    await stopped;
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
  });

  test('a failed free is retried by the next stop', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    native.freeSpawnFailures = 1;

    await expectLater(tor.stop(), throwsException);
    expect(tor.status, TorStatus.on);
    expect(native.liveClients, hasLength(1));
    await tor.stop();
    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
  });

  test('a failed free is released before the next start', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    native.freeSpawnFailures = 1;

    await expectLater(tor.stop(), throwsException);
    expect(tor.status, TorStatus.on);
    expect(native.liveClients, hasLength(1));
    await startAndBootstrap(tor, native, dataDir.path);
    expect(native.maxLiveClients, 1);
    expect(native.startDirectories, hasLength(2));
    await tor.stop();
  });

  test('a failed proxy stop is retried', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    native.stopProxySpawnFailures = 1;

    await expectLater(tor.stop(), throwsException);
    expect(tor.status, TorStatus.on);
    expect(native.liveProxies, hasLength(1));
    expect(native.liveClients, hasLength(1));
    expect(native.teardownOrder, isEmpty);
    await tor.stop();
    expect(native.liveProxies, isEmpty);
    expect(native.liveClients, isEmpty);
    expect(native.teardownOrder, ['stopProxy', 'freeClient']);
    expect(tor.status, TorStatus.off);
  });

  test('a failed proxy stop is released before the next start', () async {
    await startAndBootstrap(tor, native, dataDir.path);
    native.stopProxySpawnFailures = 1;
    await expectLater(tor.stop(), throwsException);
    expect(native.liveClients, hasLength(1));
    await startAndBootstrap(tor, native, dataDir.path);
    expect(native.maxLiveClients, 1);
    expect(native.startDirectories, hasLength(2));
    expect(native.teardownOrder, ['stopProxy', 'freeClient']);
    await tor.stop();
  });

  test('failed startup cleanup never reports on and is retried', () async {
    native.freeSpawnFailures = 1;
    final started = tor.start(torDataDirPath: dataDir.path);
    final failed = expectLater(started, throwsException);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].completeError(Exception('bootstrap failed'));
    await failed;
    expect(tor.status, TorStatus.starting);
    expect(native.liveClients, hasLength(1));
    await startAndBootstrap(tor, native, dataDir.path);
    expect(native.maxLiveClients, 1);
    expect(native.startDirectories, hasLength(2));
    await tor.stop();
  });

  test('stop releases the proxy before the client', () async {
    await startAndBootstrap(tor, native, dataDir.path);

    await tor.stop();

    expect(native.teardownOrder, ['stopProxy', 'freeClient']);
  });

  test(
    'a sub-millisecond timeout is rejected before the native start',
    () async {
      await expectLater(
        tor.start(
          torDataDirPath: dataDir.path,
          bootstrapTimeout: const Duration(microseconds: 500),
        ),
        throwsA(isA<ArgumentError>()),
      );

      expect(native.startDirectories, isEmpty);
      expect(tor.status, TorStatus.off);
    },
  );

  test('trailing separators do not change the data directory', () async {
    final slashed = '${dataDir.path}${Platform.pathSeparator}';
    final first = tor.start(torDataDirPath: slashed);
    final second = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);

    native.pendingBootstraps[0].complete();
    await Future.wait([first, second]);

    expect(native.startDirectories, hasLength(1));
    await tor.start(torDataDirPath: slashed);
    expect(tor.status, TorStatus.on);
    await tor.stop();
  });

  test('a symlink and its target share one start', () async {
    final sep = Platform.pathSeparator;
    final real = Directory('${dataDir.path}${sep}real')..createSync();
    final link = Link('${dataDir.path}${sep}link')..createSync(real.path);

    final first = tor.start(torDataDirPath: link.path);
    final second = tor.start(torDataDirPath: real.path);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].complete();
    await Future.wait([first, second]);

    expect(native.startDirectories, hasLength(1));
    await tor.stop();
  }, skip: symlinkSkip);

  test(
    'a parent segment after a symlink follows the link',
    () async {
      final sep = Platform.pathSeparator;
      final target = Directory('${dataDir.path}${sep}a${sep}target')
        ..createSync(recursive: true);
      Link('${dataDir.path}${sep}link').createSync(target.path);

      // Lexically this is dataDir/sibling; the file system resolves it to
      // dataDir/a/sibling.
      await startAndBootstrap(
        tor,
        native,
        '${dataDir.path}${sep}link$sep..${sep}sibling',
      );

      final physical = Directory('${dataDir.path}${sep}a${sep}sibling')
          .resolveSymbolicLinksSync();
      expect(native.startDirectories[0], '$physical/tor_state');
      expect(Directory('${dataDir.path}${sep}sibling').existsSync(), isFalse);
      await tor.stop();
    },
    skip: Platform.isWindows ? 'Windows resolves .. before links' : symlinkSkip,
  );

  test('on Windows a parent segment after a symlink is lexical', () async {
    final sep = Platform.pathSeparator;
    final target = Directory('${dataDir.path}${sep}a${sep}target')
      ..createSync(recursive: true);
    Link('${dataDir.path}${sep}link').createSync(target.path);

    // Win32 normalizes the path before the file system sees the link.
    await startAndBootstrap(
      tor,
      native,
      '${dataDir.path}${sep}link$sep..${sep}sibling',
    );

    final physical = Directory('${dataDir.path}${sep}sibling')
        .resolveSymbolicLinksSync();
    expect(native.startDirectories[0], '$physical/tor_state');
    expect(
      Directory('${dataDir.path}${sep}a${sep}sibling').existsSync(),
      isFalse,
    );
    await tor.stop();
  }, skip: Platform.isWindows ? symlinkSkip : 'Win32 path semantics');

  test('an unusable data directory fails while it is resolved', () async {
    final sep = Platform.pathSeparator;
    final file = File('${dataDir.path}${sep}file')..createSync();

    // The error names the data directory itself, not a subdirectory that a
    // queued start failed to create.
    await expectLater(
      tor.start(torDataDirPath: file.path),
      throwsA(
        isA<FileSystemException>().having((e) => e.path, 'path', file.path),
      ),
    );

    expect(tor.status, TorStatus.off);
    expect(native.startDirectories, isEmpty);
    // The failure is not left behind as a pending start.
    await startAndBootstrap(tor, native, dataDir.path);
    await tor.stop();
  });

  test('a pending start rejects a different data directory', () async {
    final first = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);
    final other = await Directory.systemTemp.createTemp('tor-ordering-other-');
    addTearDown(() => other.delete(recursive: true));

    final second = tor.start(torDataDirPath: other.path);
    final secondFailure = expectLater(second, throwsA(isA<StateError>()));
    native.pendingBootstraps[0].complete();
    await first;
    await secondFailure;

    expect(tor.status, TorStatus.on);
    expect(native.startDirectories, hasLength(1));
    await tor.stop();
  });

  test(
    'directory preparation reports starting and preserves running status',
    () async {
      final started = tor.start(torDataDirPath: dataDir.path);
      expect(tor.status, TorStatus.starting);
      // The queued start has begun, but filesystem I/O cannot finish until the
      // event loop advances beyond this microtask.
      await Future<void>.value();
      expect(tor.status, TorStatus.starting);
      expect(native.startDirectories, isEmpty);
      await native.waitForBootstraps(1);
      native.pendingBootstraps[0].complete();
      await started;

      final reused = tor.start(torDataDirPath: dataDir.path);
      await Future<void>.value();
      expect(tor.status, TorStatus.on);
      await reused;
      expect(native.startDirectories, hasLength(1));
      await tor.stop();
    },
  );

  test('start defers data directory creation to the queue', () async {
    final fresh = Directory('${dataDir.path}/fresh');
    final started = tor.start(torDataDirPath: fresh.path);
    expect(fresh.existsSync(), isFalse);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].complete();
    await started;
    await tor.stop();
  });

  test(
    'a start with a different timeout reuses the running instance',
    () async {
      final first = tor.start(torDataDirPath: dataDir.path);
      final second = tor.start(
        torDataDirPath: dataDir.path,
        bootstrapTimeout: const Duration(seconds: 17),
      );
      final both = Future.wait([first, second]);
      await native.waitForBootstraps(1);
      native.pendingBootstraps[0].complete();
      await both;

      expect(native.bootstrapTimeouts, hasLength(1));
      expect(tor.status, TorStatus.on);
      await tor.stop();
    },
  );

  test('stop cancels every queued start', () async {
    final gate = native.startGate = Completer<void>();
    final first = tor.start(torDataDirPath: dataDir.path);
    final firstFailure = expectLater(first, throwsA(isA<TorStartCancelled>()));
    while (native.startDirectories.isEmpty) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
    final second = tor.start(
      torDataDirPath: '${dataDir.path}${Platform.pathSeparator}',
    );
    final secondFailure = expectLater(
      second.timeout(const Duration(seconds: 2)),
      throwsA(isA<TorStartCancelled>()),
    );
    final stopped = tor.stop();
    gate.complete();
    await firstFailure;
    await secondFailure;
    await stopped;

    expect(tor.status, TorStatus.off);
    expect(native.liveClients, isEmpty);
    expect(native.startDirectories, hasLength(1));
  });

  test('cleanup failure does not replace the bootstrap failure', () async {
    native.freeError = Exception('cleanup failed');
    final started = tor.start(torDataDirPath: dataDir.path);
    await native.waitForBootstraps(1);
    native.pendingBootstraps[0].completeError(Exception('bootstrap failed'));

    await expectLater(
      started,
      throwsA(
        isA<Exception>().having(
          (error) => error.toString(),
          'message',
          contains('bootstrap failed'),
        ),
      ),
    );
    expect(tor.status, TorStatus.starting);
    expect(native.freeError, same(tor.lastCleanupError));
    expect(tor.lastCleanupStackTrace, isNotNull);
  });

  for (final loggingThrows in [false, true]) {
    test(
      'cleanup failure is reported without masking the typed start error (logging throws: $loggingThrows)',
      () async {
        final messages = <String>[];
        const primary = TorBootstrapTimeout(
          Duration(seconds: 1),
          'deadline passed',
        );
        native.freeSpawnFailures = 1;
        final started = runZoned(
          () => tor.start(torDataDirPath: dataDir.path),
          zoneSpecification: ZoneSpecification(
            print: (self, parent, zone, line) {
              messages.add(line);
              if (loggingThrows) throw StateError('log sink failed');
            },
          ),
        );
        final failure = expectLater(started, throwsA(same(primary)));
        await native.waitForBootstraps(1);
        native.pendingBootstraps[0].completeError(primary);
        await failure;
        expect(messages, hasLength(1));
        expect(messages.single, contains('freeClient isolate failed to spawn'));
        expect(tor.lastCleanupError, isNotNull);
        expect(tor.lastCleanupStackTrace, isNotNull);
        await tor.stop();
        expect(tor.status, TorStatus.off);
      },
    );
  }

  test(
    'stop before client creation prevents bootstrap from starting',
    () async {
      final gate = native.startGate = Completer<void>();
      final started = tor.start(torDataDirPath: dataDir.path);
      final startFailure = expectLater(
        started,
        throwsA(isA<TorStartCancelled>()),
      );
      while (native.startDirectories.isEmpty) {
        await Future<void>.delayed(const Duration(milliseconds: 5));
      }

      final stopped = tor.stop();
      gate.complete();
      await startFailure;
      await stopped;

      expect(native.pendingBootstraps, isEmpty);
      expect(native.liveClients, isEmpty);
      expect(native.liveProxies, isEmpty);
    },
  );

  test('forwards a finite bootstrap timeout', () async {
    const timeout = Duration(seconds: 17);
    final started = tor.start(
      torDataDirPath: dataDir.path,
      bootstrapTimeout: timeout,
    );
    await native.waitForBootstraps(1);

    expect(native.bootstrapTimeouts, [timeout]);
    native.pendingBootstraps[0].complete();
    await started;
    await tor.stop();
  });
}
