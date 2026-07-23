#ifndef EMBEDDED_SLERP_H
#define EMBEDDED_SLERP_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* export fn slerp_checksum(count: i64): f64 */
double slerp_checksum(int64_t count);

#ifdef __cplusplus
}
#endif

#endif
