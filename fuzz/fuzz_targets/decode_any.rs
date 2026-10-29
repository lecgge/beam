//! 模糊目标：`AnyMsg` 分派解码路径。
//!
//! 目标：解码器面对**任意**字节都只能返回 `Ok` 或 `Err`，绝不能 panic。
//! libFuzzer 会持续变异输入，一旦解码器 panic 就会在 `fuzz/artifacts/` 下
//! 留下可复现的崩溃样本。
//!
//! 运行（需要 nightly toolchain 与 cargo-fuzz）：
//! ```text
//! cargo +nightly fuzz run decode_any
//! ```

#![no_main]

use beam_proto::decode;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 结果是 Err 还是 Ok 都合法，唯独不能 panic。
    let _ = decode(data);
});
