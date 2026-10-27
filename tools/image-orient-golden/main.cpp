// P3-F02 golden: asymmetric 3x2 unique gray + RGB, full EXIF 1..8 Full/TurnsOnly,
// mask nearest, alpha cpu_resize (area/bilinear) then >=128.
#include "core/ImageOrient.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <vector>

template <typename T, int C>
void cpu_bilinear_resize(const T* src, int sh, int sw, T* dst, int dh, int dw) {
    const float sx = (float)sw / (float)dw;
    const float sy = (float)sh / (float)dh;
    for (int y = 0; y < dh; ++y) {
        float v = ((float)y + 0.5f) * sy - 0.5f;
        int y0 = (int)std::floor(v);
        float fy = v - (float)y0;
        int y0c = std::max(0, std::min(sh - 1, y0));
        int y1c = std::max(0, std::min(sh - 1, y0 + 1));
        for (int x = 0; x < dw; ++x) {
            float u = ((float)x + 0.5f) * sx - 0.5f;
            int x0 = (int)std::floor(u);
            float fx = u - (float)x0;
            int x0c = std::max(0, std::min(sw - 1, x0));
            int x1c = std::max(0, std::min(sw - 1, x0 + 1));
            float w00 = (1.0f - fx) * (1.0f - fy);
            float w10 = fx * (1.0f - fy);
            float w01 = (1.0f - fx) * fy;
            float w11 = fx * fy;
            const T* p00 = src + ((size_t)y0c * sw + x0c) * C;
            const T* p10 = src + ((size_t)y0c * sw + x1c) * C;
            const T* p01 = src + ((size_t)y1c * sw + x0c) * C;
            const T* p11 = src + ((size_t)y1c * sw + x1c) * C;
            T* q = dst + ((size_t)y * dw + x) * C;
            for (int c = 0; c < C; ++c) {
                float r = w00 * (float)p00[c] + w10 * (float)p10[c] + w01 * (float)p01[c] +
                          w11 * (float)p11[c];
                r = std::round(r);
                if (r < 0.0f) r = 0.0f;
                float hi = (float)std::numeric_limits<T>::max();
                if (r > hi) r = hi;
                q[c] = (T)r;
            }
        }
    }
}

template <typename T, int C>
void cpu_area_resize(const T* src, int sh, int sw, T* dst, int dh, int dw) {
    const double sy = (double)sh / dh, sx = (double)sw / dw;
    for (int y = 0; y < dh; ++y) {
        const double v0 = y * sy, v1 = (y + 1) * sy;
        const int y0 = (int)v0, y1 = std::min(sh, std::max((int)std::ceil(v1), y0 + 1));
        for (int x = 0; x < dw; ++x) {
            const double u0 = x * sx, u1 = (x + 1) * sx;
            const int x0 = (int)u0, x1 = std::min(sw, std::max((int)std::ceil(u1), x0 + 1));
            double acc[C] = {}, wsum = 0.0;
            for (int yy = y0; yy < y1; ++yy) {
                const double wy = std::min((double)yy + 1, v1) - std::max((double)yy, v0);
                for (int xx = x0; xx < x1; ++xx) {
                    const double wx = std::min((double)xx + 1, u1) - std::max((double)xx, u0);
                    const double w = wy * wx;
                    const T* p = src + ((size_t)yy * sw + xx) * C;
                    for (int c = 0; c < C; ++c) acc[c] += w * (double)p[c];
                    wsum += w;
                }
            }
            T* q = dst + ((size_t)y * dw + x) * C;
            for (int c = 0; c < C; ++c) {
                double r = wsum > 0.0 ? acc[c] / wsum : 0.0;
                r = std::round(r);
                r = std::min(std::max(r, 0.0), (double)std::numeric_limits<T>::max());
                q[c] = (T)r;
            }
        }
    }
}

template <typename T, int C>
void cpu_resize(const T* src, int sh, int sw, T* dst, int dh, int dw) {
    if (dh <= sh && dw <= sw && (dh < sh || dw < sw))
        cpu_area_resize<T, C>(src, sh, sw, dst, dh, dw);
    else
        cpu_bilinear_resize<T, C>(src, sh, sw, dst, dh, dw);
}

void cpu_nearest_resize_u8(const uint8_t* src, int sh, int sw, uint8_t* dst, int dh, int dw) {
    const float sx = (float)sw / (float)dw;
    const float sy = (float)sh / (float)dh;
    for (int y = 0; y < dh; ++y) {
        float v = ((float)y + 0.5f) * sy - 0.5f;
        int ys = std::max(0, std::min(sh - 1, (int)std::floor(v + 0.5f)));
        for (int x = 0; x < dw; ++x) {
            float u = ((float)x + 0.5f) * sx - 0.5f;
            int xs = std::max(0, std::min(sw - 1, (int)std::floor(u + 0.5f)));
            dst[(size_t)y * dw + x] = src[(size_t)ys * sw + xs];
        }
    }
}

