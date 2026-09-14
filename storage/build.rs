fn main() {
    // migrate! tracks existing files, not the addition of a previously unknown file.
    println!("cargo::rerun-if-changed=migrations");
}
