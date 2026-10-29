#include "splash.h"
#include <stddef.h>
#include "splash_art.inc"

_Static_assert(sizeof(sz_framebuffer) == 48, "boot framebuffer ABI");

/* House 5x7 bitmap alphabet. Rows run left to right, bit 4 first. */
typedef struct { char ch; uint8_t row[7]; } glyph;
static const glyph font[] = {
    {' ',{0,0,0,0,0,0,0}}, {'A',{14,17,17,31,17,17,17}},
    {'B',{30,17,17,30,17,17,30}}, {'C',{14,17,16,16,16,17,14}},
    {'D',{30,17,17,17,17,17,30}}, {'E',{31,16,16,30,16,16,31}},
    {'F',{31,16,16,30,16,16,16}}, {'G',{14,17,16,23,17,17,15}},
    {'H',{17,17,17,31,17,17,17}}, {'I',{14,4,4,4,4,4,14}},
    {'J',{7,2,2,2,2,18,12}}, {'K',{17,18,20,24,20,18,17}},
    {'L',{16,16,16,16,16,16,31}}, {'M',{17,27,21,21,17,17,17}},
    {'N',{17,25,21,19,17,17,17}}, {'O',{14,17,17,17,17,17,14}},
    {'P',{30,17,17,30,16,16,16}}, {'Q',{14,17,17,17,21,18,13}},
    {'R',{30,17,17,30,20,18,17}}, {'S',{15,16,16,14,1,1,30}},
    {'T',{31,4,4,4,4,4,4}}, {'U',{17,17,17,17,17,17,14}},
    {'V',{17,17,17,17,17,10,4}}, {'W',{17,17,17,21,21,21,10}},
    {'X',{17,17,10,4,10,17,17}}, {'Y',{17,17,10,4,4,4,4}},
    {'Z',{31,1,2,4,8,16,31}}, {'0',{14,17,19,21,25,17,14}},
    {'1',{4,12,4,4,4,4,14}}, {'2',{14,17,1,2,4,8,31}},
    {'3',{30,1,1,14,1,1,30}}, {'4',{2,6,10,18,31,2,2}},
    {'5',{31,16,16,30,1,1,30}}, {'6',{14,16,16,30,17,17,14}},
    {'7',{31,1,2,4,8,8,8}}, {'8',{14,17,17,14,17,17,14}},
    {'9',{14,17,17,15,1,1,14}}, {'.',{0,0,0,0,0,12,12}},
    {':',{0,12,12,0,12,12,0}}, {'-',{0,0,0,31,0,0,0}},
    {'/',{1,2,2,4,8,8,16}}, {'?',{14,17,1,2,4,0,4}},
    {'x',{0,0,17,10,4,10,17}}, {'y',{0,17,17,15,1,17,14}},
    {'s',{0,0,15,16,14,1,30}}, {'t',{8,8,30,8,8,9,6}},
    {'e',{0,0,14,17,31,16,14}}, {'m',{0,0,26,21,21,21,21}},
};

static int byte_mask(uint32_t mask) {
    return mask == 0xffu || mask == 0xff00u ||
           mask == 0xff0000u || mask == 0xff000000u;
}

int sz_framebuffer_valid(const sz_framebuffer *fb) {
    if (!fb || !fb->address || (fb->address & 3u) ||
        fb->width < 640 || fb->height < 360 ||
        fb->width > 16384 || fb->height > 16384 ||
        fb->stride < fb->width || fb->stride > 65536) return 0;
    uint64_t needed = (uint64_t)fb->stride * fb->height * 4u;
    if (fb->size < needed || fb->address > UINT64_MAX - needed) return 0;
    if (fb->format < 2) return 1;
    return fb->format == 2 && byte_mask(fb->red_mask) &&
        byte_mask(fb->green_mask) && byte_mask(fb->blue_mask) &&
        !(fb->red_mask & fb->green_mask) &&
        !(fb->red_mask & fb->blue_mask) &&
        !(fb->green_mask & fb->blue_mask);
}

typedef struct {
    const sz_framebuffer *fb;
    uint32_t scale, left, top;
    uint32_t rs, gs, bs;
} canvas;

