import 'dart:async';
import 'package:socks_socket/socks_socket.dart';

// Instantiate a socks socket at localhost and on the port selected by the
// tor service.
var socksSocket = await SOCKSSocket.create(
proxyHost: InternetAddress.loopbackIPv4.address,
proxyPort: tor.port,
// sslEnabled: true, // For SSL connections.
);

// Connect to the socks instantiated above.
await socksSocket.connect();

// Connect to bitcoincash.stackwallet.com on port 50001 via socks socket.
await socksSocket.connectTo('bitcoincash.stackwallet.com', 50001);

// Send a server features command to the connected socket, see method for
// more specific usage example..
await socksSocket.sendServerFeaturesCommand();
await socksSocket.close();
