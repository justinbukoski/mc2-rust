use std::{env, fs, path::PathBuf};
fn main() {
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../product.env");
    println!("cargo:rerun-if-changed={}", path.display());
    let product = fs::read_to_string(path).expect("read product.env");
    let name = product
        .lines()
        .find_map(|line| line.strip_prefix("PRODUCT_NAME='"))
        .and_then(|value| value.strip_suffix('\''))
        .expect("PRODUCT_NAME must be single-quoted in product.env");
    println!("cargo:rustc-env=MC2_PRODUCT_NAME={name}");
}
