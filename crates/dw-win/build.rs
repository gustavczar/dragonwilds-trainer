//! Generates the version.dll proxy: one exported jump stub per real export.
//! Single source of truth for the export list; `src/proxy.rs` includes the output.

const EXPORTS: [&str; 17] = [
    "GetFileVersionInfoA",
    "GetFileVersionInfoByHandle",
    "GetFileVersionInfoExA",
    "GetFileVersionInfoExW",
    "GetFileVersionInfoSizeA",
    "GetFileVersionInfoSizeExA",
    "GetFileVersionInfoSizeExW",
    "GetFileVersionInfoSizeW",
    "GetFileVersionInfoW",
    "VerFindFileA",
    "VerFindFileW",
    "VerInstallFileA",
    "VerInstallFileW",
    "VerLanguageNameA",
    "VerLanguageNameW",
    "VerQueryValueA",
    "VerQueryValueW",
];

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut asm = String::new();
    for (i, name) in EXPORTS.iter().enumerate() {
        // Tail jump through the table filled from the real System32 version.dll,
        // so the stub works for any signature.
        asm += &format!(".globl {name}\n{name}:\n    jmp qword ptr [rip + {{real}} + {}]\n", i * 8);
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
    }
    std::fs::write(out.join("proxy.s"), asm).unwrap();
    let names = EXPORTS.iter().map(|n| format!("\"{n}\\0\"")).collect::<Vec<_>>().join(", ");
    std::fs::write(out.join("exports.rs"), format!("pub const NAMES: [&str; {}] = [{names}];\n", EXPORTS.len())).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
