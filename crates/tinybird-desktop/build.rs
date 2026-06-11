fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bin=tinybird=/STACK:16777216"),
        Ok("gnu") => println!("cargo:rustc-link-arg-bin=tinybird=-Wl,--stack,16777216"),
        _ => {}
    }
}
