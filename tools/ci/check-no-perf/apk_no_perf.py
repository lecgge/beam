#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""release APK「无 perf 类」可执行门禁（§7:276 / 差距清单 X7-15、X13-14）。

为什么不能用原来的 `unzip -l "$APK" | grep -i "feature-perf\\|perfstats"`：

    `unzip -l` 列出的是 APK 的 **zip 条目名**，而 JVM 类名存放在 `classes*.dex`
    的字符串池里，根本不出现在条目名中。实测 release APK 有 419 个条目
    （`classes.dex`、`lib/arm64-v8a/*.so`、`res/**`、`assets/dexopt/*` …），
    上面那条 grep 无论构建成什么样都是 0 命中 —— 门禁恒通过。

本脚本改成对 **dex 内的类名字符串** 做扫描（对应 `grep -a` 扫二进制）：

    1. 解压流式读取每个 `classes*.dex` 条目到内存（不落盘、不解压到临时目录），
       在字节流里匹配 perf 命名空间与 perf 类型名 —— 命中即失败（硬门禁）。
    2. 交叉检查 R8 映射表：把 mapping.txt 里的 perf 类解析成混淆名，再看混淆名
       是否出现在 dex 里。R8 会**改名保留**仍然可达的类，所以只按原名扫 dex 会漏；
       这一层能识别「类其实还在 release 里，只是被混淆了」。

关于「多个 APK」（引入 store / internal 两个 flavor 之后）：

    ``--apk`` / ``--mapping`` 都接受**多个**参数，且 **glob 会展开成全部匹配**。
    旧版本 ``--apk`` 只取 glob 的第一个匹配 —— flavor 只有一个时看不出问题，
    变成两个之后就会静默漏检一个（store 通过、internal 里的 perf 类根本没人看），
    门禁等于从「恒通过」退化成「随机通过」。因此这里对每个 APK 逐一判定。

    mapping 与 APK 的配对规则：

    - 不给 ``--mapping``：只扫 dex；
    - 给 1 个 mapping：应用到所有 APK；
    - 给 N 个 mapping 且 N == APK 数：按顺序 1:1 配对（store / internal 各用自己的）。

关于 mapping.txt 的判定分两档（理由见下面常量旁的注释）：

    - `dev.beam.feature.perf.*`（`:feature-perf` 模块，debug 专属）
      混淆名出现在 dex 里 = release 构建把 perf 模块打进包了 → **硬失败**。
    - `dev.beam.core.data.perf.*` / `dev.beam.core.net.Perf*` / `dev.beam.app.perf.*`
      来自 `:core-data` / `:core-net` / `:app` 这些**始终参与 release** 的模块，
      它们在 release 里存在是现状设计问题（要彻底去掉必须改 android/ 侧，
      例如把性能面板挂到 `BuildConfig.PERF_STATS` 开关后面），
      本脚本只**告警并列出类名**，不硬失败。

退出码：0 = 通过；1 = 门禁失败；2 = 用法/IO 错误。
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys
import zipfile
from typing import Dict, Iterable, List, Optional, Tuple

# Windows 控制台默认 GBK，输出中文/类名可能触发 UnicodeEncodeError。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except AttributeError:  # pragma: no cover - Python < 3.7
    pass

# ---------------------------------------------------------------------------
# 禁用模式（对 dex 字节流做大小写不敏感匹配）
# ---------------------------------------------------------------------------

#: perf 命名空间（斜杠形式：dex 里类描述符形如 Ldev/beam/.../X;）。
#: 全部是**字面量**（不是正则）：大小写不敏感靠 lower() 后 find() 实现。
NAMESPACE_PATTERNS: Tuple[Tuple[str, bytes], ...] = (
    # §7：`:feature-perf` 只在 debug —— 这一组是 release 绝对不该有的。
    ("dev/beam/feature/perf", b"dev/beam/feature/perf"),
    ("dev.beam.feature.perf", b"dev.beam.feature.perf"),
    # 核心层 perf 数据模型 / 网络埋点。
    ("dev/beam/core/data/perf", b"dev/beam/core/data/perf"),
    ("dev.beam.core.data.perf", b"dev.beam.core.data.perf"),
    ("dev/beam/core/net/Perf", b"dev/beam/core/net/perf"),
    ("dev.beam.core.net.Perf", b"dev.beam.core.net.perf"),
    # app 层性能面板入口。
    ("dev/beam/app/perf", b"dev/beam/app/perf"),
    ("dev.beam.app.perf", b"dev.beam.app.perf"),
    # 文档 §7:276 里出现过的原始模式，保留以便对照。
    ("feature-perf", b"feature-perf"),
    ("perfstats", b"perfstats"),
)

