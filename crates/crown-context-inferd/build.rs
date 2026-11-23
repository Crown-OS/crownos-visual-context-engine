//! The package installs libllama and libggml next to the ggml device modules, in
//! /usr/lib/crown-context; the binary finds them there without a system library path.
//!
//! A development build has no such directory. `cargo run` and `cargo test` set the library path
//! for it. Started any other way (by a user unit in a test, for example), it needs
//! `LD_LIBRARY_PATH` set to the `out/lib` directory of the llama-cpp-sys-2 build in the target
//! directory. Cargo gives this build script no reliable path to that directory, so the rpath
//! does not name it.
fn main() {
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../lib/crown-context");
}
