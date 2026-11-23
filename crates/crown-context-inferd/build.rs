//! The package installs libllama and libggml next to the ggml device modules, in
//! /usr/lib/crown-context; the binary finds them there without a system library path.
fn main() {
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../lib/crown-context");
}
