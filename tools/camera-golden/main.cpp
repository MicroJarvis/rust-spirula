// Host-only golden dumper for P3-F01 (CameraMath + SourceCamera + DistortionFit).
// Compiles against the pin export from build.sh. No CUDA / GL.
#include <cmath>
#include <cstdio>
#include <cstring>

#include "core/CameraModel.h"
#include "data/CameraMath.h"
#include "data/DistortionFit.h"
#include "data/SourceCamera.h"

static void jnum(double x) {
    if (std::isfinite(x))
        std::printf("%.17g", x);
    else
        std::printf("null");
}
static void jbool(bool b) { std::printf(b ? "true" : "false"); }

static void dump_math_case(const char* name, int model, int tier, const float* d) {
    std::printf("  {\"name\":\"%s\",\"model\":%d,\"tier\":%d,\"coeffs\":[", name, model, tier);
    for (int i = 0; i < 8; i++) {
        if (i)
            std::putchar(',');
        std::printf("%.9g", (double)d[i]);
    }
    std::printf("],\"samples\":[\n");
    const double pts[][2] = {{0, 0}, {0.1, 0}, {-0.2, 0.15}, {0.5, 0.25}, {-0.8, -0.4},
                             {1.2, 0.0}, {-1.5, 0.7}, {0.0, 1.1}};
    for (size_t i = 0; i < sizeof(pts) / sizeof(pts[0]); i++) {
        double u = pts[i][0], v = pts[i][1];
        double dist[2], und[2], ray[3], back[2];
        camhost::distort_point(u, v, model, tier, d, dist);
        bool und_ok = camhost::undistort_point(dist[0], dist[1], model, tier, d, und);
        bool ray_ok = camhost::generate_ray(dist[0], dist[1], model, tier, d, ray);
        bool proj_ok = ray_ok && camhost::project_ray(ray, model, tier, d, back);
        bool valid = camhost::valid_distortion(u, v, tier, d);
        if (i)
            std::printf(",\n");
        std::printf("    {\"u\":");
        jnum(u);
        std::printf(",\"v\":");
        jnum(v);
        std::printf(",\"distort\":[");
        jnum(dist[0]);
        std::printf(",");
        jnum(dist[1]);
        std::printf("],\"valid\":");
        jbool(valid);
        std::printf(",\"undistort_ok\":");
        jbool(und_ok);
        std::printf(",\"undistort\":[");
        if (und_ok) {
            jnum(und[0]);
            std::printf(",");
            jnum(und[1]);
        } else {
            std::printf("null,null");
        }
        std::printf("],\"ray_ok\":");
        jbool(ray_ok);
        std::printf(",\"ray\":[");
        if (ray_ok) {
            jnum(ray[0]);
            std::printf(",");
            jnum(ray[1]);
            std::printf(",");
            jnum(ray[2]);
        } else {
            std::printf("null,null,null");
        }
        std::printf("],\"project_ok\":");
        jbool(proj_ok);
        std::printf(",\"project\":[");
        if (proj_ok) {
            jnum(back[0]);
            std::printf(",");
            jnum(back[1]);
        } else {
            std::printf("null,null");
        }
        std::printf("]}");
    }
    std::printf("\n  ]}");
}

static void dump_ray_boundary(const char* name, int model, double u, double v) {
    float z[8] = {};
    double ray[3] = {}, back[2] = {};
    bool ray_ok = camhost::generate_ray(u, v, model, 0, z, ray);
    bool proj_ok = ray_ok && camhost::project_ray(ray, model, 0, z, back);
    std::printf("{\"name\":\"%s\",\"model\":%d,\"u\":", name, model);
    jnum(u);
    std::printf(",\"v\":");
    jnum(v);
    std::printf(",\"ray_ok\":");
    jbool(ray_ok);
    std::printf(",\"ray\":[");
    jnum(ray[0]);
    std::printf(",");
    jnum(ray[1]);
    std::printf(",");
    jnum(ray[2]);
    std::printf("],\"project_ok\":");
    jbool(proj_ok);
    std::printf(",\"project\":[");
    jnum(back[0]);
    std::printf(",");
    jnum(back[1]);
    std::printf("]}");
}