#: perf 类型名（不含命名空间时也要能抓到）。
TYPE_PATTERNS: Tuple[Tuple[str, bytes], ...] = (
    ("PerfSnapshotStore", rb"PerfSnapshotStore"),
    ("PerfExportFormat", rb"PerfExportFormat"),
    ("PerfRecorder", rb"PerfRecorder"),
)

#: 已知**不**纳入禁用集的类型，以及原因（避免后人"顺手补上"把门禁弄红）：
#:
#: `PerfUiState` 是 `:core-data` 里一个 Kotlin data class，编译器生成的
#: `toString()` 会把 `"PerfUiState(fps=0.0, bitrateKbps=0.0, ...)"` 当成
#: **字符串常量**写进 dex。R8 混淆的是类名，改不了这个字面量，所以 release
#: dex 里必然残留 1 处 `PerfUiState` 命中 —— 它不代表 perf 采集代码进了包。
KNOWN_EXCLUSIONS: Tuple[Tuple[str, str], ...] = (
    ("PerfUiState", "Kotlin data class 的 toString() 字面量，R8 混淆不影响，非采集代码"),
    ("perform*", "AndroidX/Compose 的方法名，与 perf 无关"),
    ("performanceMode", "CameraX 的相机模式常量，与 perf 无关"),
)

ALL_PATTERNS: Tuple[Tuple[str, bytes], ...] = NAMESPACE_PATTERNS + TYPE_PATTERNS

#: mapping.txt 的类级行：`原始类名 -> 混淆名:`（行首无空白）。
MAPPING_CLASS_RE = re.compile(r"^(\S+) -> (\S+):\s*$")

#: 只有这些命名空间在 release 中出现才算「硬失败」。
FEATURE_PERF_NAMESPACE = "dev.beam.feature.perf"


class DexHit:
    """一次 dex 命中记录。"""

    def __init__(self, dex_name: str, pattern: str, context: str) -> None:
        self.dex_name = dex_name
        self.pattern = pattern
        self.context = context

    def __str__(self) -> str:  # pragma: no cover - 展示用
        return f"{self.dex_name} 命中 {self.pattern} → {self.context}"


class MappingHit:
    """mapping.txt 中一个 perf 类。"""

    def __init__(self, original: str, obfuscated: str, in_dex: bool) -> None:
        self.original = original
        self.obfuscated = obfuscated
        self.in_dex = in_dex

    @property
    def is_feature_perf(self) -> bool:
        return self.original.startswith(FEATURE_PERF_NAMESPACE)

    def __str__(self) -> str:  # pragma: no cover - 展示用
        state = "仍存在于 dex" if self.in_dex else "已被 R8 移除"
        return f"{self.original} -> {self.obfuscated}（{state}）"


def _read_dex_blobs(apk_path: str) -> Dict[str, bytes]:
    """流式读取 APK 里的 ``classes*.dex`` 条目到内存（不解压到磁盘）。"""
    blobs: Dict[str, bytes] = {}
    with zipfile.ZipFile(apk_path) as zf:
        for info in zf.infolist():
            if info.is_dir():
                continue
            name = info.filename
            if not re.match(r"^classes\d*\.dex$", name):
                continue
            with zf.open(info) as fh:
                blobs[name] = fh.read()
    return blobs


def scan_dex(blobs: Dict[str, bytes]) -> List[DexHit]:
    """在 dex 字节流里扫描 perf 类名（等价于 ``grep -a -i -E``）。"""
    hits: List[DexHit] = []
    for dex_name, data in blobs.items():
        lower = data.lower()
        for label, pattern in ALL_PATTERNS:
            needle = pattern.lower()
            idx = lower.find(needle)
            if idx < 0:
                continue
            ctx = data[max(0, idx - 40): idx + len(pattern) + 40]
            # 只保留可打印字符，避免把二进制噪声打进日志。
            ctx_text = re.sub(rb"[^ -~]", b".", ctx).decode("ascii", "replace")
            hits.append(DexHit(dex_name, label, ctx_text))
    return hits


