#include <cmath>
#include <cstdio>

#include "core/CameraModel.h"
#include "data/CameraMath.h"

static void jnum(double x) {
    if (std::isfinite(x))
        std::printf("%.17g", x);
    else
        std::printf("null");
}
static void jbool(bool b) { std::printf(b ? "true" : "false"); }

static void dump_case(const char* name, int model, int tier, const float* d) {
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

int main() {
    std::printf(
        "{\"schema\":\"spirula-camera-math-golden\",\"version\":1,"
        "\"reference_commit\":\"9a24f09927944a795e7ba1bd8243a625bf419479\",\"cases\":[\n");
    float z[8] = {};
    float opencv[8] = {0.01f, -0.002f, 0.0001f, -0.0002f};
    float prism[8] = {0.01f, -0.002f, 0.0003f, -0.0001f, 0.0001f, -0.0002f, 0.00005f, -0.00004f};
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
        dump_case(rows[i].n, rows[i].m, rows[i].t, rows[i].d);
    }
    double ray_behind[3] = {0, 0, -1}, out[2];
    bool pb = camhost::project_ray(ray_behind, 0, 0, z, out);
    double ray_side[3] = {1, 0, 0};
    bool ps = camhost::project_ray(ray_side, 0, 0, z, out);
    double big[3];
    bool fisheye_over = camhost::generate_ray(3.2, 0.0, 1, 0, z, big);
    std::printf(
        "\n],\"domain\":["
        "{\"name\":\"pinhole_behind\",\"project_ok\":%s},"
        "{\"name\":\"pinhole_side_90\",\"project_ok\":%s},"
        "{\"name\":\"fisheye_r_ge_pi\",\"ray_ok\":%s}"
        "]}\n",
        pb ? "true" : "false", ps ? "true" : "false", fisheye_over ? "true" : "false");
    return 0;
}
