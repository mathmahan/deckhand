//! Builds argosy-sigil, the C library save sync reads and writes emulator saves through — see
//! `src/sigil`. It is a git submodule pinned to one commit (`third_party/argosy-sigil`), because its
//! C API is not frozen before 1.0: the bindings in `src/sigil/ffi.rs` are written against that commit.
//!
//! Static, so the AppImage and the .exe stay one file each, and always optimised: a debug build of
//! the C would make hashing an 8 MB PS2 card the slow part of every sync. Every container sigil reads
//! (CHD, CSO, zip) comes with it — zlib, zstd and LZMA from libchdr's tree — so nothing on the build
//! machine beyond a C compiler and CMake is linked.
//!
//! A small C file of `sizeof`s is compiled beside it, which the bindings' tests hold their own
//! layouts to: a field sigil adds or moves in a later commit fails a test instead of reading the
//! wrong bytes.

use std::path::Path;

fn main() {
    let sigil = Path::new("third_party/argosy-sigil");
    if !sigil.join("CMakeLists.txt").is_file() || !sigil.join("third_party/libchdr/CMakeLists.txt").is_file() {
        panic!("third_party/argosy-sigil is missing: run `git submodule update --init --recursive`");
    }

    let built = cmake::Config::new(sigil)
        .define("SIGIL_BUILD_CLI", "OFF")
        .define("SIGIL_BUILD_TESTS", "OFF")
        .define("SIGIL_BUILD_SHARED", "OFF")
        .profile("Release")
        .build_target("sigil")
        .build();
    let libs = built.join("build");
    println!("cargo:rustc-link-search=native={}", libs.display());
    // sigil first: the linker resolves left to right, and it is what needs the others.
    for lib in ["sigil", "sigil_chdr", "sigil_lzma", "sigil_zlib", "sigil_zstd", "sigil_aes"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    cc::Build::new().file("src/sigil/sizes.c").include(sigil.join("include")).compile("deckhand_sigil_sizes");

    println!("cargo:rerun-if-changed=src/sigil/sizes.c");
    for watched in ["CMakeLists.txt", "include", "src"] {
        println!("cargo:rerun-if-changed={}", sigil.join(watched).display());
    }
}