def parse_mapping(mapping_path: str) -> List[MappingHit]:
    """解析 R8 mapping.txt，取出所有 perf 类及其混淆名。"""
    hits: List[MappingHit] = []
    with open(mapping_path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = MAPPING_CLASS_RE.match(line.rstrip("\n"))
            if not m:
                continue
            original, obfuscated = m.group(1), m.group(2)
            if not _is_perf_class(original):
                continue
            hits.append(MappingHit(original, obfuscated, False))
    return hits


def _is_perf_class(class_name: str) -> bool:
    """判断一个类名是否属于 perf 家族。"""
    if class_name.startswith(FEATURE_PERF_NAMESPACE):
        return True
    for prefix in (
        "dev.beam.core.data.perf.",
        "dev.beam.core.net.Perf",
        "dev.beam.app.perf.",
    ):
        if class_name.startswith(prefix):
            return True
    return False


def _descriptor(obfuscated: str) -> bytes:
    """混淆类名 → dex 类型描述符，例如 ``q3.b`` → ``Lq3/b;``。"""
    return b"L" + obfuscated.replace(".", "/").encode("ascii", "replace") + b";"


def check_mapping(mapping_path: str, blobs: Dict[str, bytes]) -> Tuple[List[MappingHit], List[MappingHit]]:
    """返回 (硬失败项, 告警项)。

    硬失败：``dev.beam.feature.perf.*`` 的混淆名出现在 dex 里 —— 说明
    debug 专属的 ``:feature-perf`` 模块被打进了 release 包。
    告警：其它 perf 命名空间（来自始终参与 release 的模块）被混淆保留在包里。
    """
    hard: List[MappingHit] = []
    soft: List[MappingHit] = []
    for hit in parse_mapping(mapping_path):
        if hit.obfuscated != hit.original:
            desc = _descriptor(hit.obfuscated)
            hit.in_dex = any(desc in blob for blob in blobs.values())
        else:
            # 没被混淆 = 原名就在包里，dex 扫描那层已经会抓到。
            hit.in_dex = True
        if not hit.in_dex:
            continue
        (hard if hit.is_feature_perf else soft).append(hit)
    return hard, soft


def _resolve_apks(pattern: str) -> List[str]:
    """把可能是 glob 的路径解析成**全部**匹配的文件列表。

    旧实现只取 ``matches[0]``，多 flavor 时会静默漏检。这里返回全部匹配，
    由调用方对每个 APK 逐一判定。
    """
    if os.path.isfile(pattern):
        return [pattern]
    matches = sorted(glob.glob(pattern))
    if not matches:
        raise FileNotFoundError(f"找不到 APK：{pattern}")
    return matches


def _resolve_mappings(patterns: List[str]) -> List[str]:
    """解析 mapping 参数：允许普通路径或 glob。"""
    resolved: List[str] = []
    for pattern in patterns:
        if os.path.isfile(pattern):
            resolved.append(pattern)
            continue
        matches = sorted(glob.glob(pattern))
        if not matches:
            raise FileNotFoundError(f"找不到 R8 映射表：{pattern}")
        resolved.extend(matches)
    return resolved


def _pair_mappings(
    apks: List[str], mappings: List[str]
) -> List[Optional[str]]:
    """把 mapping 与 APK 配对，返回与 ``apks`` 等长的列表（元素可为 None）。"""
    if not mappings:
        return [None] * len(apks)
    if len(mappings) == 1:
        return [mappings[0]] * len(apks)
    if len(mappings) == len(apks):
        return list(mappings)
    raise ValueError(
        f"--mapping 给了 {len(mappings)} 个，但 APK 有 {len(apks)} 个："
        f"要么只给 1 个（应用到全部），要么给等量（1:1 配对）"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="apk_no_perf.py",
        description="检查 APK 的 classes*.dex 中是否残留 perf 类（§7:276 可执行校验）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例：\n"
            "  # release 必须通过（store / internal 两个 flavor 一起查）\n"
            "  python apk_no_perf.py \\\n"
            "      --apk 'app/build/outputs/apk/store/release/*.apk' \\\n"
            "            'app/build/outputs/apk/internal/release/*.apk' \\\n"
            "      --mapping 'app/build/outputs/mapping/storeRelease/mapping.txt' \\\n"
            "               'app/build/outputs/mapping/internalRelease/mapping.txt'\n"
            "  # debug 必须被抓到（门禁自检 / 负样本）\n"
            "  python apk_no_perf.py \\\n"
            "      --apk 'app/build/outputs/apk/store/debug/*.apk' \\\n"
            "            'app/build/outputs/apk/internal/debug/*.apk' --expect perf\n"
        ),
    )
    parser.add_argument(
        "--apk",
        required=True,
        nargs="+",
        help="APK 路径或 glob（可给多个；glob 展开成全部匹配，逐个判定）",
    )
    parser.add_argument(
        "--mapping",
        default=None,
        nargs="+",
        help="R8 mapping.txt 路径（可选；给 1 个则应用到全部 APK，"
        "给等量则按顺序 1:1 配对）",
    )
    parser.add_argument(
        "--expect",
        choices=("clean", "perf"),
        default="clean",
        help="clean=不许出现 perf（默认，门禁用）；perf=必须出现 perf（负样本自检用）",
    )
    parser.add_argument(
        "--strict-mapping",
        action="store_true",
        help="把 mapping 告警升级为硬失败（需要 android/ 侧先把性能面板关进 "
        "BuildConfig.PERF_STATS 开关之后才能开启）",
    )
    parser.add_argument("--quiet", action="store_true", help="只输出结论")
    return parser


