use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct TestStruct {
    field: String,
}

fn main() {
    let test = TestStruct {
        field: "hello".to_string(),
    };
    println!("{:?}", test);
}
