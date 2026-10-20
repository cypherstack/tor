#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * The outcome of [`tor_client_bootstrap`].
 *
 * Every code other than `Ok` also sets the message returned by
 * [`tor_last_error_message`].
 */
enum TorBootstrapStatus
#ifdef __cplusplus
  : int32_t
#endif // __cplusplus
 {
  /**
   * The client bootstrapped.
   */
  TorBootstrapStatus_Ok = 0,
  /**
   * [`tor_client_cancel_bootstrap`] was called on the handle.
   */
  TorBootstrapStatus_Cancelled = 1,
  /**
   * The deadline passed before bootstrap finished.
   */
  TorBootstrapStatus_TimedOut = 2,
  /**
   * Arti reported an error.
   */
  TorBootstrapStatus_Failed = 3,
  /**
   * The client handle was null.
   */
  TorBootstrapStatus_NullClient = 4,
};
#ifndef __cplusplus
typedef int32_t TorBootstrapStatus;
#endif // __cplusplus

typedef struct Tor {
  void *client;
  void *proxy;
  uint16_t port;
} Tor;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Create a Tor client and a localhost SOCKS proxy.
 *
 * # Safety
 * `state_dir` and `cache_dir` must point to valid, NUL-terminated strings
 * for the duration of the call. Each returned non-null handle must be
 * released exactly once with its corresponding cleanup function.
 */
struct Tor tor_start(uint16_t socks_port, const char *state_dir, const char *cache_dir);

/**
 * Bootstrap the client, returning on cancellation or after `timeout_millis`.
 * A null `client` returns `NullClient`.
 *
 * # Safety
 * `client` must be null or a live handle returned by [`tor_start`].
 * It must not be freed while this call is in progress.
 */
TorBootstrapStatus tor_client_bootstrap(void *client, uint64_t timeout_millis);

/**
 * Ask an in-progress bootstrap to return promptly. Null is accepted.
 *
 * Cancellation is sticky: once this is called, every later
 * [`tor_client_bootstrap`] on the same handle returns `Cancelled` at once.
 * To bootstrap again, free the handle with [`tor_client_free`] and create a
 * new one with [`tor_start`].
 *
 * # Safety
 * `client` must be null or a live handle returned by [`tor_start`]. It may be
 * called concurrently with [`tor_client_bootstrap`], but not with or after
 * [`tor_client_free`].
 */
void tor_client_cancel_bootstrap(void *client);

/**
 * Change the client's dormant mode. A null `client` is ignored and sets the
 * message returned by [`tor_last_error_message`].
 *
 * # Safety
 * `client` must be null or a live handle returned by [`tor_start`].
 * It must not be freed while this call is in progress.
 */
void tor_client_set_dormant(void *client, bool soft_mode);

/**
 * Release the client handle returned by [`tor_start`].
 *
 * This shuts down the client's background tasks and closes its state and
 * cache files, which can block for a few seconds. Stop the proxy first: it
 * keeps its own reference to the client, but cannot serve connections once
 * the client is freed.
 *
 * # Safety
 * `client` must be null or a live handle returned by [`tor_start`].
 * No other call may use the handle concurrently with or after this call.
 * It must not be called from within an async runtime.
 */
void tor_client_free(void *client);

/**
 * Start accepting SOCKS connections on the listeners reserved by [`tor_start`].
 * Repeated calls are harmless.
 *
 * # Safety
 * `proxy` must be a live, non-null proxy handle returned by [`tor_start`]. It
 * must not be used concurrently or after [`tor_proxy_stop`].
 */
bool tor_proxy_start(void *proxy);

/**
 * Stop the proxy, or release its reserved listeners if it was not started.
 * Null is accepted.
 *
 * This waits for the accept loop to exit, so the listeners and the loop's
 * reference to the client are released by the time it returns. Call it
 * before [`tor_client_free`].
 *
 * # Safety
 * `proxy` must be null or a live proxy handle returned by [`tor_start`].
 * A non-null handle must be passed to this function only once.
 * It must not be called from within an async runtime.
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

#ifdef __cplusplus
} // extern "C"
#endif // __cplusplus
