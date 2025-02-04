// SPDX-FileCopyrightText: 2022 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin_bindings_generated.dart';

DynamicLibrary _load(name) {
  if (Platform.isAndroid || Platform.isLinux) {
    return DynamicLibrary.open('lib$name.so');
  } else if (Platform.isIOS || Platform.isMacOS) {
    return DynamicLibrary.open('$name.framework/$name');
  } else if (Platform.isWindows) {
    return DynamicLibrary.open('$name.dll');
  } else {
    throw NotSupportedPlatform('${Platform.operatingSystem} is not supported!');
  }
}

class CouldntBootstrapDirectory implements Exception {
  String? rustError;

  CouldntBootstrapDirectory({this.rustError});
}

class NotSupportedPlatform implements Exception {
  NotSupportedPlatform(String s);
}

enum TorStatus {
  on,
  starting,
  off;
}

class Tor {
  /// Private constructor for the Tor class.
  Tor._() {
    _lib = _load(_libName);
    if (kDebugMode) {
      print("Native tor library loaded!");
    }
  }

  /// Singleton instance of the Tor class.
  static final Tor instance = Tor._();

  static const String _libName = "tor_ffi_plugin";
  static late DynamicLibrary _lib;

  /// Status of the tor proxy service
  TorStatus get status => _status;
  TorStatus _status = TorStatus.off;

  /// Flag to indicate that a Tor circuit is thought to have been established
  /// (true means that Tor has bootstrapped).
  bool _bootstrapped = false;

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
  Future<void> start({required String torDataDirPath}) async {
    if (_status != TorStatus.off) {
      // already starting or running
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
      final tor = await Isolate.run(() async {
        // Load the Tor library.
        var lib = TorFfiPluginBindings(_load(_libName));

        // Start the Tor service.
        final ptr = lib.tor_start(
            newPort,
            stateDir.path.toNativeUtf8() as Pointer<Char>,
            cacheDir.path.toNativeUtf8() as Pointer<Char>);

        // Throw an exception if the Tor service fails to start.
        if (ptr == nullptr) {
          throwRustException(lib);
        }

        // Return the pointer.
        return ptr;
      });

      // Set the client pointer and started flag.
      _clientPtr = Pointer.fromAddress(tor.client.address);
      _proxyPtr = Pointer.fromAddress(tor.proxy.address);

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
  void _bootstrap() {
    // Load the Tor library.
    final lib = TorFfiPluginBindings(_lib);

    // Bootstrap the Tor service.
    _bootstrapped = lib.tor_client_bootstrap(_clientPtr);

    // Throw an exception if the Tor service fails to bootstrap.
    if (!_bootstrapped) {
      throwRustException(lib);
    }
  }

  /// Prevent traffic flowing through the proxy
  void disable() {
    _status = TorStatus.off;
  }

  /// Stop the proxy.
  stop() async {
    final lib = TorFfiPluginBindings(_lib);
    lib.tor_proxy_stop(_proxyPtr);
    _proxyPtr = nullptr;
  }

  setClientDormant(bool dormant) async {
    if (_clientPtr == nullptr || status == TorStatus.off) {
      throw ClientNotActive();
    }

    final lib = TorFfiPluginBindings(_lib);
    lib.tor_client_set_dormant(_clientPtr, dormant);
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

  static void throwRustException(TorFfiPluginBindings lib) {
    String rustError = lib.tor_last_error_message().cast<Utf8>().toDartString();

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
    TorFfiPluginBindings(_lib).tor_hello();
  }
}

class ClientNotActive implements Exception {}
