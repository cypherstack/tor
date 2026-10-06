import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin.dart';

Future<void> expectSocksHandshake(int port) async {
  final socket = await Socket.connect(
    InternetAddress.loopbackIPv4,
    port,
    timeout: const Duration(seconds: 10),
  );
  try {
    socket.add([5, 1, 0]);
    final response = await socket
        .expand((chunk) => chunk)
        .take(2)
        .toList()
        .timeout(const Duration(seconds: 10));
    expect(response, [5, 0]);
  } finally {
    socket.destroy();
  }
}

void main() {
  test('stop is idempotent while off', () async {
    final tor = Tor.instance;
    await tor.stop();
    await tor.stop();
    expect(tor.status, TorStatus.off);
    expect(() => tor.port, throwsException);
    await expectLater(
      tor.setClientDormant(true),
      throwsA(isA<ClientNotActive>()),
    );
  });

  test(
    'stop resets state and allows a fresh start',
    () async {
      final directory = await Directory.systemTemp.createTemp('tor-lifecycle-');
      final tor = Tor.instance;
      try {
        for (var attempt = 0; attempt < 2; attempt++) {
          await tor.start(torDataDirPath: directory.path);
          expect(tor.status, TorStatus.on);
          await expectSocksHandshake(tor.port);
          await tor.setClientDormant(true);
          await tor.setClientDormant(false);
          await expectSocksHandshake(tor.port);
          await tor.stop();
          expect(tor.status, TorStatus.off);
          expect(() => tor.port, throwsException);
          await expectLater(
            tor.setClientDormant(true),
            throwsA(isA<ClientNotActive>()),
          );
          await tor.stop();
        }
      } finally {
        await tor.stop();
        await directory.delete(recursive: true);
      }
    },
    skip: !const bool.fromEnvironment('TOR_NETWORK_TESTS'),
    timeout: const Timeout(Duration(minutes: 5)),
  );
}
