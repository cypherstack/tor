// SPDX-FileCopyrightText: 2022 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';
import 'package:meta/meta.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin_bindings_generated.dart'
    as bindings;

class CouldntBootstrapDirectory implements Exception {
  String? rustError;

  CouldntBootstrapDirectory({this.rustError});
}

class TorStartCancelled implements Exception {
  const TorStartCancelled();

  @override
  String toString() => 'Tor start was cancelled';
}

/// Bootstrap did not finish within the deadline passed to [Tor.start].
class TorBootstrapTimeout implements Exception {
  const TorBootstrapTimeout(this.timeout, this.message);

  /// The deadline that passed.
  final Duration timeout;

  /// The native error message.
  final String message;

  @override
  String toString() => 'TorBootstrapTimeout: $message';
}

enum TorStatus { on, starting, off }

/// Native handles produced by a successful [TorNative.start].
typedef TorHandles = ({Pointer<Void> client, Pointer<Void> proxy, int port});

/// The native calls [Tor] makes. Tests substitute a fake to drive the
/// lifecycle without starting Tor.
@visibleForTesting
class TorNative {
  const TorNative();

  /// Start Tor with its SOCKS proxy on [port], in a worker isolate.
  ///
  /// Throws the Rust error if the native start fails.
  Future<TorHandles> start(int port, String stateDir, String cacheDir) async {
    final (client, proxy, boundPort) = await Isolate.run(
      () => _startInIsolate(port, stateDir, cacheDir),
    );
    return (
      client: Pointer<Void>.fromAddress(client),
      proxy: Pointer<Void>.fromAddress(proxy),
      port: boundPort,
    );
  }

  static (int, int, int) _startInIsolate(
    int port,
    String stateDir,
    String cacheDir,
  ) {
    // tor_start() copies both paths, so they are freed once it returns.
    final stateDirPtr = stateDir.toNativeUtf8();
    final cacheDirPtr = cacheDir.toNativeUtf8();
    try {
      final tor = bindings.tor_start(
        port,
        stateDirPtr.cast<Char>(),
        cacheDirPtr.cast<Char>(),
      );

      // Throw an exception if the Tor service fails to start.
      if (tor.client == nullptr) {
        Tor.throwRustException();
      }

      return (tor.client.address, tor.proxy.address, tor.port);
    } finally {
      malloc.free(stateDirPtr);
      malloc.free(cacheDirPtr);
    }
  }

  /// Bootstrap [client] in a worker isolate. Throws [TorStartCancelled] on
  /// cancellation, [TorBootstrapTimeout] when [timeout] elapses, and the Rust
  /// error on failure.
  Future<void> bootstrap(Pointer<Void> client, Duration timeout) async {
    final address = client.address;
    await Isolate.run(() => _bootstrapInIsolate(address, timeout));
  }

  static void _bootstrapInIsolate(int client, Duration timeout) {
    final code = bindings.tor_client_bootstrap(
      Pointer<Void>.fromAddress(client),
      // The native deadline has millisecond resolution.
      timeout.inMilliseconds,
    );
    switch (bindings.TorBootstrapStatus.fromValue(code)) {
      case bindings.TorBootstrapStatus.Ok:
        return;
      case bindings.TorBootstrapStatus.Cancelled:
        Tor._takeRustError();
        throw const TorStartCancelled();
      case bindings.TorBootstrapStatus.TimedOut:
        // Report the deadline as the caller passed it, not rounded.
        throw TorBootstrapTimeout(timeout, Tor._takeRustError());
      case bindings.TorBootstrapStatus.Failed:
        Tor.throwRustException();
      case bindings.TorBootstrapStatus.NullClient:
        throw ArgumentError(Tor._takeRustError());
    }
  }

  /// Interrupt an in-progress [bootstrap].
  void cancelBootstrap(Pointer<Void> client) =>
      bindings.tor_client_cancel_bootstrap(client);

