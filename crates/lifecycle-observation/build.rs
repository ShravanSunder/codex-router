fn main() {
    // Stable Rust must rerun migrate! when a new migration file is added.
    println!("cargo:rerun-if-changed=migrations");
}
