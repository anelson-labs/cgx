// This is the standard test program in the crx test crates.  They all pull in this
// source file for consistency.
//
// It prints out information that provides a hint as to when it was compiled and from where, which
// helps our tests.  Otherwise it has no purpose.

fn main() {
    println!("Hello, world!");
    println!("The source directory was: {}", env!("CARGO_MANIFEST_DIR"));
}