int main() {
    const double pi = std::acos(-1.0);
    float z[8] = {};
    float opencv[8] = {0.01f, -0.002f, 0.0001f, -0.0002f};
    float prism[8] = {0.01f, -0.002f, 0.0003f, -0.0001f, 0.0001f, -0.0002f, 0.00005f, -0.00004f};

    std::printf(
        "{\"schema\":\"spirula-camera-golden\",\"version\":2,"
        "\"reference_commit\":\"9a24f09927944a795e7ba1bd8243a625bf419479\","
        "\"exact_fit_px\":%.17g,",
        dsfit::kExactFitPx);

    // --- CameraMath model×tier grid (first golden 10) ---
    std::printf("\"math_cases\":[\n");
    struct Row {
        const char* n;
        int m;
        int t;
        float* d;
    } rows[] = {
        {"pinhole_none", 0, 0, z},
        {"pinhole_opencv", 0, 1, opencv},
        {"pinhole_thinprism", 0, 2, prism},
        {"fisheye_none", 1, 0, z},
        {"fisheye_opencv", 1, 1, opencv},
        {"fisheye_thinprism", 1, 2, prism},
        {"equisolid_none", 2, 0, z},
        {"equisolid_opencv", 2, 1, opencv},
        {"equisolid_thinprism", 2, 2, prism},
        {"equirect_none", 3, 0, z},
    };
    for (size_t i = 0; i < sizeof(rows) / sizeof(rows[0]); i++) {
        if (i)
            std::printf(",\n");
        dump_math_case(rows[i].n, rows[i].m, rows[i].t, rows[i].d);
    }
    std::printf("\n],");

    // --- domain / boundary ---
    double ray_behind[3] = {0, 0, -1}, out[2];
    bool pb = camhost::project_ray(ray_behind, 0, 0, z, out);
    double ray_side[3] = {1, 0, 0};
    bool ps = camhost::project_ray(ray_side, 0, 0, z, out);
    double big[3];
    bool fisheye_over = camhost::generate_ray(3.2, 0.0, 1, 0, z, big);
    bool equi_back = camhost::project_ray(ray_behind, 2, 0, z, out);

    std::printf("\"domain\":[");
    std::printf("{\"name\":\"pinhole_behind\",\"project_ok\":%s},", pb ? "true" : "false");
    std::printf("{\"name\":\"pinhole_side_90\",\"project_ok\":%s},", ps ? "true" : "false");
    std::printf("{\"name\":\"fisheye_r_ge_pi\",\"ray_ok\":%s},", fisheye_over ? "true" : "false");
    std::printf("{\"name\":\"equisolid_backaxis\",\"project_ok\":%s},", equi_back ? "true" : "false");
    dump_ray_boundary("fisheye_past90", 1, 2.0, 0.0);
    std::printf(",");
    dump_ray_boundary("fisheye_pi", 1, pi, 0.0);
    std::printf(",");
    dump_ray_boundary("equisolid_past90", 2, 1.8, 0.0);
    std::printf(",");
    dump_ray_boundary("equisolid_r2", 2, 2.0, 0.0);
    std::printf(",");
    dump_ray_boundary("pano_seam", 3, pi, 0.0);
    std::printf(",");
    dump_ray_boundary("pano_pole", 3, 1.0, pi / 2.0);
    std::printf("],");

    // Newton residual vs valid_distortion (fold OpenCV k1=-1)
    float fold[8] = {-1.f, 0, 0, 0, 0, 0, 0, 0};
    std::printf("\"newton_vs_valid\":[");
    bool first_fold = true;
    for (double u : {0.4, 0.5, 0.7, 1.0}) {
        double inv[2] = {};
        bool ok = camhost::undistort_lens(u, 0, 1, fold, inv);
        bool valid = camhost::valid_distortion(u, 0, 1, fold);
        if (!first_fold)
            std::printf(",");
        first_fold = false;
        std::printf("{\"u\":");
        jnum(u);
        std::printf(",\"valid\":");
        jbool(valid);
        std::printf(",\"inverse_ok\":");
        jbool(ok);
        std::printf(",\"q\":[");
        jnum(inv[0]);
        std::printf(",");
        jnum(inv[1]);
        std::printf("]}");
    }
    std::printf("],");

    // Panorama frame helpers
    camhost::Camera c;
    c.model = 3;
    c.width = 800;
    c.height = 400;
    c.fx = 800 / (2 * pi);
    c.fy = 400 / pi;
    c.cx = 400;
    c.cy = 200;
    c.source_model = -1;
    double px[2] = {-1, 200};
    bool n = camhost::normalize_pixel(c, px);
    double aa[2] = {799, 200}, bb[2] = {1, 200}, delta[2];
    camhost::pixel_difference(c, aa, bb, delta);
    double pole_px[2] = {400, -0.5};
    bool pole_norm = camhost::normalize_pixel(c, pole_px);
    std::printf(
        "\"panorama\":{\"full_longitude\":%s,\"normalize_wrap\":%s,"
        "\"px\":[%.17g,%.17g],\"delta_799_1\":[%.17g,%.17g],"
        "\"normalize_pole_slack\":%s},",
        camhost::full_longitude(c) ? "true" : "false", n ? "true" : "false", px[0], px[1],
        delta[0], delta[1], pole_norm ? "true" : "false");

    // SourceCamera samples (params stay f32)
    float fov_prm[16] = {1000, 900, 800, 600, 0.9f};
    float fov_before[16];
    std::memcpy(fov_before, fov_prm, sizeof fov_prm);
    double su, sv;
    bool s_ok = srccam::project(srccam::kColmapFOV, fov_prm, 0.3, -0.2, 1, &su, &sv);
    float eucm[16] = {600, 600, 800, 600, 0.6f, 1.1f};
    double eu = 0, ev = 0, ebu = 0, ebv = 0;
    bool e_ok = srccam::project(srccam::kColmapEUCM, eucm, 0.2, 0.1, 1, &eu, &ev);
    bool e_behind = srccam::project(srccam::kColmapEUCM, eucm, 0, 0, -1, &ebu, &ebv);
    std::printf(
        "\"source\":["
        "{\"name\":\"fov_forward\",\"model\":%d,\"ok\":%s,\"pixel\":[",
        srccam::kColmapFOV, s_ok ? "true" : "false");
    jnum(su);
    std::printf(",");
    jnum(sv);
    std::printf(
        "],\"params_f32\":[1000,900,800,600,0.9],"
        "\"params_unchanged\":%s},",
        std::memcmp(fov_before, fov_prm, sizeof fov_prm) == 0 ? "true" : "false");
    std::printf(
        "{\"name\":\"eucm_forward\",\"model\":%d,\"ok\":%s,\"pixel\":[",
        srccam::kColmapEUCM, e_ok ? "true" : "false");
    jnum(eu);
    std::printf(",");
    jnum(ev);
    std::printf("]},{\"name\":\"eucm_behind\",\"model\":%d,\"ok\":%s}",
                srccam::kColmapEUCM, e_behind ? "true" : "false");
    std::printf("],");

    // DistortionFit: FOV auto + empty + holdout metrics; preserve source params
    dsfit::FitOptions opts;
    opts.grid = 16;
    dsfit::SourceProject source = [&](double x, double y, double z, double* u, double* v) {
        return srccam::project(srccam::kColmapFOV, fov_prm, x, y, z, u, v);
    };
    auto fit = dsfit::fit_camera_auto(source, 1600, 1200, opts);
    bool src_unchanged = std::memcmp(fov_before, fov_prm, sizeof fov_prm) == 0;
    int active_source = (fit.max_px < dsfit::kExactFitPx) ? -1 : (int)srccam::kColmapFOV;
    int original_source = (int)srccam::kColmapFOV;

    auto none = dsfit::fit_camera_auto(
        [](double, double, double, double*, double*) { return false; }, 1600, 1200, opts);

    std::printf(
        "\"fit\":["
        "{\"name\":\"fov_auto_grid16\",\"ok\":%s,\"invertible\":%s,"
        "\"model\":%d,\"tier\":%d,\"rms_px\":",
        fit.ok ? "true" : "false", fit.invertible ? "true" : "false", (int)fit.target.model,
        (int)fit.target.distortion);
    jnum(fit.rms_px);
    std::printf(",\"max_px\":");
    jnum(fit.max_px);
    std::printf(",\"samples\":%d,\"fx\":", fit.samples);
    jnum(fit.target.fx);
    std::printf(",\"fy\":");
    jnum(fit.target.fy);
    std::printf(",\"cx\":");
    jnum(fit.target.cx);
    std::printf(",\"cy\":");
    jnum(fit.target.cy);
    std::printf(",\"coeffs\":[");
    for (int i = 0; i < 8; i++) {
        if (i)
            std::putchar(',');
        jnum(fit.target.coeffs[i]);
    }
    std::printf(
        "],\"source_params_unchanged\":%s,"
        "\"active_source_model\":%d,\"original_source_model\":%d,"
        "\"source_params_f32\":[1000,900,800,600,0.9]},",
        src_unchanged ? "true" : "false", active_source, original_source);

    std::printf(
        "{\"name\":\"empty_domain\",\"ok\":%s,\"model\":%d,\"tier\":%d,"
        "\"fx\":",
        none.ok ? "true" : "false", (int)none.target.model, (int)none.target.distortion);
    jnum(none.target.fx);
    std::printf(",\"fy\":");
    jnum(none.target.fy);
    std::printf(",\"cx\":");
    jnum(none.target.cx);
    std::printf(",\"cy\":");
    jnum(none.target.cy);
    std::printf(",\"samples\":%d}", none.samples);

    std::printf("]}\n");
    return 0;
}
