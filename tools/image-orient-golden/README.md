# image-orient-golden

Host-only dumper for P3-F02 C++ pixel goldens. Pin
`9a24f09927944a795e7ba1bd8243a625bf419479`.

**Does not commit copied reference algorithms.** `extract_ref.py` archives real
functions from the pin into **gitignored** `.build/`:

- `src/data/DataManager.cpp` → `cpu_resize` / nearest / `turn_normal_vectors`
- `src/sfm/core/Exif.h` → `ExifTransform` / `exifTransform`
- `src/core/ImageOrient.h`, `src/core/DistanceTransform.h`

```bash
SPIRULA_REFERENCE_REPO=/path/to/spirula-studio ./build.sh
./bin/spirula-image-orient-golden > ../../fixtures/images/cpp_orient_mask_golden.json
```

`bin/` and `.build/` are gitignored. Regenerated JSON is committed and
registered in `fixtures/manifest.json`.
