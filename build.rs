use std::env;

fn main() {
    let target = env::var("TARGET").unwrap();

    match &target[..] {
        "thumbv6m-none-eabi"
        | "thumbv7m-none-eabi"
        | "thumbv7em-none-eabi"
        | "thumbv7em-none-eabihf"
        | "thumbv8m.base-none-eabi"
        | "thumbv8m.main-none-eabi"
        | "thumbv8m.main-none-eabihf" => println!("cargo:rustc-cfg=cortex_m"),
        _ => {}
    }
}
