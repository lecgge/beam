#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""release 产物「无 perf 符号」可执行门禁（§7:275 / 差距清单 X7-12、X7-14）。

背景
----
`crates/beam-perf` 的采集实现（`collect.rs`、`Span`、`perf_span!` 的实体分支）
全部由 `#[cfg(feature = "perf-stats")]` 门控；`perf-stats` 默认关闭、release
禁止开启。因此 release 产物里**不应该**出现这些符号。

两种口径，本脚本都做
--------------------
1. **字节级扫描（跨平台，Windows 也可用）**
   直接在 release 产物里找**只可能由 perf-stats 开启产生的符号**：

       - 混淆名（legacy mangling）：`_ZN9beam_perf7collect6recordE`、
         `_ZN9beam_perf4Span3newE` → 关键字 `beam_perf7collect` / `beam_perf4Span`
       - 混淆名（v0 mangling 变体）：`beam_perf_7collect` / `beam_perf_4Span`
       - 反混淆名：`beam_perf::collect` / `beam_perf::Span`

   实测（rustc 1.86 / MSVC，feature 关 vs 开）：

       | 关键字              | feature 关 | feature 开 |
       |---------------------|-----------|-----------|
       | beam_perf7collect   |     0     |    41     |
       | beam_perf4Span      |     0     |     3     |
       | perf_span           |     6     |     7     |  ← 两边都有，不能用
       | beam_perf4Perf      |   有      |   有      |  ← stub，无采集逻辑，不能用

   注意 `perf_span` 宏名在 feature 关闭时**依然存在**（`macro_rules!` 的空实现
   仍然导出），`beam_perf::Perf::*` 也只是空 stub —— 用它们当判据会造成恒失败或
   恒通过，正是本批要修的「门禁恒通过」问题。

2. **`nm -C` 反混淆扫描（文档 §7:275 建议的口径，Linux/macOS 才有 nm）**
   对同一批产物跑 `nm -C --defined-only`，用反混淆后的名字再查一遍。
   `nm` 不存在时自动跳过；`--require-nm` 可强制要求 nm（Linux CI 用）。

关于 `cargo bloat`：它需要 nightly + 额外安装，且比的是体积差而不是符号，
对本仓库「一个 crate 一个 feature」的结构收益有限，故未采用。

退出码：0 = 通过；1 = 门禁失败；2 = 用法/IO 错误。
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from typing import Dict, Iterable, List, Optional, Tuple

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except AttributeError:  # pragma: no cover - Python < 3.7
    pass

#: 字节级关键字：只在 perf-stats 开启时才会出现在产物里。
BYTE_NEEDLES: Tuple[Tuple[str, bytes], ...] = (
    # legacy mangling：_ZN9beam_perf7collect6record17h<E>
    ("beam_perf::collect (mangled)", b"beam_perf7collect"),
    # v0 mangling 变体：beam_perf_7collect
    ("beam_perf::collect (mangled v0)", b"beam_perf_7collect"),
    ("beam_perf::collect (裸名)", b"beam_perfcollect"),
    # legacy mangling：_ZN9beam_perf4Span3new17h<E>
    ("beam_perf::Span (mangled)", b"beam_perf4Span"),
    ("beam_perf::Span (mangled v0)", b"beam_perf_4Span"),
    ("beam_perf::Span (裸名)", b"beam_perfSpan"),
    # 反混淆形式（带调试信息或 nm -C 输出时可见）
    ("beam_perf::collect (demangled)", b"beam_perf::collect"),
    ("beam_perf::Span (demangled)", b"beam_perf::Span"),
)

#: 会扫描的产物后缀。
SCAN_SUFFIXES: Tuple[str, ...] = (
    ".rlib",
    ".rmeta",
    ".dll",
    ".exe",
    ".so",
    ".a",
    ".lib",
    ".dylib",
)

#: 这些子目录不是 release 产物（构建脚本等宿主侧工具），跳过。
SKIP_DIRS: Tuple[str, ...] = ("build", "incremental", ".fingerprint", "tmp")

