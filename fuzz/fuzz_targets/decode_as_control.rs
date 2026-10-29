//! 模糊目标：定类型解码路径（`decode_as::<ControlMsg>`）。
//!
//! 目标：与 `decode_any` 相同——任意字节都不得让解码器 panic。
//! 额外关注 `decode_as` 的族标签校验：族不匹配时必须干净地返回错误。
//!
//! 运行（需要 nightly toolchain 与 cargo-fuzz）：
//! ```text
//! cargo +nightly fuzz run decode_as_control
//! ```

#![no_main]

use beam_proto::{decode_as, ControlMsg};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decode_as::<ControlMsg>(data);
});
