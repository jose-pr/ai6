use std::{env, fs, path::Path};

/// Writes each `lib/<name>.lib.hex` below `OUT_DIR` as `<name>.lib` and points the linker there.
fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = env::var("OUT_DIR").unwrap();
    for entry in fs::read_dir(Path::new(&dir).join("lib")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(lib) = name.strip_suffix(".hex") else { continue };
        println!("cargo:rerun-if-changed={}", path.display());
        let text = fs::read_to_string(&path).unwrap();
        let digits: Vec<u8> = text.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
        let bytes: Vec<u8> = digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        fs::write(Path::new(&out).join(lib), bytes).unwrap();
    }
    println!("cargo:rustc-link-search=native={out}");
}
