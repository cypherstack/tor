import 'dart:io';
import 'dart:math';

import 'package:flutter_test/flutter_test.dart';
import 'package:tor_ffi_plugin/tor_ffi_plugin.dart';

/// Returns [values] from [nextInt] in order and records the bounds asked for.
class ScriptedRandom implements Random {
  ScriptedRandom(this.values);

  final List<int> values;
  final bounds = <int>[];
  int _next = 0;

  @override
  int nextInt(int max) {
    bounds.add(max);
    return values[_next++];
  }

  @override
  bool nextBool() => throw UnimplementedError();

  @override
  double nextDouble() => throw UnimplementedError();
}

Future<int> freePort() async {
  final socket = await ServerSocket.bind(InternetAddress.anyIPv4, 0);
  final port = socket.port;
  await socket.close();
  return port;
}

void main() {
  test('tries another port after a failed bind', () async {
    final occupied = await ServerSocket.bind(InternetAddress.anyIPv4, 0);
    addTearDown(occupied.close);
    final free = await freePort();

    final port = await Tor.pickUnusedPort(
      random: ScriptedRandom([occupied.port - 1024, free - 1024]),
    );

    expect(port, free);
  });

  test('gives up after the allowed attempts', () async {
    final occupied = await ServerSocket.bind(InternetAddress.anyIPv4, 0);
    addTearDown(occupied.close);

    final port = await Tor.pickUnusedPort(
      random: ScriptedRandom([occupied.port - 1024, occupied.port - 1024]),
      attempts: 2,
    );

    expect(port, isNull);
  });

  test('never picks port zero or a privileged port', () async {
    final random = ScriptedRandom([0]);

    final port = await Tor.pickUnusedPort(random: random, attempts: 1);

    expect(random.bounds, [65536 - 1024]);
    expect(port, anyOf(isNull, 1024));
  });
}