CHUNK = 4 * 1024 * 1024
OVERLAP = 64  # > 最长关键字长度，保证跨块命中不被切断


class SymbolHit:
    """一次符号命中记录。"""

    def __init__(self, path: str, source: str, needle: str, context: str) -> None:
        self.path = path
        self.source = source  # "bytes" 或 "nm -C"
        self.needle = needle
        self.context = context

    def __str__(self) -> str:  # pragma: no cover - 展示用
        return f"{self.path} [{self.source}] {self.needle} → {self.context}"


def iter_artifacts(target_dir: str) -> List[str]:
    """列出待扫描的产物文件。"""
    found: List[str] = []
    for root, dirs, files in os.walk(target_dir):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for name in files:
            if name.lower().endswith(SCAN_SUFFIXES):
                found.append(os.path.join(root, name))
    return sorted(found)


def scan_bytes(path: str) -> List[SymbolHit]:
    """在二进制产物里做字节级关键字扫描（分块 + 重叠，等价于 ``grep -a``）。

    同一个关键字在一个文件里只报第一次命中，避免大文件里刷屏。
    """
    hits: List[SymbolHit] = []
    seen = set()
    try:
        with open(path, "rb") as fh:
            tail = b""
            while True:
                chunk = fh.read(CHUNK)
                if not chunk:
                    break
                buf = tail + chunk
                for label, needle in BYTE_NEEDLES:
                    if needle in seen:
                        continue
                    idx = buf.find(needle)
                    if idx < 0:
                        continue
                    seen.add(needle)
                    ctx = buf[max(0, idx - 30): idx + len(needle) + 30]
                    ctx_text = re.sub(rb"[^ -~]", b".", ctx).decode("ascii", "replace")
                    hits.append(SymbolHit(path, "bytes", label, ctx_text))
                tail = buf[-OVERLAP:] if len(buf) >= OVERLAP else buf
    except OSError as exc:  # pragma: no cover - 权限/占用等
        print(f"[beam] 跳过无法读取的产物 {path}: {exc}", file=sys.stderr)
    return hits


def scan_nm(paths: List[str], nm: str) -> Tuple[List[SymbolHit], int]:
    """用 ``nm -C`` 反混淆后再查一遍（文档 §7:275 的口径）。

    返回 (命中列表, nm 解析出的符号行数)。符号行数为 0 说明 nm 根本没读到
    符号（例如换了不兼容的 nm 或目标文件格式不被支持），此时 nm 这一路校验
    等于形同虚设 —— 调用方需要据此报错，而不是当作「没有命中」。
    """
    hits: List[SymbolHit] = []
    symbol_lines = 0
    seen = set()
    batch = 100
    for start in range(0, len(paths), batch):
        chunk = paths[start:start + batch]
        cmd = [nm, "-C", "--defined-only"] + chunk
        try:
            proc = subprocess.run(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        except OSError as exc:  # pragma: no cover - nm 执行失败
            print(f"[beam] nm 执行失败：{exc}", file=sys.stderr)
            return hits, symbol_lines
        text = proc.stdout.decode("utf-8", "replace")
        for line in text.splitlines():
            if not line.strip():
                continue
            symbol_lines += 1
            low = line.lower()
            for label, needle in BYTE_NEEDLES:
                if needle in seen:
                    continue
                if needle.decode("ascii").lower() in low:
                    seen.add(needle)
                    hits.append(SymbolHit("(nm -C)", "nm -C", label, line.strip()[:160]))
    return hits, symbol_lines


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="rust_no_perf.py",
        description="检查 release 产物里是否残留 perf 采集符号（§7:275 可执行校验）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例：\n"
            "  # release 必须通过\n"
            "  python rust_no_perf.py --target-dir target/release\n"
            "  # Linux 上再叠加 nm -C 口径（找不到 nm 直接失败）\n"
            "  python3 rust_no_perf.py --target-dir target/release --require-nm\n"
            "  # 负样本自检：feature 开启的产物必须被检出\n"
            "  CARGO_TARGET_DIR=target/tmp/perf-on cargo build --release -p beam-perf --features perf-stats\n"
            "  python rust_no_perf.py --target-dir target/tmp/perf-on/release --expect perf\n"
        ),
    )
    parser.add_argument(
        "--target-dir",
        default=os.path.join("target", "release"),
        help="release 产物目录（默认 target/release）",
    )
    parser.add_argument(
        "--require-nm",
        action="store_true",
        help="要求本机存在 nm 并执行 nm -C 扫描（找不到 nm 则失败）",
    )
    parser.add_argument(
        "--no-nm",
        action="store_true",
        help="跳过 nm -C 扫描，只做字节级扫描",
    )
    parser.add_argument(
        "--expect",
        choices=("clean", "perf", "nm-perf"),
        default="clean",
        help="clean=不许出现 perf 符号（默认，门禁用）；"
        "perf=必须出现（负样本自检用）；"
        "nm-perf=必须由 nm -C 通道检出（Linux 上验证 nm 口径本身有效）",
    )
    parser.add_argument("--quiet", action="store_true", help="只输出结论")
    return parser


