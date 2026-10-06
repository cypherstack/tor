#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct Tor {
  void *client;
  void *proxy;
} Tor;

/**
 * Start a bootstrapped Tor client and a localhost SOCKS proxy.
 *
 * # Safety
 * `state_dir` and `cache_dir` must point to valid, NUL-terminated strings
 * for the duration of the call. Each returned non-null handle must be
 * released exactly once with its corresponding cleanup function.
 */
struct Tor tor_start(uint16_t socks_port, const char *state_dir, const char *cache_dir);

/**
 * Ensure the client has bootstrapped.
 *
 * # Safety
 * `client` must be a live, non-null handle returned by [`tor_start`].
 * It must not be freed while this call is in progress.
 */
bool tor_client_bootstrap(void *client);

/**
 * Change the client's dormant mode.
 *
 * # Safety
 * `client` must be a live, non-null handle returned by [`tor_start`].
 * It must not be freed while this call is in progress.
 */
void tor_client_set_dormant(void *client, bool soft_mode);

/**
 * Release the client handle returned by [`tor_start`].
 *
 * The handle must not be used after this call. The proxy task keeps its own
 * reference to the underlying client, so stopping the proxy and freeing the
 * handle can happen in either order.
 *
 * # Safety
 * `client` must be null or a live handle returned by [`tor_start`].
 * No other call may use the handle concurrently with or after this call.
 */
void tor_client_free(void *client);

/**
 * Stop the proxy and release its handle. Null is accepted.
 *
 * # Safety
 * `proxy` must be null or a live proxy handle returned by [`tor_start`].
 * A non-null handle must be passed to this function only once.
 */
void tor_proxy_stop(void *proxy);

/**
 * Print a greeting to verify that the library is linked.
 *
 * # Safety
 * This function has no additional safety requirements.
 */
void tor_hello(void);

/**
 * Take the current thread's last error as a newly allocated C string.
 *
 * # Safety
 * This function has no preconditions. The returned string is owned by the
 * caller and must be released exactly once with [`tor_string_free`].
 */
const char *tor_last_error_message(void);

/**
 * Release a string returned by [`tor_last_error_message`].
 *
 * # Safety
 * `message` must be null or a pointer returned by `tor_last_error_message`
 * that has not been released yet. It must not be used after this call.
 */
void tor_string_free(char *message);

/**
 * Read the current open-file limit.
 *
 * # Safety
 * This function has no additional safety requirements.
 */
uint64_t tor_get_nofile_limit(void);

/**
 * Increase the open-file limit, up to the hard limit.
 *
 * # Safety
 * This function has no additional safety requirements.
 */
uint64_t tor_set_nofile_limit(uint64_t limit);
