#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct Tor {
  void *client;
  void *proxy;
} Tor;

struct Tor tor_start(uint16_t socks_port, const char *state_dir, const char *cache_dir);

bool tor_client_bootstrap(void *client);

void tor_client_set_dormant(void *client, bool soft_mode);

/**
 * Release the client handle returned by [`tor_start`].
 *
 * The handle must not be used after this call. The proxy task keeps its own
 * clone of the underlying client, so stopping the proxy and freeing the
 * handle can happen in either order.
 */
void tor_client_free(void *client);

void tor_proxy_stop(void *proxy);

void tor_hello(void);

const char *tor_last_error_message(void);

uint64_t tor_get_nofile_limit(void);

uint64_t tor_set_nofile_limit(uint64_t limit);
