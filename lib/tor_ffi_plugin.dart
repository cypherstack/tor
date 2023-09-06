// SPDX-FileCopyrightText: 2022 Foundation Devices Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'package:ffi/ffi.dart';
import 'package:path_provider/path_provider.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin_bindings_generated.dart';

DynamicLibrary load(name) {
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

class Tor {
  static const String _libName = "tor_ffi_plugin";
  static late DynamicLibrary _lib;

  Pointer<Int> _clientPtr = nullptr;

  bool get enabled => _enabled;
  bool _enabled = true;

  bool get started => _started;
  bool _started = false;

  bool get bootstrapped => _bootstrapped;
  bool _bootstrapped = false;

  // This stream broadcast just the port for now (-1 if circuit not established)
  final StreamController events = StreamController.broadcast();

  int get port {
    if (!_enabled) {
      return -1;
    }
    return _proxyPort;
  }

  int _proxyPort = -1;
  static final Tor _instance = Tor._internal();

  static Tor get instance => _instance;

  // TODO: is this function supposed to await anything?
  // in its current state it does not need to be a future as it only
  // sets the `enabled` bool flag
  static Future<Tor> init({enabled = true}) async {
    var singleton = Tor._instance;
    singleton._enabled = enabled;
    return singleton;
  }

  Tor._internal() {
    _lib = load(_libName);
    print("Instance of Tor created!");
  }

  Future<void> enable() async {
    _enabled = true;
    if (!started) {
      await start();
    }
  }

  Future<int> _getRandomUnusedPort({List<int> excluded = const []}) async {
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

    return -1;
  }

  Future<void> start() async {
    events.add(port);

    final Directory appSupportDir = await getApplicationSupportDirectory();
    final stateDir =
        await Directory('${appSupportDir.path}/tor_state').create();
    final cacheDir =
        await Directory('${appSupportDir.path}/tor_cache').create();

    int newPort = await _getRandomUnusedPort();
    int ptr = await Isolate.run(() async {
      var lib = NativeLibrary(load(_libName));
      final ptr = lib.tor_start(
          newPort,
          stateDir.path.toNativeUtf8() as Pointer<Char>,
          cacheDir.path.toNativeUtf8() as Pointer<Char>);

      if (ptr == nullptr) {
        throwRustException(lib);
      }

      return ptr.address;
    });

    _clientPtr = Pointer.fromAddress(ptr);
    _started = true;
    bootstrap();
    _proxyPort = newPort;
  }

  void bootstrap() {
    final lib = NativeLibrary(_lib);
    _bootstrapped = lib.tor_bootstrap(_clientPtr);
    if (!bootstrapped) {
      throwRustException(lib);
    }
  }

  void disable() {
    _enabled = false;
  }

  void restart() {
    // TODO: arti seems to recover by itself and there is no client restart fn
    // TODO: but follow up with them if restart is truly unnecessary
    // if (enabled && started && circuitEstablished) {}
  }

  Future<void> isReady() async {
    return await Future.doWhile(
        () => Future.delayed(const Duration(seconds: 1)).then((_) {
              // We are waiting and making absolutely no request unless:
              // Tor is disabled
              if (!enabled) {
                return false;
              }

              // ...or Tor circuit is established
              if (bootstrapped) {
                return false;
              }

              // This way we avoid making clearnet req's while Tor is initialising
              return true;
            }));
  }

  static throwRustException(NativeLibrary lib) {
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
    NativeLibrary(_lib).tor_hello();
  }
}