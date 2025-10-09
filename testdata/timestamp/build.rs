fn main() {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", timestamp);
    println!("cargo:rerun-if-changed=build.rs");
}
