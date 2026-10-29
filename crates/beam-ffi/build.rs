//! 构建脚本（§3.12 uniffi 导出面）。
//!
//! ## 为什么本脚本不生成 scaffolding
//!
//! 本仓库用的是 uniffi 0.29 的 **proc-macro 模式**：scaffolding 由
//! `uniffi::setup_scaffolding!()`（`src/lib.rs`）在编译期生成，
//! 所以 build.rs 没有「生成 scaffolding」这一步。
//!
//! 备选是 uniffi 的 **UDL 模式**（`uniffi::generate_scaffolding("src/beam_ffi.udl")`），
//! 但那条路径会引入 `uniffi` 的 `build` feature → `uniffi_build` → `uniffi_bindgen`
//! → `uniffi_udl` → `textwrap 0.16`；而根 `Cargo.lock` 里锁定的 `textwrap 0.16.4`
//! 声明 `rust-version = 1.88`，高于本仓库 MSRV（1.86），cargo 会直接拒绝编译。
//! 要改走 UDL 模式，需要在根 `Cargo.lock` 里把 `textwrap` 钉到 `0.16.2`
//! （`cargo update -p textwrap --precise 0.16.2`），这属于根清单改动，不在本 crate 范围内。
//!
//! ## 本脚本做的事
//!
//! 1. 声明重编译依赖：导出面源码变了要重新编译；
//! 2. 导出 uniffi 契约版本号（`BEAM_FFI_UNIFFI_CONTRACT_VERSION`），
//!    供 Android 构建脚本校验「生成的 Kotlin 绑定」与「`libbeam_ffi.so`」来自同一契约版本。
//!
//! Kotlin 绑定的生成命令见 `android/core-net/README.md` §2.2：
//! `uniffi-bindgen generate --library <libbeam_ffi.so> --language kotlin --out-dir …`。

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/client.rs");
    println!("cargo:rerun-if-changed=src/error.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    // uniffi 0.29 的契约版本 = 29（与根清单里的 `uniffi = "0.29"` 对应）。
    println!("cargo:rustc-env=BEAM_FFI_UNIFFI_CONTRACT_VERSION=29");
}