  void setDormant(Pointer<Void> client, bool softMode) =>
      bindings.tor_client_set_dormant(client, softMode);

  void startProxy(Pointer<Void> proxy) {
    if (!bindings.tor_proxy_start(proxy)) {
      Tor.throwRustException();
    }
  }

  /// Stop [proxy] in a worker isolate. The native call waits for the accept
  /// loop to exit, so its listeners and client reference are released when
  /// this completes.
  Future<void> stopProxy(Pointer<Void> proxy) async {
    if (proxy == nullptr) {
      return;
    }

    final address = proxy.address;
    await Isolate.run(() => _stopProxyInIsolate(address));
  }

  static void _stopProxyInIsolate(int proxy) =>
      bindings.tor_proxy_stop(Pointer<Void>.fromAddress(proxy));

  /// Release [client] in a worker isolate, since tearing down Tor can block
  /// for a few seconds.
  Future<void> freeClient(Pointer<Void> client) async {
    if (client == nullptr) {
      return;
    }

    final address = client.address;
    await Isolate.run(() => _freeInIsolate(address));
  }

  static void _freeInIsolate(int client) =>
      bindings.tor_client_free(Pointer<Void>.fromAddress(client));

  /// Print the native greeting to verify that the library is linked.
  void hello() => bindings.tor_hello();
}

class Tor {
  /// Private constructor for the Tor class.
  Tor._() : _native = const TorNative();

  /// Create an instance that is independent of [instance] and talks to
  /// [native] instead of the Rust library.
  @visibleForTesting
  Tor.withNative(TorNative native) : _native = native;

  final TorNative _native;

  /// Singleton instance of the Tor class.
  static final Tor instance = Tor._();

  /// The lifecycle state of the Tor service.
  ///
  /// This does not monitor SOCKS accept-loop liveness after startup.
  TorStatus get status => _status;
  TorStatus _status = TorStatus.off;

  /// Getter for the proxy port.
  ///
  /// Throws if Tor is not enabled or if the circuit is not established.
  ///
  /// Returns the proxy port if Tor is enabled and the circuit is established.
  ///
  /// This is the port that should be used for all requests.
  int get port {
    if (_proxyPort == null || _status == TorStatus.off) {
      throw Exception('Tor proxy is not running');
    }
    return _proxyPort!;
  }

  /// The proxy port.
  int? _proxyPort;

  /// Start the Tor service.
  ///
  /// This will start the Tor service and establish a Tor circuit if there
  /// already hasn't been one established.
  ///
  /// Throws an exception if the Tor service fails to start.
  ///
  /// Returns a Future that completes when the Tor service has started.
  ///
  /// [start] and [stop] run one at a time, in the order they were called, so
  /// a start issued after [stop] restarts Tor once that stop has finished.
  /// Concurrent starts with the same raw data directory and timeout share
  /// one attempt and see the same success or failure. Other starts queue
  /// behind it and reuse the running instance, or fail with a [StateError]
  /// for a different data directory, leaving that instance running.
  ///
  /// [torDataDirPath] is created and resolved asynchronously inside the queued
  /// start. Physical paths are compared with the running instance, so symbolic
  /// links and trailing separators can reuse it. A path that cannot be created
  /// or resolved fails with the file system error.
  Future<void> start({
    required String torDataDirPath,
    Duration bootstrapTimeout = const Duration(minutes: 2),
  }) {
    // The native deadline has millisecond resolution.
    if (bootstrapTimeout.inMilliseconds <= 0) {
      return Future<void>.error(
        ArgumentError.value(
          bootstrapTimeout,
          'bootstrapTimeout',
          'must be at least one millisecond',
        ),
      );
    }

    for (final pending in _pendingStarts) {
      if (pending.dataDirPath == torDataDirPath &&
          pending.bootstrapTimeout == bootstrapTimeout) {
        return pending.future;
      }
    }

    // Always queue, even when Tor looks running: a stop may be queued ahead of
    // this start, in which case the start must restart Tor rather than return
    // early and leave it off.
    final operation = _StartOperation(torDataDirPath, bootstrapTimeout);
    // _start runs later, so report starting now. A start cancelled before it
    // runs is reset to off by the stop that cancelled it.
    if (_status == TorStatus.off) {
      _status = TorStatus.starting;
    }
    operation.future = _enqueue(() => _start(operation)).whenComplete(() {
      _pendingStarts.remove(operation);
    });
    _pendingStarts.add(operation);
    return operation.future;
  }

