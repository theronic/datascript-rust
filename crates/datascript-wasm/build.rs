//! How the WebAssembly module is linked, wherever it is built from.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        // Asked of whatever of this crate's is linked: the module, and the module built into another program
        // (examples/embedded.rs). A program in a crate of its own asks for both in its own build script: what a
        // build script asks for is asked of its own crate alone.
        //
        // Queries, pulls and transactions recurse as deep as their data nests: 8 MB of stack in the module's memory.
        println!("cargo:rustc-link-arg=-zstack-size=8388608");
        // The host puts the stack back where it was when a call does not return: when the host's own stack ran out
        // under it, or a trap ended it.
        println!("cargo:rustc-link-arg=--export=__stack_pointer");
    }
}
