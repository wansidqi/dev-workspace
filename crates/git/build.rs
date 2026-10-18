fn main() {
    // libgit2-sys's own link directives don't pull in advapi32 on this
    // toolchain/linker combo, which left `git2`'s Windows crypto/registry
    // calls (CryptAcquireContextA, RegOpenKeyExW, OpenProcessToken, ...)
    // unresolved when this crate is linked into a standalone binary (its
    // symbols get pulled in transitively by gpui in the `app` binary, which
    // is why `cargo build -p app` didn't show this but `cargo test -p
    // devspace-git` does). Link it explicitly instead of relying on that.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=dylib=advapi32");
    }
}