  /// Queued starts that later calls can join until a [stop] is requested.
  final _pendingStarts = <_StartOperation>[];

  String? _activeDataDirPath;

  // Retained handles need teardown, even if status still reports on.
  bool _releasePending = false;

  /// The last cleanup failure observed while preserving an earlier start
  /// failure. This is cleared at the beginning of each start attempt.
  Object? get lastCleanupError => _lastCleanupError;

  /// Stack trace associated with [lastCleanupError].
  StackTrace? get lastCleanupStackTrace => _lastCleanupStackTrace;

  Object? _lastCleanupError;
  StackTrace? _lastCleanupStackTrace;

  Future<void> _start(_StartOperation operation) async {
    if (operation.cancelRequested) {
      // A stop was requested after this start was queued and runs next.
      throw const TorStartCancelled();
    }

    final wasRunning = _status == TorStatus.on;
    if (!wasRunning) {
      _status = TorStatus.starting;
    }
    final String dataDirPath;
    try {
      final directory = await Directory(operation.dataDirPath).absolute
          .create(recursive: true);
      dataDirPath = await directory.resolveSymbolicLinks();
    } catch (_) {
      if (!wasRunning && !_releasePending) {
        _status = TorStatus.off;
      }
      rethrow;
    }

    // A stop queued while the directory was resolving runs next. Returning
    // success here would report Tor as running just before that stop turns
    // it off.
    if (operation.cancelRequested) {
      throw const TorStartCancelled();
    }

    if (_status == TorStatus.on && !_releasePending) {
      if (_activeDataDirPath == dataDirPath) {
        return;
      }
      throw StateError(
        'Tor is already running with a different data directory.',
      );
    }

    _lastCleanupError = null;
    _lastCleanupStackTrace = null;
    try {
      if (_proxyPtr != nullptr || _clientPtr != nullptr) {
        await _stop();
      }
      _status = TorStatus.starting;

      // Set the state and cache directories.
      final stateDir = await Directory('$dataDirPath/tor_state').create();
      final cacheDir = await Directory('$dataDirPath/tor_cache').create();

      if (operation.cancelRequested) {
        throw const TorStartCancelled();
      }

      // Let the OS allocate the port while Rust binds the real listeners. This
      // avoids a probe-then-bind race and keeps the published port reserved.
      final tor = await _native.start(0, stateDir.path, cacheDir.path);

      // Retain the native handles before bootstrap so stop can cancel it.
      _clientPtr = tor.client;
      _proxyPtr = tor.proxy;
      operation.client = tor.client;

      if (operation.cancelRequested) {
        throw const TorStartCancelled();
      }

      // Bootstrap the Tor service.
      await _native.bootstrap(tor.client, operation.bootstrapTimeout);

      if (operation.cancelRequested) {
        throw const TorStartCancelled();
      }

      _native.startProxy(tor.proxy);

      // Set the proxy port and change status.
      _proxyPort = tor.port;
      _activeDataDirPath = dataDirPath;
      _status = TorStatus.on;
    } catch (error, stackTrace) {
      // Release anything the native start produced before bootstrap failed;
      // the next start would otherwise overwrite these handles.
      operation.client = nullptr;
      try {
        await _stop();
      } catch (cleanupError, cleanupStackTrace) {
        _lastCleanupError = cleanupError;
        _lastCleanupStackTrace = cleanupStackTrace;
        try {
          // Also expose secondary failures in normal application logs. A
          // custom zone's print handler must not mask the bootstrap error.
          // ignore: avoid_print
          print(
            'Tor cleanup failed after start failed: $cleanupError\n$cleanupStackTrace',
          );
        } catch (_) {
          // Preserve the original start exception even if logging fails.
        }
      }
      Error.throwWithStackTrace(error, stackTrace);
    }
  }

