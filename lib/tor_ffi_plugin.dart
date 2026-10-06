// SPDX-FileCopyrightText: 2022 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'package:ffi/ffi.dart';
import 'package:meta/meta.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin_bindings_generated.dart'
    as bindings;

class CouldntBootstrapDirectory implements Exception {
  String? rustError;

  CouldntBootstrapDirectory({this.rustError});
}

enum TorStatus { on, starting, off }

/// Native handles produced by a successful [TorNative.start].
typedef TorHandles = ({Pointer<Void> client, Pointer<Void> proxy});

/// The native calls [Tor] makes. Tests substitute a fake to drive the
/// lifecycle without starting Tor.
@visibleForTesting
class TorNative {
  const TorNative();

  /// Start Tor with its SOCKS proxy on [port], in a worker isolate.
  ///
  /// Throws the Rust error if the native start fails.
  Future<TorHandles> start(int port, String stateDir, String cacheDir) async {
    final (client, proxy) = await Isolate.run(
      () => _startInIsolate(port, stateDir, cacheDir),
    );
    return (
      client: Pointer<Void>.fromAddress(client),
      proxy: Pointer<Void>.fromAddress(proxy),
    );
  }

  static (int, int) _startInIsolate(
    int port,
    String stateDir,
    String cacheDir,
  ) {
    final tor = bindings.tor_start(
      port,
      stateDir.toNativeUtf8() as Pointer<Char>,
      cacheDir.toNativeUtf8() as Pointer<Char>,
    );

    // Throw an exception if the Tor service fails to start.
    if (tor.client == nullptr) {
      Tor.throwRustException();
    }

    return (tor.client.address, tor.proxy.address);
  }

  /// Bootstrap [client]. Throws the Rust error on failure.
  void bootstrap(Pointer<Void> client) {
    if (!bindings.tor_client_bootstrap(client)) {
      Tor.throwRustException();
    }
  }

  void setDormant(Pointer<Void> client, bool softMode) =>
      bindings.tor_client_set_dormant(client, softMode);

  void stopProxy(Pointer<Void> proxy) => bindings.tor_proxy_stop(proxy);

  void freeClient(Pointer<Void> client) => bindings.tor_client_free(client);
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

  /// Status of the tor proxy service
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
      throw Exception("Tor proxy port is unexpectedly null");
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
  /// [start] and [stop] run one at a time, in the order they were called.
  /// Calling [start] while an earlier start is still pending returns that
  /// start's future, so every caller sees the same success or failure.
  Future<void> start({required String torDataDirPath}) {
    final pending = _pendingStart;
    if (pending != null) {
      return pending;
    }

    late final Future<void> attempt;
    attempt = _enqueue(() => _start(torDataDirPath)).whenComplete(() {
      if (identical(_pendingStart, attempt)) {
        _pendingStart = null;
      }
    });
    return _pendingStart = attempt;
  }

  /// The start that later [start] calls join, until a [stop] is requested.
  Future<void>? _pendingStart;

  Future<void> _start(String torDataDirPath) async {
    if (_status == TorStatus.on) {
      return;
    }

    try {
      _status = TorStatus.starting;

      // Set the state and cache directories.
      final stateDir = await Directory('$torDataDirPath/tor_state').create();
      final cacheDir = await Directory('$torDataDirPath/tor_cache').create();

      // Generate a random port.
      final int? newPort = await _getRandomUnusedPort();

      if (newPort == null) {
        throw Exception("Failed to get random unused port!");
      }

      // Start the Tor service in an isolate.
      final tor = await _native.start(newPort, stateDir.path, cacheDir.path);

      // Set the client pointer and started flag.
      _clientPtr = tor.client;
      _proxyPtr = tor.proxy;

      // Bootstrap the Tor service.
      _bootstrap();

      // Set the proxy port and change status.
      _proxyPort = newPort;
      _status = TorStatus.on;
    } catch (_) {
      _status = TorStatus.off;
      rethrow;
    }
  }

  /// Bootstrap the Tor service.
  ///
  /// This will bootstrap the Tor service and establish a Tor circuit.  This
  /// function should only be called after the Tor service has been started.
  ///
  /// This function will block until the Tor service has bootstrapped.
  ///
  /// Throws an exception if the Tor service fails to bootstrap.
  ///
  /// Returns void.
  void _bootstrap() => _native.bootstrap(_clientPtr);

  /// Prevent traffic flowing through the proxy
  void disable() {
    _status = TorStatus.off;
  }

  /// Stop the proxy and release the native client handle.
  ///
  /// If a [start] is still in progress, this waits for it to finish and then
  /// tears down what it started, so no Tor instance outlives the call.
  Future<void> stop() {
    // A start requested after this stop must not join the earlier attempt.
    _pendingStart = null;
    return _enqueue(() async => _stop());
  }

  void _stop() {
    _native.stopProxy(_proxyPtr);
    _proxyPtr = nullptr;

    _native.freeClient(_clientPtr);
    _clientPtr = nullptr;

    _proxyPort = null;
    _status = TorStatus.off;
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
    if (_clientPtr == nullptr || status == TorStatus.off) {
      throw ClientNotActive();
    }

    _native.setDormant(_clientPtr, dormant);
  }

  Pointer<Void> _clientPtr = nullptr;
  Pointer<Void> _proxyPtr = nullptr;

  Future<int?> _getRandomUnusedPort({List<int> excluded = const []}) async {
    var random = Random.secure();
    int potentialPort = 0;

    retry:
    while (potentialPort <= 0 || excluded.contains(potentialPort)) {
      potentialPort = random.nextInt(65535);
      try {
        var socket = await ServerSocket.bind("0.0.0.0", potentialPort);
        socket.close();
        return potentialPort;
      } catch (_) {
        continue retry;
      }
    }

    return null;
  }

  // Future<void> restart() async {
  //   // TODO: arti seems to recover by itself and there is no client restart fn
  //   // TODO: but follow up with them if restart is truly unnecessary
  //   // if (enabled && started && circuitEstablished) {}
  // }

  static void throwRustException() {
    String rustError = bindings
        .tor_last_error_message()
        .cast<Utf8>()
        .toDartString();

    throw _getRustException(rustError);
  }

  static Exception _getRustException(String rustError) {
    if (rustError.contains('Unable to bootstrap a working directory')) {
      return CouldntBootstrapDirectory(rustError: rustError);
    } else {
      return Exception(rustError);
    }
  }

  void hello() {
    bindings.tor_hello();
  }
}

class ClientNotActive implements Exception {}