static uint32_t shift(uint32_t mask) {
    uint32_t s = 0;
    while (s < 24 && !(mask & 255u)) { mask >>= 8; s += 8; }
    return s;
}

static uint32_t color(const canvas *c, uint32_t rgb) {
    return ((rgb >> 16 & 255u) << c->rs) |
           ((rgb >> 8 & 255u) << c->gs) |
           ((rgb & 255u) << c->bs);
}

static canvas begin(const sz_framebuffer *fb) {
    canvas c;
    c.fb = fb;
    c.scale = fb->width / 640;
    if (fb->height / 360 < c.scale) c.scale = fb->height / 360;
    c.left = (fb->width - 640 * c.scale) / 2;
    c.top = (fb->height - 360 * c.scale) / 2;
    c.rs = fb->format == 0 ? 0 : 16;
    c.gs = 8;
    c.bs = fb->format == 0 ? 16 : 0;
    if (fb->format == 2) {
        c.rs = shift(fb->red_mask); c.gs = shift(fb->green_mask);
        c.bs = shift(fb->blue_mask);
    }
    volatile uint32_t *pixels = (volatile uint32_t *)(uintptr_t)fb->address;
    /* Do not touch scanline padding. Volatile stores are required for MMIO. */
    for (uint32_t y = 0; y < fb->height; y++)
        for (uint32_t x = 0; x < fb->width; x++)
            pixels[(uint64_t)y * fb->stride + x] = 0;
    return c;
}

static void rect(const canvas *c, uint32_t x, uint32_t y,
                 uint32_t w, uint32_t h, uint32_t rgb) {
    if (x >= 640 || y >= 360) return;
    if (w > 640 - x) w = 640 - x;
    if (h > 360 - y) h = 360 - y;
    volatile uint32_t *pixels = (volatile uint32_t *)(uintptr_t)c->fb->address;
    uint32_t packed = color(c, rgb);
    uint32_t px = c->left + x * c->scale, py = c->top + y * c->scale;
    for (uint32_t dy = 0; dy < h * c->scale; dy++)
        for (uint32_t dx = 0; dx < w * c->scale; dx++)
            pixels[(uint64_t)(py + dy) * c->fb->stride + px + dx] = packed;
}

static const uint8_t *rows(char ch) {
    for (size_t i = 0; i < sizeof(font) / sizeof(font[0]); i++)
        if (font[i].ch == ch) return font[i].row;
    if (ch >= 'a' && ch <= 'z') return rows((char)(ch - 'a' + 'A'));
    return font[41].row; /* '?' */
}

static uint32_t length(const char *s) {
    uint32_t n = 0;
    if (s) while (s[n] && n < 100) n++;
    return n;
}

static void text(const canvas *c, const char *s, uint32_t x, uint32_t y,
                 uint32_t scale, uint32_t rgb) {
    uint32_t n = length(s);
    for (uint32_t i = 0; i < n; i++) {
        const uint8_t *r = rows(s[i]);
        for (uint32_t dy = 0; dy < 7; dy++)
            for (uint32_t dx = 0; dx < 5; dx++)
                if (r[dy] & (16u >> dx))
                    rect(c, x + (i * 6 + dx) * scale, y + dy * scale,
                         scale, scale, rgb);
    }
}

static void center(const canvas *c, const char *s, uint32_t y,
                   uint32_t scale, uint32_t rgb) {
    uint32_t w = length(s) * 6 * scale;
    if (w > 632) scale = 1, w = length(s) * 6;
    text(c, s, (640 - w) / 2, y, scale, rgb);
}

/* The source photograph stays fixed. The two recursive tunnels expand
 * through their own texture as frames advance; the face/hands in the
 * middle are untouched. A moving dim scanline adds old-screen motion.
 * Integer-only, bounded, and the same low source resolution throughout. */
