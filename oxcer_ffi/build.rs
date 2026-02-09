// Set the dylib install name so the app can load it from the bundle via runpath
// (e.g. @executable_path/../PlugIns) without hard-coded absolute paths.
fn main() {
    if std::env::var("TARGET").map_or(false, |t| t.contains("apple-darwin")) {
        println!("cargo:rustc-link-arg=-Wl,-install_name,@rpath/liboxcer_ffi.dylib");
    }
}
