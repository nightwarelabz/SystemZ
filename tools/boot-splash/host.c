/* Exercise the real freestanding renderer in guarded host memory. The
 * optional PPM frames are a labelled visual proof, not an OS emulator. */
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../../boot/splash.h"
#include "../../loader/efi.h"
#include "../../loader/zboot.h"

enum { GUARD = 64 };
static const uint32_t sentinel = 0xdeadbeef;

static uint64_t hash(const uint32_t *p, size_t n) {
    uint64_t h = 1469598103934665603ull;
    for (size_t i = 0; i < n; i++) h = (h ^ p[i]) * 1099511628211ull;
    return h;
}

static void ppm(const char *path, const sz_framebuffer *fb) {
    FILE *f = fopen(path, "wb"); assert(f);
    fprintf(f, "P6\n%u %u\n255\n", fb->width, fb->height);
    const uint32_t *pixels = (const uint32_t *)(uintptr_t)fb->address;
    for (uint32_t y = 0; y < fb->height; y++)
        for (uint32_t x = 0; x < fb->width; x++) {
            uint32_t p = pixels[(size_t)y * fb->stride + x];
            fputc((p >> 16) & 255, f); fputc((p >> 8) & 255, f); fputc(p & 255, f);
        }
    fclose(f);
}

static void test_mode(uint32_t w, uint32_t h, uint32_t format) {
    size_t stride = w + 13, count = stride * h;
    uint32_t *allocation = malloc((count + 2 * GUARD) * sizeof(uint32_t));
    assert(allocation);
    for (size_t i = 0; i < count + 2 * GUARD; i++) allocation[i] = sentinel;
    uint32_t *pixels = allocation + GUARD;
    sz_framebuffer fb = {(uint64_t)(uintptr_t)pixels, count * 4, w, h,
                         (uint32_t)stride, format, 0xff0000, 0xff00, 0xff, 0};
    assert(sz_framebuffer_valid(&fb));
    sz_splash_draw(&fb, 0, 0, "OPENING ZCORE", 0, 0);
    uint64_t initial = hash(pixels, count);
    sz_splash_draw(&fb, 3, 15, "READING ZCORE", 0, 0);
    assert(hash(pixels, count) != initial); /* real frame/state updates */
    uint32_t scale = w / 640 < h / 360 ? w / 640 : h / 360;
    uint32_t left = (w - 640 * scale) / 2, top = (h - 360 * scale) / 2;
    size_t filled = (size_t)(top + 290 * scale) * stride + left + 220 * scale;
    size_t empty = filled + 5 * 23 * scale;
    assert(pixels[filled] == (format == 0 ? 0x8ab891u : 0x91b88au));
    assert(pixels[empty] == (format == 0 ? 0x131a12u : 0x121a13u));
    sz_splash_draw(&fb, 3, 15, "MISSING ZCORE", 1, 0x800000000000000eull);
    assert(pixels[filled] == (format == 0 ? 0x6979d8u : 0xd87969u));
    sz_console_draw(&fb, "CPU: TEST WITNESS", "GUEST: NOT CLAIMED", "HOST RENDERER PROOF ONLY");
    for (size_t i = 0; i < GUARD; i++) {
        assert(allocation[i] == sentinel);
        assert(allocation[GUARD + count + i] == sentinel);
    }
    for (uint32_t y = 0; y < h; y++)
        for (size_t x = w; x < stride; x++) assert(pixels[y * stride + x] == sentinel);
    uint64_t before = hash(pixels, count);
    fb.size = count * 4 - 1;
    assert(!sz_framebuffer_valid(&fb));
    sz_splash_draw(&fb, 9, 0, "MUST NOT WRITE", 0, 0);
    assert(hash(pixels, count) == before);
    fb.size++; fb.format = 3; assert(!sz_framebuffer_valid(&fb));
    fb.format = 2; fb.green_mask = fb.red_mask; assert(!sz_framebuffer_valid(&fb));
    fb.green_mask = 0x07e0; assert(!sz_framebuffer_valid(&fb));
    fb.green_mask = 0xff00; fb.stride = w - 1; assert(!sz_framebuffer_valid(&fb));
    fb.stride = (uint32_t)stride; fb.address = UINT64_MAX - 3;
    assert(!sz_framebuffer_valid(&fb));
    free(allocation);
}

int main(int argc, char **argv) {
    assert(!sz_framebuffer_valid(NULL));
    sz_splash_draw(NULL, 0, 0, NULL, 0, 0);
    uint32_t modes[][2] = {{640,360},{800,600},{1024,768},{1280,720},{1920,1080}};
    for (size_t i = 0; i < sizeof(modes) / sizeof(modes[0]); i++)
        for (uint32_t format = 0; format < 3; format++) test_mode(modes[i][0], modes[i][1], format);
    puts("PASS: RGBX/BGRX/masks, 5 resolutions, stride guards, animation, progress, errors, diagnostics, invalid framebuffer fallback");
    if (argc == 2) {
        uint32_t *pixels = calloc(1280 * 720, sizeof(uint32_t)); assert(pixels);
        sz_framebuffer fb = {(uint64_t)(uintptr_t)pixels,1280*720*4,1280,720,1280,1,0,0,0,0};
        for (uint32_t frame = 0; frame < 96; frame++) {
            char path[512];
            snprintf(path, sizeof(path), "%s/frame-%03u.ppm", argv[1], frame);
            if (frame < 78)
                sz_splash_draw(&fb, frame / 9, frame, "RENDERER PREVIEW - LIVE STATUS IS PROVIDED BY BOOT", 0, 0);
            else
                sz_console_draw(&fb, "CPU: HOST RENDERER PREVIEW", "GUEST: NO EXECUTION IN THIS PREVIEW", "REAL HANDOFF IS TESTED WITH QEMU / OVMF");
            ppm(path, &fb);
        }
        free(pixels);
    }
}
