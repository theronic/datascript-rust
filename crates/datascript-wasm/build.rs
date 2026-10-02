//! How the WebAssembly module is linked, wherever it is built from.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        // Queries, pulls and transactions recurse as deep as their data nests: 8 MB of stack in the module's memory.
        println!("cargo:rustc-cdylib-link-arg=-zstack-size=8388608");
        // The host puts the stack back where it was when a call does not return: when the host's own stack ran out
        // under it, or a trap ended it.
        println!("cargo:rustc-cdylib-link-arg=--export=__stack_pointer");
    }
}
