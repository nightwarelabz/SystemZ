/* SystemZ boot graphics. Same renderer in UEFI, ZCore and the host proof.
 * No firmware calls, allocator, floating point, libc, or external code. */
#ifndef SYSTEMZ_SPLASH_H
#define SYSTEMZ_SPLASH_H
#include <stdint.h>

#define SZ_SPLASH_STEPS 9u
#define SZ_SPLASH_LOADING 0u
#define SZ_SPLASH_ERROR 1u

/* Pixel formats follow GOP: 0 = RGBX, 1 = BGRX, 2 = explicit masks.
 * This descriptor is a value, not a GOP protocol pointer. It remains
 * usable after ExitBootServices; the physical mapping must be preserved. */
typedef struct {
    uint64_t address;
    uint64_t size;
    uint32_t width, height, stride, format;
    uint32_t red_mask, green_mask, blue_mask, reserved;
} sz_framebuffer;

int sz_framebuffer_valid(const sz_framebuffer *fb);
void sz_splash_draw(const sz_framebuffer *fb, uint32_t completed,
                    uint32_t frame, const char *status, uint32_t state,
                    uint64_t error_code);
void sz_console_draw(const sz_framebuffer *fb, const char *cpu,
                     const char *guest, const char *result);
#endif
