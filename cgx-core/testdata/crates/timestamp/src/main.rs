fn main() {
    println!("Hello, world!");
    println!("The source directory was: {}", env!("CARGO_MANIFEST_DIR"));

    print!("Features enabled: ");
    let mut features = Vec::new();
    if cfg!(feature = "frobnulator") {
        features.push("frobnulator");
    }
    if cfg!(feature = "gonkolator") {
        features.push("gonkolator");
    }
    if features.is_empty() {
        println!("none");
    } else {
        println!("{}", features.join(", "));
    }

    println!("Built at: {}", env!("BUILD_TIMESTAMP"));
}
