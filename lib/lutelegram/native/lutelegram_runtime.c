#include <curl/curl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct buffer {
  char *data;
  size_t length;
};

static _Thread_local struct buffer result;

static const char *find_bytes(const char *haystack, size_t haystack_len,
                              const char *needle, size_t needle_len) {
  if (!needle_len || needle_len > haystack_len) return NULL;
  for (size_t i = 0; i <= haystack_len - needle_len; i++)
    if (!memcmp(haystack + i, needle, needle_len)) return haystack + i;
  return NULL;
}

static size_t append(void *data, size_t size, size_t count, void *user) {
  struct buffer *buffer = user;
  size_t amount = size * count;
  char *next = realloc(buffer->data, buffer->length + amount + 1);
  if (!next) return 0;
  buffer->data = next;
  memcpy(buffer->data + buffer->length, data, amount);
  buffer->length += amount;
  buffer->data[buffer->length] = '\0';
  return amount;
}

static const char *failure(const char *message, int64_t *out_len) {
  free(result.data);
  size_t needed = strlen(message) + 80;
  result.data = malloc(needed);
  if (!result.data) abort();
  result.length = (size_t)snprintf(
      result.data, needed,
      "{\"ok\":false,\"error_code\":-1,\"description\":\"transport error: %s\"}",
      message);
  *out_len = (int64_t)result.length;
  return result.data;
}

const char *lutelegram_request(const char *token, int64_t token_len,
                               const char *request, int64_t request_len,
                               int64_t *out_len) {
  static int initialized;
  if (!initialized) {
    if (curl_global_init(CURL_GLOBAL_DEFAULT) != CURLE_OK)
      return failure("curl initialization failed", out_len);
    initialized = 1;
  }
  if (!token || token_len <= 0 || !request || request_len <= 0)
    return failure("invalid request", out_len);

  const char marker[] = "{\"method\":\"";
  if ((size_t)request_len < sizeof(marker) || memcmp(request, marker, sizeof(marker) - 1))
    return failure("invalid request envelope", out_len);
  const char *method = request + sizeof(marker) - 1;
  const char *request_end = request + request_len;
  const char *method_end = memchr(method, '"', (size_t)(request_end - method));
  const char params_marker[] = ",\"params\":";
  const char *params =
      method_end
          ? find_bytes(method_end, (size_t)(request_end - method_end),
                       params_marker, sizeof(params_marker) - 1)
          : NULL;
  if (!method_end || !params) return failure("invalid request envelope", out_len);
  params += sizeof(params_marker) - 1;
  size_t params_len = (size_t)(request + request_len - params);
  if (!params_len || params[params_len - 1] != '}')
    return failure("invalid request parameters", out_len);
  params_len--;

  size_t method_len = (size_t)(method_end - method);
  size_t url_len = strlen("https://api.telegram.org/bot//") + (size_t)token_len + method_len;
  char *url = malloc(url_len + 1);
  char *body = malloc(params_len + 1);
  if (!url || !body) abort();
  snprintf(url, url_len + 1, "https://api.telegram.org/bot%.*s/%.*s",
           (int)token_len, token, (int)method_len, method);
  memcpy(body, params, params_len);
  body[params_len] = '\0';

  free(result.data);
  result.data = NULL;
  result.length = 0;
  CURL *curl = curl_easy_init();
  if (!curl) {
    free(url);
    free(body);
    return failure("curl handle allocation failed", out_len);
  }
  struct curl_slist *headers = NULL;
  headers = curl_slist_append(headers, "Content-Type: application/json");
  curl_easy_setopt(curl, CURLOPT_URL, url);
  curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
  curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body);
  curl_easy_setopt(curl, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)params_len);
  curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, append);
  curl_easy_setopt(curl, CURLOPT_WRITEDATA, &result);
  curl_easy_setopt(curl, CURLOPT_USERAGENT, "lutelegram/0.1");
  curl_easy_setopt(curl, CURLOPT_TIMEOUT, 70L);
  CURLcode status = curl_easy_perform(curl);
  const char *error = status == CURLE_OK ? NULL : curl_easy_strerror(status);
  curl_slist_free_all(headers);
  curl_easy_cleanup(curl);
  free(url);
  free(body);
  if (error) return failure(error, out_len);
  if (!result.data) return failure("empty response", out_len);
  *out_len = (int64_t)result.length;
  return result.data;
}