  /// Prevent traffic flowing through the proxy.
  ///
  /// This is the same as [stop]: the SOCKS proxy is shut down and the native
  /// client released. Await the returned future and handle release failures,
  /// including when calling from a synchronous dispose method.
  Future<void> disable() => stop();

  /// Stop the proxy and release the native client handle.
  ///
  /// Cancels an in-progress [start], which throws [TorStartCancelled], then
  /// releases its handles. A running instance stays [TorStatus.on] until its
  /// native resources have been released, including when release fails and
  /// needs retry. A failed start stays [TorStatus.starting] if cleanup fails.
  /// Await a successful [stop] before deleting the data directory.
  Future<void> stop() {
    for (final pending in _pendingStarts) {
      pending.cancelRequested = true;
      if (pending.client != nullptr) {
        _native.cancelBootstrap(pending.client);
      }
    }
    _pendingStarts.clear();
    return _enqueue(_stop);
  }

  Future<void> _stop() async {
    // Retry retained handles on the next start or stop. Never shut down the
    // client's runtime while its proxy may still be accepting connections.
    _releasePending = true;
    _proxyPort = null;
    _activeDataDirPath = null;
    try {
      if (_proxyPtr != nullptr) {
        await _native.stopProxy(_proxyPtr);
        _proxyPtr = nullptr;
      }
      if (_clientPtr != nullptr) {
        await _native.freeClient(_clientPtr);
        _clientPtr = nullptr;
      }
    } finally {
      _releasePending = _proxyPtr != nullptr || _clientPtr != nullptr;
      if (!_releasePending) {
        _status = TorStatus.off;
      }
    }
  }

  /// Tail of the queue that runs [start] and [stop] one at a time.
  Future<void> _lifecycle = Future<void>.value();

  Future<void> _enqueue(Future<void> Function() operation) {
    final result = _lifecycle.then((_) => operation());
    // A failed operation must not stall the ones queued behind it.
    _lifecycle = result.catchError((Object _) {});
    return result;
  }

  Future<void> setClientDormant(bool dormant) async {
    if (_releasePending || _clientPtr == nullptr || status == TorStatus.off) {
      throw ClientNotActive();
    }

    _native.setDormant(_clientPtr, dormant);
  }

  Pointer<Void> _clientPtr = nullptr;
  Pointer<Void> _proxyPtr = nullptr;

  // Future<void> restart() async {
  //   // TODO: arti seems to recover by itself and there is no client restart fn
  //   // TODO: but follow up with them if restart is truly unnecessary
  //   // if (enabled && started && circuitEstablished) {}
  // }

  static Never throwRustException() {
    throw _getRustException(_takeRustError());
  }

  /// The calling thread's last Rust error message, which this clears.
  static String _takeRustError() {
    final message = bindings.tor_last_error_message();
    try {
      return message.cast<Utf8>().toDartString();
    } finally {
      bindings.tor_string_free(message);
    }
  }

  static Exception _getRustException(String rustError) {
    if (rustError.contains('Unable to bootstrap a working directory')) {
      return CouldntBootstrapDirectory(rustError: rustError);
    } else {
      return Exception(rustError);
    }
  }

  void hello() => _native.hello();
}

class ClientNotActive implements Exception {}

class _StartOperation {
  _StartOperation(this.dataDirPath, this.bootstrapTimeout);

  final String dataDirPath;
  final Duration bootstrapTimeout;
  late final Future<void> future;
  bool cancelRequested = false;
  Pointer<Void> client = nullptr;
}