def _check_one_apk(
    apk_path: str,
    mapping_path: Optional[str],
    args: argparse.Namespace,
) -> Tuple[bool, int]:
    """检查单个 APK，返回 (是否命中 perf, dex 命中处数)。

    ``mapping_path`` 为 None 时只扫 dex。
    """
    blobs = _read_dex_blobs(apk_path)
    if not blobs:
        raise LookupError(f"APK 内没有 classes*.dex：{apk_path}")

    if not args.quiet:
        print(f"[beam] APK: {apk_path}")
        print(f"[beam] dex 条目: {len(blobs)} 个 → {', '.join(sorted(blobs))}")

    dex_hits = scan_dex(blobs)

    hard_mapping: List[MappingHit] = []
    soft_mapping: List[MappingHit] = []
    if mapping_path and os.path.isfile(mapping_path):
        hard_mapping, soft_mapping = check_mapping(mapping_path, blobs)
    elif mapping_path and not args.quiet:
        print(f"[beam] 未找到 R8 映射表（跳过交叉验证）：{mapping_path}")

    found = bool(dex_hits) or bool(hard_mapping)
    for hit in dex_hits:
        print(
            f"::error::{os.path.basename(apk_path)} 的 {hit.dex_name} 中出现 perf 类："
            f"{hit.pattern} → {hit.context}"
        )
    for hit in hard_mapping:
        print(
            f"::error::R8 映射表显示 debug 专属的 perf 模块被打进 {os.path.basename(apk_path)}：{hit}"
        )

    if soft_mapping:
        level = "error" if args.strict_mapping else "warning"
        print(
            f"::{level}::R8 映射表显示以下 perf 类被混淆保留在 "
            f"{os.path.basename(apk_path)} 中"
            f"（来自始终参与 release 的模块，非 :feature-perf）："
        )
        for hit in soft_mapping:
            print(f"::{level}::  {hit}")

    return found, len(dex_hits)


def main(argv: Optional[Iterable[str]] = None) -> int:
    args = build_parser().parse_args(list(argv) if argv is not None else None)

    # --- 解析 APK（glob 展开成全部匹配，避免多 flavor 时静默漏检）---
    apks: List[str] = []
    try:
        for pattern in args.apk:
            for path in _resolve_apks(pattern):
                if path not in apks:
                    apks.append(path)
    except FileNotFoundError as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 2

    # --- 解析 mapping 并与 APK 配对 ---
    try:
        mappings = _resolve_mappings(list(args.mapping or []))
        paired = _pair_mappings(apks, mappings)
    except (FileNotFoundError, ValueError) as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 2

    if not args.quiet:
        print(f"[beam] 待检查 APK {len(apks)} 个：{', '.join(os.path.basename(a) for a in apks)}")

    results: List[Tuple[str, bool, int]] = []
    for apk_path, mapping_path in zip(apks, paired):
        try:
            found, hit_count = _check_one_apk(apk_path, mapping_path, args)
        except LookupError as exc:
            print(f"::error::{exc}", file=sys.stderr)
            return 2
        results.append((apk_path, found, hit_count))

    # --- 汇总：每个 APK 都要满足期望（clean = 全干净；perf = 全命中）---
    if args.expect == "perf":
        missed = [os.path.basename(p) for p, found, _ in results if not found]
        if missed:
            print(
                "::error::期望检出 perf 类，以下 APK 没有命中 —— 门禁可能已经失效："
                + ", ".join(missed),
                file=sys.stderr,
            )
            return 1
        total = sum(c for _, _, c in results)
        if not args.quiet:
            print(f"[beam] 结论: 全部 {len(results)} 个 APK 均检出 perf（合计 {total} 处，负样本符合预期）")
        return 0

    dirty = [os.path.basename(p) for p, found, _ in results if found]
    if dirty:
        sys.stdout.flush()
        print(
            "::error::以下 APK 中出现 perf 相关类：" + ", ".join(dirty),
            file=sys.stderr,
        )
        return 1

    if not args.quiet:
        print(f"[beam] 结论: 通过 —— {len(results)} 个 APK 的 dex 中都没有 perf 类")
    return 0


if __name__ == "__main__":
    sys.exit(main())