struct XF {
    int turns;
    bool mirror;
};
XF exif_xf(int o) {
    switch (o) {
        case 2: return {0, true};
        case 3: return {2, false};
        case 4: return {2, true};
        case 5: return {1, true};
        case 6: return {1, false};
        case 7: return {3, true};
        case 8: return {3, false};
        default: return {0, false};
    }
}

void dump_arr(const uint8_t* p, size_t n) {
    std::printf("[");
    for (size_t i = 0; i < n; i++) {
        if (i) std::putchar(',');
        std::printf("%u", p[i]);
    }
    std::printf("]");
}

int main() {
    // Asymmetric 3x2 unique gray values 1..6
    const int W = 3, H = 2;
    uint8_t gray[6] = {1, 2, 3, 4, 5, 6};
    // RGB: unique per pixel (r=10*i,g=20*i,b=30*i) i=1..6
    uint8_t rgb[18];
    for (int i = 0; i < 6; i++) {
        rgb[i * 3] = (uint8_t)(10 * (i + 1));
        rgb[i * 3 + 1] = (uint8_t)(20 * (i + 1));
        rgb[i * 3 + 2] = (uint8_t)(30 * (i + 1));
    }
    // Alpha 3x2 for resize golden: distinctive
    uint8_t alpha[6] = {0, 64, 128, 192, 255, 32};

    std::printf(
        "{\"schema\":\"spirula-image-orient-mask-cpp-golden\",\"version\":1,"
        "\"reference_commit\":\"9a24f09927944a795e7ba1bd8243a625bf419479\","
        "\"src_w\":%d,\"src_h\":%d,\"gray\":",
        W, H);
    dump_arr(gray, 6);
    std::printf(",\"rgb\":");
    dump_arr(rgb, 18);
    std::printf(",\"alpha\":");
    dump_arr(alpha, 6);
    std::printf(",\"orientations\":[");

    for (int o = 1; o <= 8; o++) {
        if (o > 1) std::putchar(',');
        XF xf = exif_xf(o);
        int dw = W, dh = H;
        spirula::oriented_size(xf.turns, dw, dh);
        std::vector<uint8_t> full_g((size_t)dw * dh), turns_g((size_t)dw * dh);
        std::vector<uint8_t> full_rgb((size_t)dw * dh * 3), turns_rgb((size_t)dw * dh * 3);
        spirula::orient_pixels(gray, W, H, 1, xf.turns, xf.mirror, full_g.data());
        spirula::orient_pixels(gray, W, H, 1, xf.turns, false, turns_g.data());
        spirula::orient_pixels(rgb, W, H, 3, xf.turns, xf.mirror, full_rgb.data());
        spirula::orient_pixels(rgb, W, H, 3, xf.turns, false, turns_rgb.data());
        // Mask nonzero from gray after turns-only, nearest to 2x3
        std::vector<uint8_t> bin(6);
        for (int i = 0; i < 6; i++) bin[i] = gray[i] != 0 ? 1 : 0;
        int mw = W, mh = H;
        spirula::oriented_size(xf.turns, mw, mh);
        std::vector<uint8_t> mob((size_t)mw * mh);
        spirula::orient_pixels(bin.data(), W, H, 1, xf.turns, false, mob.data());
        std::vector<uint8_t> mdst(2 * 3);
        cpu_nearest_resize_u8(mob.data(), mh, mw, mdst.data(), 3, 2);

        std::printf(
            "{\"orientation\":%d,\"turns_cw\":%d,\"mirror\":%s,\"out_w\":%d,\"out_h\":%d,"
            "\"full_gray\":",
            o, xf.turns, xf.mirror ? "true" : "false", dw, dh);
        dump_arr(full_g.data(), full_g.size());
        std::printf(",\"turns_gray\":");
        dump_arr(turns_g.data(), turns_g.size());
        std::printf(",\"full_rgb\":");
        dump_arr(full_rgb.data(), full_rgb.size());
        std::printf(",\"turns_rgb\":");
        dump_arr(turns_rgb.data(), turns_rgb.size());
        std::printf(",\"mask_turns_nearest_2x3\":");
        dump_arr(mdst.data(), mdst.size());
        std::printf("}");
    }
    std::printf("],\"alpha_resize\":[");
    // Downsample 3x2 -> 2x1 (area) and upsample 3x2 -> 5x3 (bilinear), then >=128
    uint8_t down[2], up[15];
    cpu_resize<uint8_t, 1>(alpha, H, W, down, 1, 2);
    cpu_resize<uint8_t, 1>(alpha, H, W, up, 3, 5);
    uint8_t down_bin[2], up_bin[15];
    for (int i = 0; i < 2; i++) down_bin[i] = down[i] >= 128 ? 1 : 0;
    for (int i = 0; i < 15; i++) up_bin[i] = up[i] >= 128 ? 1 : 0;
    std::printf("{\"dst_w\":2,\"dst_h\":1,\"raw\":");
    dump_arr(down, 2);
    std::printf(",\"bin\":");
    dump_arr(down_bin, 2);
    std::printf("},{\"dst_w\":5,\"dst_h\":3,\"raw\":");
    dump_arr(up, 15);
    std::printf(",\"bin\":");
    dump_arr(up_bin, 15);
    std::printf("}]}\n");
    return 0;
}
