// P3-F02 golden dumper — links *extracted* pin sources (see extract_ref.py).
// Does not embed hand-copied cpu_resize / EXIF tables.
#include "ImageOrient.h"
#include "DistanceTransform.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

#include "cpu_resize.inc"
#include "exif_transform.inc"
#include "turn_normal.inc"

void dump_arr(const uint8_t* p, size_t n) {
    std::printf("[");
    for (size_t i = 0; i < n; i++) {
        if (i) std::putchar(',');
        std::printf("%u", p[i]);
    }
    std::printf("]");
}

void dump_u16(const uint16_t* p, size_t n) {
    std::printf("[");
    for (size_t i = 0; i < n; i++) {
        if (i) std::putchar(',');
        std::printf("%u", (unsigned)p[i]);
    }
    std::printf("]");
}

int main() {
    const int W = 3, H = 2;
    uint8_t gray[6] = {1, 2, 3, 4, 5, 6};
    uint8_t rgb[18];
    for (int i = 0; i < 6; i++) {
        rgb[i * 3] = (uint8_t)(10 * (i + 1));
        rgb[i * 3 + 1] = (uint8_t)(20 * (i + 1));
        rgb[i * 3 + 2] = (uint8_t)(30 * (i + 1));
    }
    uint8_t alpha[6] = {0, 64, 128, 192, 255, 32};
    // Counterexample: 2x1 RGBA black-opaque / red-transparent NonZero plane.
    uint8_t rgba_ce[8] = {0, 0, 0, 255, 255, 0, 0, 0};
    uint8_t alpha_ce[2] = {0, 255};

    std::printf(
        "{\"schema\":\"spirula-image-orient-mask-cpp-golden\",\"version\":2,"
        "\"reference_commit\":\"9a24f09927944a795e7ba1bd8243a625bf419479\","
        "\"extraction\":\"DataManager.cpp+Exif.h+ImageOrient.h+DistanceTransform.h\","
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
        ExifTransform xf = exifTransform(o);
        int dw = W, dh = H;
        spirula::oriented_size(xf.turns_cw, dw, dh);
        std::vector<uint8_t> full_g((size_t)dw * dh), turns_g((size_t)dw * dh);
        std::vector<uint8_t> full_rgb((size_t)dw * dh * 3), turns_rgb((size_t)dw * dh * 3);
        spirula::orient_pixels(gray, W, H, 1, xf.turns_cw, xf.mirror, full_g.data());
        spirula::orient_pixels(gray, W, H, 1, xf.turns_cw, false, turns_g.data());
        spirula::orient_pixels(rgb, W, H, 3, xf.turns_cw, xf.mirror, full_rgb.data());
        spirula::orient_pixels(rgb, W, H, 3, xf.turns_cw, false, turns_rgb.data());
        std::vector<uint8_t> bin(6);
        for (int i = 0; i < 6; i++) bin[i] = gray[i] != 0 ? 1 : 0;
        int mw = W, mh = H;
        spirula::oriented_size(xf.turns_cw, mw, mh);
        std::vector<uint8_t> mob((size_t)mw * mh);
        spirula::orient_pixels(bin.data(), W, H, 1, xf.turns_cw, false, mob.data());
        std::vector<uint8_t> mdst(2 * 3);
        cpu_nearest_resize_u8(mob.data(), mh, mw, mdst.data(), 3, 2);

        std::printf(
            "{\"orientation\":%d,\"turns_cw\":%d,\"mirror\":%s,\"out_w\":%d,\"out_h\":%d,"
            "\"full_gray\":",
            o, xf.turns_cw, xf.mirror ? "true" : "false", dw, dh);
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
    std::printf("}],\"counterexamples\":{");

    // stbi gray NonZero on RGBA counterexample
    auto luma = [](uint8_t r, uint8_t g, uint8_t b) -> uint8_t {
        return (uint8_t)((r * 77u + g * 150u + b * 29u) >> 8);
    };
    uint8_t nz[2] = {
        (uint8_t)(luma(rgba_ce[0], rgba_ce[1], rgba_ce[2]) != 0),
        (uint8_t)(luma(rgba_ce[4], rgba_ce[5], rgba_ce[6]) != 0),
    };
    uint8_t araw[1], abin[1];
    cpu_resize<uint8_t, 1>(alpha_ce, 1, 2, araw, 1, 1);
    abin[0] = araw[0] >= 128 ? 1 : 0;
    std::printf("\"nonzero_rgba_luma\":");
    dump_arr(nz, 2);
    std::printf(",\"alpha_2x1_to_1x1_raw\":");
    dump_arr(araw, 1);
    std::printf(",\"alpha_2x1_to_1x1_bin\":");
    dump_arr(abin, 1);

    // Boundary offset on a 5x5 plus-shaped mask, fraction -> px
    uint8_t mask25[25] = {};
    for (int i = 0; i < 25; i++) mask25[i] = 0;
    mask25[12] = 1;  // center
    mask25[7] = 1;
    mask25[11] = 1;
    mask25[13] = 1;
    mask25[17] = 1;
    float frac = 0.2f;
    float offset_px = frac * std::sqrt(25.0f);
    uint8_t dil[25];
    std::memcpy(dil, mask25, 25);
    edt::apply_mask_boundary_offset_in_place(dil, 5, 5, offset_px);
    uint8_t ero[25];
    std::memcpy(ero, mask25, 25);
    edt::apply_mask_boundary_offset_in_place(ero, 5, 5, -offset_px);
    std::printf(",\"offset_frac\":%.6g,\"offset_px\":%.6g,\"mask5_plus\":", frac, offset_px);
    dump_arr(mask25, 25);
    std::printf(",\"mask5_dilate\":");
    dump_arr(dil, 25);
    std::printf(",\"mask5_erode\":");
    dump_arr(ero, 25);

    // Normal turn 90 CW on one vector encoded as uint8
    uint8_t nrm[3] = {200, 100, 128};
    turn_normal_vectors(nrm, 1, 1);
    std::printf(",\"normal_turn1\":");
    dump_arr(nrm, 3);

    // decode_normal_into order: pixel turn → bilinear resize → turn_normal_vectors
    // Counterexample src 2x1 RGB, dst 1x1, for turns_cw 0..3 (exact pixels).
    std::printf(",\"normal_combined_2x1_to_1x1\":[");
    for (int turns = 0; turns < 4; turns++) {
        if (turns) std::putchar(',');
        uint8_t src6[6] = {0, 0, 128, 0, 1, 128};
        const uint8_t* src = src6;
        int sw = 2, sh = 1;
        std::vector<uint8_t> turned;
        if ((turns & 3) != 0) {
            turned.resize((size_t)sw * sh * 3);
            spirula::orient_pixels(src, sw, sh, 3, turns, false, turned.data());
            spirula::oriented_size(turns, sw, sh);
            src = turned.data();
        }
        uint8_t dst3[3];
        if (sw == 1 && sh == 1) {
            std::memcpy(dst3, src, 3);
        } else {
            cpu_bilinear_resize<uint8_t, 3>(src, sh, sw, dst3, 1, 1);
        }
        turn_normal_vectors(dst3, 1, turns);
        std::printf("{\"turns_cw\":%d,\"rgb\":", turns);
        dump_arr(dst3, 3);
        std::printf("}");
    }
    std::printf("]");

    // Depth bilinear 2x2 -> 1x1
    uint16_t depth[4] = {100, 200, 300, 400};
    uint16_t dout[1];
    cpu_bilinear_resize<uint16_t, 1>(depth, 2, 2, dout, 1, 1);
    std::printf(",\"depth_bilinear_2x2_to_1x1\":");
    dump_u16(dout, 1);
    std::printf("}}\n");
    return 0;
}
