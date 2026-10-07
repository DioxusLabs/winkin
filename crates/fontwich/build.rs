//! Chooses how libfontconfig is reached.
//!
//! Deliberately an environment variable and not a Cargo feature. Features
//! unify across a build: one crate enabling `fontconfig-sys/dlopen` turns it
//! on for every crate in the graph, including ones that are linking against
//! fontconfig and need the symbols to exist. There is no way to express
//! "dlopen for me, linked for you" through a feature, so whichever side loses
//! gets a compile error. `RUST_FONTCONFIG_DLOPEN` is what `fontconfig-sys`
//! reads in its own build script for exactly this reason, so reading the same
//! variable keeps the two in step without our ever naming the feature.
//!
//! Unset, the default, means linking — which needs libfontconfig and its
//! pkg-config file at build time. Set to anything at all means loading at
//! runtime, which needs neither.

//!
//! It also names two platform conditions the sources test often:
//! `fontwich_fontconfig` for the fontconfig backend (Linux and the BSDs with
//! `system`), and `fontwich_android` for the Android backend (the `android`
//! feature, or an Android target with `system`).

use std::env;

fn main() {
    println!("cargo::rerun-if-env-changed=RUST_FONTCONFIG_DLOPEN");
    println!("cargo::rustc-check-cfg=cfg(fontconfig_dlopen)");
    println!("cargo::rustc-check-cfg=cfg(fontwich_fontconfig)");
    println!("cargo::rustc-check-cfg=cfg(fontwich_android)");
    if env::var_os("RUST_FONTCONFIG_DLOPEN").is_some() {
        println!("cargo::rustc-cfg=fontconfig_dlopen");
    }

    let feature = |name: &str| env::var_os(format!("CARGO_FEATURE_{name}")).is_some();
    let target = |key: &str| env::var(format!("CARGO_CFG_TARGET_{key}")).unwrap_or_default();
    let unix = env::var_os("CARGO_CFG_UNIX").is_some();
    let apple = target("VENDOR") == "apple";
    let android = target("OS") == "android";
    if unix && !apple && !android && feature("SYSTEM") {
        println!("cargo::rustc-cfg=fontwich_fontconfig");
    }
    if feature("ANDROID") || (android && feature("SYSTEM")) {
        println!("cargo::rustc-cfg=fontwich_android");
    }
}
