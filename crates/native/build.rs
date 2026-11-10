//! Embeds Windows version information (product name, version, publisher
//! description) into the exe. Code signing (SignPath) checks these fields,
//! and Explorer shows them under Properties > Details.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "Xylem")
        .set("FileDescription", "Xylem disk space analyzer")
        .set("CompanyName", "Xylem contributors")
        .set("LegalCopyright", "MIT OR Apache-2.0");
    res.compile().expect("embedding Windows version resource");
}
