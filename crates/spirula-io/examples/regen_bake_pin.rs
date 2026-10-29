//! Bake-pin expected.txt is a **C++ pin oracle**, not regenerated from under-test Rust.
//!
//! Use the host-only extract/compile entry instead:
//!
//! ```bash
//! ./tools/bake-pin-golden/build.sh
//! ./tools/bake-pin-golden/bin/spirula-bake-pin-golden \
//!   > fixtures/datasets/colmap-goldens/bake-pin/expected.txt
//! ```
//!
//! See `tools/bake-pin-golden/README.md`. This example refuses to overwrite the
//! golden with `bake_and_fit` / `source_project` (that previously collapsed the
//! independent C++ oracle).

fn main() {
    eprintln!(
        "error: bake-pin expected.txt is the C++ pin oracle.\n\
         Regenerate via tools/bake-pin-golden (extract + compile under .build/;\n\
         do not commit C++ sources). See tools/bake-pin-golden/README.md."
    );
    std::process::exit(2);
}
