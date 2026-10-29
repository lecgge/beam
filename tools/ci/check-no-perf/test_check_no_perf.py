#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""tools/ci/check-no-perf 的单元测试。

目的：证明两个检查器**不是空转**——本批的核心问题就是旧门禁「怎么构建都通过」，
所以必须有一个能独立验证「检查器能抓到东西」的测试。

运行：

    python tools/ci/check-no-perf/test_check_no_perf.py

（只依赖标准库，不需要 cargo / gradle / APK 产物。）
"""

from __future__ import annotations

import io
import os
import stat
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import apk_no_perf  # noqa: E402
import rust_no_perf  # noqa: E402


def _write_apk(path: str, dex_payload: bytes) -> str:
    """构造一个最小 APK：含一个 classes.dex 与若干无关条目。"""
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("classes.dex", dex_payload)
        zf.writestr("res/layout/main.xml", b"whatever")
        zf.writestr("lib/arm64-v8a/libbeam.so", b"\x7fELF fake")
    return path


class ApkCheckTests(unittest.TestCase):
    def test_clean_apk_has_no_hit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(
                os.path.join(tmp, "clean.apk"),
                b"hello world, no perf classes here at all",
            )
            blobs = apk_no_perf._read_dex_blobs(apk)
            self.assertEqual(sorted(blobs), ["classes.dex"])
            self.assertEqual(apk_no_perf.scan_dex(blobs), [])

    def test_feature_perf_namespace_is_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(
                os.path.join(tmp, "dirty.apk"),
                b"xxLdev/beam/feature/perf/PerfRecorder;yy",
            )
            hits = apk_no_perf.scan_dex(apk_no_perf._read_dex_blobs(apk))
            self.assertTrue(hits)
            self.assertEqual(hits[0].pattern, "dev/beam/feature/perf")

    def test_core_perf_namespaces_are_flagged(self) -> None:
        payloads = (
            b"Ldev/beam/core/data/perf/PerfSnapshotStore;",
            b"Ldev/beam/core/net/PerfTick;",
            b"Ldev/beam/app/perf/PerfEntry;",
            b"dev.beam.feature.perf.PerfPanel",
        )
        for payload in payloads:
            with self.subTest(payload=payload):
                with tempfile.TemporaryDirectory() as tmp:
                    apk = _write_apk(os.path.join(tmp, "dirty.apk"), payload)
                    hits = apk_no_perf.scan_dex(apk_no_perf._read_dex_blobs(apk))
                    self.assertTrue(hits, f"未检出：{payload!r}")

    def test_perf_type_names_are_flagged(self) -> None:
        for name in ("PerfSnapshotStore", "PerfExportFormat", "PerfRecorder"):
            with self.subTest(name=name):
                with tempfile.TemporaryDirectory() as tmp:
                    apk = _write_apk(
                        os.path.join(tmp, "dirty.apk"),
                        f"Ldev/beam/x/{name};".encode("ascii"),
                    )
                    hits = apk_no_perf.scan_dex(apk_no_perf._read_dex_blobs(apk))
                    self.assertTrue(hits, f"未检出：{name}")

    def test_perf_ui_state_literal_is_not_flagged(self) -> None:
        """Kotlin data class 的 toString() 字面量不是采集代码，不应误报。"""
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(
                os.path.join(tmp, "app.apk"),
                b"PerfUiState(fps=0.0, bitrateKbps=0.0, lossPercent=0.0)",
            )
            self.assertEqual(apk_no_perf.scan_dex(apk_no_perf._read_dex_blobs(apk)), [])

    def test_mapping_feature_perf_surviving_is_hard_failure(self) -> None:
        """:feature-perf 被混淆保留在 dex 里 → 硬失败。"""
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(os.path.join(tmp, "app.apk"), b"...La/b;...")
            mapping = os.path.join(tmp, "mapping.txt")
            with open(mapping, "w", encoding="utf-8") as fh:
                fh.write("dev.beam.feature.perf.PerfPanel -> a.b:\n")
            hard, soft = apk_no_perf.check_mapping(mapping, apk_no_perf._read_dex_blobs(apk))
            self.assertEqual(len(hard), 1)
            self.assertEqual(soft, [])

    def test_mapping_core_perf_surviving_is_warning_only(self) -> None:
        """core/app 层的 perf 类被保留 → 只告警（它们来自始终参与 release 的模块）。"""
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(os.path.join(tmp, "app.apk"), b"...Lq3/b;...")
            mapping = os.path.join(tmp, "mapping.txt")
            with open(mapping, "w", encoding="utf-8") as fh:
                fh.write("dev.beam.core.data.perf.PerfSnapshotStore -> q3.b:\n")
            hard, soft = apk_no_perf.check_mapping(mapping, apk_no_perf._read_dex_blobs(apk))
            self.assertEqual(hard, [])
            self.assertEqual(len(soft), 1)
            self.assertTrue(soft[0].in_dex)

    def test_mapping_removed_class_is_not_reported(self) -> None:
        """R8 已移除（不在 dex 里）的 perf 类不算命中。"""
        with tempfile.TemporaryDirectory() as tmp:
            apk = _write_apk(os.path.join(tmp, "app.apk"), b"nothing here")
            mapping = os.path.join(tmp, "mapping.txt")
            with open(mapping, "w", encoding="utf-8") as fh:
                fh.write("dev.beam.core.data.perf.PerfSnapshotStore -> q3.b:\n")
            hard, soft = apk_no_perf.check_mapping(mapping, apk_no_perf._read_dex_blobs(apk))
            self.assertEqual(hard, [])
            self.assertEqual(soft, [])


class RustCheckTests(unittest.TestCase):
    def test_clean_binary_has_no_hit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "libfoo.rlib")
            with open(path, "wb") as fh:
                fh.write(b"beam_perf::Perf::record beam_perf::Perf::export nothing")
            self.assertEqual(rust_no_perf.scan_bytes(path), [])

    def test_mangled_collect_symbol_is_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "libbeam_perf.rlib")
            with open(path, "wb") as fh:
                fh.write(b"\x00_ZN9beam_perf7collect6record17h9dd4b35daf2c39a5E\x00")
            hits = rust_no_perf.scan_bytes(path)
            self.assertTrue(hits)

    def test_mangled_span_symbol_is_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "libbeam_perf.rlib")
            with open(path, "wb") as fh:
                fh.write(b"\x00_ZN9beam_perf4Span3new17h8a934bf0d39cebfbE\x00")
            hits = rust_no_perf.scan_bytes(path)
            self.assertTrue(hits)

    def test_demangled_symbol_is_flagged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "libbeam_perf.rlib")
            with open(path, "wb") as fh:
                fh.write(b"beam_perf::collect::record::h9dd4b35daf2c39a5")
            hits = rust_no_perf.scan_bytes(path)
            self.assertTrue(hits)

    def test_symbol_spanning_chunk_boundary_is_flagged(self) -> None:
        """关键字跨块边界时不能漏（分块扫描的重叠逻辑）。"""
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "big.rlib")
            with open(path, "wb") as fh:
                fh.write(b"A" * (rust_no_perf.CHUNK - 4))
                fh.write(b"_ZN9beam_perf7collect6record17hE")
            hits = rust_no_perf.scan_bytes(path)
            self.assertTrue(hits)

    def test_nm_channel_is_used(self) -> None:
        """nm -C 输出的反混淆符号也要能被抓到。"""
        with tempfile.TemporaryDirectory() as tmp:
            artifact = os.path.join(tmp, "libbeam_perf.rlib")
            with open(artifact, "wb") as fh:
                fh.write(b"irrelevant")
            fake_nm = _make_fake_nm(tmp, "beam_perf::collect::record::h9dd4b35daf2c39a5")
            hits, symbol_lines = rust_no_perf.scan_nm([artifact], fake_nm)
            self.assertTrue(hits)
            self.assertGreater(symbol_lines, 0)


def _make_fake_nm(tmpdir: str, symbol_line: str) -> str:
    """造一个只会打印固定符号行的假 nm，用来验证 nm 通道的代码路径。"""
    script = os.path.join(tmpdir, "fake_nm.py")
    with open(script, "w", encoding="utf-8") as fh:
        fh.write("import sys\n")
        fh.write("sys.stdout.write(%r + '\\n')\n" % symbol_line)
    if os.name == "nt":
        wrapper = os.path.join(tmpdir, "nm.cmd")
        with open(wrapper, "w", encoding="utf-8", newline="") as fh:
            fh.write(f'@echo off\r\n"{sys.executable}" "{script}"\r\n')
        return wrapper
    wrapper = os.path.join(tmpdir, "nm")
    with open(wrapper, "w", encoding="utf-8") as fh:
        fh.write(f'#!/bin/sh\nexec "{sys.executable}" "{script}"\n')
    os.chmod(wrapper, os.stat(wrapper).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    return wrapper


if __name__ == "__main__":
    unittest.main(verbosity=2)
