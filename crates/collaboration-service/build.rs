fn main() {
    // Stable Rust must rerun migrate! when either owned migration bundle changes.
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=interaction-migrations");
}
