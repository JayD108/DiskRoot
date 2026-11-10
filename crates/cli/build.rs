//! Embeds Windows version information (product name, version, publisher
//! description) into the exe. Code signing (SignPath) checks these fields,
//! and Explorer shows them under Properties > Details.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // DLL planting defence: load every imported DLL from System32 only,
    // never from the exe's own folder (0x800 = LOAD_LIBRARY_SEARCH_SYSTEM32).
    // Matters most for a portable copy in Downloads, because xylem.exe is
    // also the elevated Fast-scan helper.
    println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "Xylem")
        .set("FileDescription", "Xylem command-line disk analyzer")
        .set("CompanyName", "Xylem contributors")
        .set("LegalCopyright", "MIT OR Apache-2.0");
    res.compile().expect("embedding Windows version resource");
}
