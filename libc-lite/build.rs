use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let picolibc_dir = manifest_dir
        .parent()
        .expect("workspace root")
        .join("third_party/picolibc");

    let string_dir = picolibc_dir.join("newlib/libc/string");
    let include_dir = picolibc_dir.join("newlib/libc/include");
    let ctype_dir = picolibc_dir.join("newlib/libc/ctype");
    let locale_dir = picolibc_dir.join("newlib/libc/locale");

    let sources = [
        "memcpy.c",
        "memcmp.c",
        "memmove.c",
        "memset.c",
        "strlen.c",
        "strcpy.c",
        "strncpy.c",
        "strcmp.c",
    ];

    let mut build = cc::Build::new();
    build.include(&include_dir);
    build.include(&picolibc_dir);
    build.include(&ctype_dir);
    build.include(&locale_dir);
    build.flag_if_supported("-std=c99");

    for source in sources.iter() {
        let path = string_dir.join(source);
        build.file(&path);
        println!("cargo:rerun-if-changed={}", path.display());
    }

    println!("cargo:rerun-if-changed={}", include_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        picolibc_dir.join("COPYING").display()
    );

    build.compile("picolibc_string");
}
