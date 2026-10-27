# image-orient-golden

Host-only dumper for P3-F02 C++ pixel goldens (pin
`9a24f09927944a795e7ba1bd8243a625bf419479`).

```bash
SPIRULA_REFERENCE_REPO=/path/to/spirula-studio ./build.sh
./bin/spirula-image-orient-golden > ../../fixtures/images/cpp_orient_mask_golden.json
```

Archives `src/core/ImageOrient.h` from the pin and inlines `cpu_resize` /
`cpu_nearest_resize_u8` from `DataManager.cpp` so the golden matches train
resize without linking the full studio binary. Output is consumed by
`crates/spirula-io/tests/image_orient_mask.rs` and registered in
`fixtures/manifest.json`.