static void artwork(const canvas *c, uint32_t frame) {
    for (uint32_t y = 0; y < 128; y++) {
        for (uint32_t x = 0; x < 384; x++) {
            uint32_t sx = x, sy = y;
            if (x < 120 || x >= 264) {
                int cx = x < 120 ? 78 : 306;
                int zoom = 256 + (int)(frame % 32);
                int tx = cx + ((int)x - cx) * 256 / zoom;
                int ty = 64 + ((int)y - 64) * 256 / zoom;
                if (tx >= 0 && tx < 384 && ty >= 0 && ty < 128)
                    sx = (uint32_t)tx, sy = (uint32_t)ty;
            }
            uint32_t at = (sy * 384 + sx) * 2;
            uint32_t p = sz_art[at] | ((uint32_t)sz_art[at + 1] << 8);
            uint32_t r = ((p >> 11) & 31) * 255 / 31;
            uint32_t g = ((p >> 5) & 63) * 255 / 63;
            uint32_t b = (p & 31) * 255 / 31;
            if (y == frame % 128) r = r * 7 / 8, g = g * 7 / 8, b = b * 7 / 8;
            rect(c, 128 + x, 36 + y, 1, 1, (r << 16) | (g << 8) | b);
        }
    }
}

void sz_splash_draw(const sz_framebuffer *fb, uint32_t completed,
                    uint32_t frame, const char *status, uint32_t state,
                    uint64_t error_code) {
    if (!sz_framebuffer_valid(fb)) return;
    canvas c = begin(fb);
    uint32_t accent = state == SZ_SPLASH_ERROR ? 0xd87969 : 0x91b88a;
    artwork(&c, frame);
    text(&c, "System", 173, 196, 7, 0xd4d4ce);
    text(&c, "Z", 425, 196, 7, accent);
    center(&c, "NIGHTWARE LABZ", 254, 2, 0x899087);
    uint32_t done = completed > SZ_SPLASH_STEPS ? SZ_SPLASH_STEPS : completed;
    rect(&c, 212, 285, 216, 1, 0x465246);
    rect(&c, 212, 298, 216, 1, 0x465246);
    rect(&c, 212, 285, 1, 14, 0x465246);
    rect(&c, 427, 285, 1, 14, 0x465246);
    /* Exactly one segment per completed operation, not a timed percentage. */
    for (uint32_t i = 0; i < SZ_SPLASH_STEPS; i++)
        rect(&c, 216 + i * 23, 289, 19, 6, i < done ? accent : 0x121a13);
    center(&c, status, 313, 1, accent);
    if (state == SZ_SPLASH_ERROR) {
        char code[19] = "0x0000000000000000";
        const char *digits = "0123456789ABCDEF";
        for (uint32_t i = 0; i < 16; i++)
            code[2 + i] = digits[(error_code >> (60 - i * 4)) & 15];
        center(&c, code, 329, 1, 0xb4a49a);
        center(&c, "BOOT STOPPED - DETAILS ON SERIAL / FIRMWARE CONSOLE", 346, 1, 0x858a80);
    } else {
        center(&c, completed < 6 ? "ESC: FIRMWARE BOOT DETAILS" : "FLIGHT RECORDER: COM1", 346, 1, 0x566050);
    }
}

void sz_console_draw(const sz_framebuffer *fb, const char *cpu,
                     const char *guest, const char *result) {
    if (!sz_framebuffer_valid(fb)) return;
    canvas c = begin(fb);
    text(&c, "SystemZ", 32, 32, 3, 0xcbd3c5);
    text(&c, "NIGHTWARE LABZ", 438, 41, 1, 0x7c9476);
    rect(&c, 32, 69, 576, 1, 0x354833);
    text(&c, "ZCORE - EARLY BOOT DIAGNOSTICS", 32, 94, 2, 0x91b88a);
    text(&c, "LOADER HANDOFF COMPLETE", 32, 132, 1, 0xb9c5b1);
    text(&c, cpu, 32, 158, 1, 0xb9c5b1);
    text(&c, guest, 32, 181, 1, 0xb9c5b1);
    text(&c, result, 32, 211, 1, 0xd0b37b);
    rect(&c, 32, 252, 576, 1, 0x354833);
    text(&c, "ZADMIN / OPERATOR SHELL: NOT IMPLEMENTED", 32, 274, 1, 0xd0b37b);
    text(&c, "FULL BOOT RECORD: COM1 SERIAL", 32, 301, 1, 0x7c9476);
    text(&c, "CORE HALTED AT ITS CURRENT IMPLEMENTATION BOUNDARY", 32, 325, 1, 0x7c9476);
}
