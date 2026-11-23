# Recipes for this repository. `just check` is the gate CI runs.

target := env("CARGO_TARGET_DIR", "target")

# llama-cpp-sys-2 (dynamic-link) leaves lib*.so links to versioned files it does not copy;
# a later build-script run panics on them. Run this before every build.
clean-llama-links:
    find "{{target}}" -maxdepth 3 -xtype l -name 'lib*.so' -delete

check: clean-llama-links
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# crown-context-inferd with the Vulkan ggml module, as the package builds it.
build-inferd: clean-llama-links
    cargo build --release -p crown-context-inferd --features vulkan
