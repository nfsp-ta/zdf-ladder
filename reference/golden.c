/* SPDX-License-Identifier: GPL-3.0-only
 * Golden-value generator: runs Clipper's original C ladder (ladder_ref.h, extracted verbatim from
 * firmware/src/dsp.c) and prints hashes that tests/bit_exact.rs must reproduce.
 *
 *   cc -O2 -o golden reference/golden.c -lm && ./golden [sample_rate]
 */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static uint16_t SVF_G[128];
static int16_t TANH_Q15[257];

#include "ladder_ref.h"

static void gen_tables(double fs)   /* as Clipper's tools/gen_tables.py */
{
    for (int v = 0; v < 128; v++) {
        double f = 30.0 * pow(16000.0 / 30.0, v / 127.0);
        double fm = f < 0.45 * fs ? f : 0.45 * fs;
        SVF_G[v] = (uint16_t)(int)(4096.0 * tan(M_PI * fm / fs));
    }
    for (int i = 0; i < 257; i++)
        TANH_Q15[i] = (int16_t)(int)(32767.0 * tanh(i / 256.0 * 2.0));
}

static uint64_t h = 0xcbf29ce484222325ull;
static void fnv(int32_t v)
{
    uint32_t u = (uint32_t)v;
    for (int b = 0; b < 4; b++) {
        h ^= (u >> (8 * b)) & 0xFF;
        h *= 0x100000001b3ull;
    }
}

static uint32_t r = 0x12345678u;
static uint32_t xs(void)
{
    r ^= r << 13;
    r ^= r >> 17;
    r ^= r << 5;
    return r;
}

int main(int argc, char **argv)
{
    double fs = argc > 1 ? atof(argv[1]) : 44100.0;
    gen_tables(fs);

    /* 1. every coefficient set */
    h = 0xcbf29ce484222325ull;
    for (int32_t cut = 0; cut <= 127 << 8; cut++)
        for (int32_t res = 0; res <= 127; res++) {
            ladder_t c;
            ladder_coef(&c, cut, res);
            fnv(c.G), fnv(c.H), fnv(c.G4), fnv(c.inv), fnv(c.k);
        }
    printf("pub const COEF_HASH: u64 = 0x%016llx;\n", (unsigned long long)h);

    /* 2. softclip */
    h = 0xcbf29ce484222325ull;
    for (int32_t x = -300000; x <= 300000; x++)
        fnv(softclip(x));
    printf("pub const SOFTCLIP_HASH: u64 = 0x%016llx;\n", (unsigned long long)h);

    /* 3. a long stream: random settings every 32 samples, noise / saw / impulses / silence */
    h = 0xcbf29ce484222325ull;
    {
        ladder_t c = {0, 0, 0, 0, 0};
        int32_t s[4] = {0, 0, 0, 0};
        uint32_t kind = 0, sh = 16, phase = 0, inc = 0;
        int32_t first[8];
        for (uint32_t n = 0; n < 2000000u; n++) {
            int32_t x, y;
            if (n % 2048 == 0) {
                kind = xs() % 4;
                sh = 12 + xs() % 8;
                inc = xs() >> 6;
            }
            if (n % 32 == 0) {
                int32_t cut = (int32_t)(xs() % 32513u), res = (int32_t)(xs() % 128u);
                ladder_coef(&c, cut, res);
            }
            switch (kind) {
            case 0: x = (int32_t)xs() >> sh; break;
            case 1: phase += inc; x = (int32_t)phase >> sh; break;
            case 2: x = (n % 512 == 0) ? 131072 : 0; break;
            default: x = 0; break;
            }
            y = ladder_tick(&c, x, s);
            if (n < 8)
                first[n] = y;
            fnv(y);
        }
        fnv(s[0]), fnv(s[1]), fnv(s[2]), fnv(s[3]);
        printf("pub const STREAM_HASH: u64 = 0x%016llx;\n", (unsigned long long)h);
        printf("// first outputs:");
        for (int i = 0; i < 8; i++)
            printf(" %d", first[i]);
        printf("\n");
    }
    return 0;
}