def main(argv: Optional[Iterable[str]] = None) -> int:
    args = build_parser().parse_args(list(argv) if argv is not None else None)

    target_dir = args.target_dir
    if not os.path.isdir(target_dir):
        print(f"::error::找不到产物目录：{target_dir}", file=sys.stderr)
        return 2

    artifacts = iter_artifacts(target_dir)
    if not artifacts:
        print(f"::error::{target_dir} 下没有可扫描的产物（rlib/rmeta/dll/exe/so/a）", file=sys.stderr)
        return 2

    nm = None
    if not args.no_nm:
        nm = shutil.which("nm")
        if nm is None and args.require_nm:
            print("::error::找不到 nm，无法执行 nm -C 校验", file=sys.stderr)
            return 2
        if nm is None and not args.quiet:
            print("[beam] 未找到 nm，跳过 nm -C 扫描（仅字节级扫描）")

    if not args.quiet:
        print(f"[beam] 产物目录: {target_dir}")
        print(f"[beam] 待扫描产物: {len(artifacts)} 个")

    hits: List[SymbolHit] = []
    nm_hits: List[SymbolHit] = []
    for path in artifacts:
        hits.extend(scan_bytes(path))
    if nm is not None:
        nm_hits, symbol_lines = scan_nm(artifacts, nm)
        hits.extend(nm_hits)
        if not args.quiet:
            print(f"[beam] nm -C 解析出 {symbol_lines} 行符号")
        if symbol_lines == 0:
            # nm 存在却读不出符号（例如 nm 与目标文件格式不兼容），
            # 这一路校验等于没做 —— 要求 nm 时直接失败，否则只告警。
            message = "::error::nm 没有解析出任何符号 —— nm 这一路校验无效"
            if not args.require_nm:
                message = "::warning::nm 没有解析出任何符号 —— nm 这一路校验无效"
            print(message, file=sys.stderr)
            if args.require_nm:
                return 2

    for hit in hits:
        print(f"::error::release 产物中出现 perf 符号：{hit}")

    if args.expect == "perf":
        if hits:
            if not args.quiet:
                print(f"[beam] 结论: 检出 perf 符号 {len(hits)} 处（负样本符合预期）")
            return 0
        print("::error::期望检出 perf 符号，实际没有命中 —— 门禁可能已经失效", file=sys.stderr)
        return 1

    if args.expect == "nm-perf":
        if nm_hits:
            if not args.quiet:
                print(f"[beam] 结论: nm -C 通道检出 perf 符号 {len(nm_hits)} 处（负样本符合预期）")
            return 0
        print(
            "::error::期望由 nm -C 通道检出 perf 符号，实际没有 —— nm 口径无效",
            file=sys.stderr,
        )
        return 1

    if hits:
        sys.stdout.flush()
        print("::error::release 产物中出现 perf 符号", file=sys.stderr)
        return 1

    if not args.quiet:
        print("[beam] 结论: 通过 —— release 产物中没有 perf 采集符号")
    return 0


if __name__ == "__main__":
    sys.exit(main())
