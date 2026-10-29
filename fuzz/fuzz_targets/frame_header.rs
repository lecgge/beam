//! 模糊目标：16 字节帧头解析。
//!
//! 目标：任意字节流都不得让帧头解析 panic；长度不足 16 字节时必须返回错误。
//!
//! 运行（需要 nightly toolchain 与 cargo-fuzz）：
//! ```text
//! cargo +nightly fuzz run frame_header
//! ```

#![no_main]

use beam_proto::VideoPktHeader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = VideoPktHeader::decode(data);
});
