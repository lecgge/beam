#!/usr/bin/env python3
r"""安装 Windows 原生工具链：FFmpeg（LGPL 6.x，带 NVENC）+ bindgen 用的 libclang。

## 为什么需要它

§7 规定：FFmpeg 只能用 **6.x**（libavcodec 60.x）——`ffmpeg-next 8.1.0` 与 7.x/8.x 的
ABI 不兼容（`AVCodec::sample_fmts`、`AVCodec::ch_layouts` 已被移除）；bindgen 还需要
`libclang.dll`。

两个容易踩的坑，本脚本都处理了：

1. **别装在 `%TEMP%` 里** —— Windows 的存储感知会定期清理临时目录，工具链会
   「莫名其妙消失」。本脚本默认装到 `%USERPROFILE%\Documents\work\beam-tools`。
2. **只装 ffmpeg 本体跑不起来** —— conda 的 ffmpeg 包只带它自己的 DLL，运行库依赖
   （libiconv / zlib / openssl / openh264 / icu …）在各自的包里，缺了就是
   `0xC0000135`（STATUS_DLL_NOT_FOUND）。脚本按 `info/index.json` 的 `depends`
   **递归**补齐，并且**尊重版本上下界**（只按下限挑会拿到不兼容的新版）。

## 用法

```powershell
python tools/dev/setup_native_toolchain.py
# 脚本最后会打印三个环境变量：
$env:FFMPEG_DIR    = "<prefix>\ffmpeg\root\Library"
$env:LIBCLANG_PATH = "<prefix>\libclang\libclang-<ver>.data\platlib\clang\native"
$env:PATH          = "$env:FFMPEG_DIR\bin;$env:PATH"
```

换过 FFmpeg 版本后必须 `cargo clean -p ffmpeg-sys-next -p ffmpeg-next`，
否则 cargo 会复用旧的 bindings（§7 有同样的提醒）。
"""

import argparse
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile

CHANNEL = "https://conda.anaconda.org/conda-forge"
PYPI = "https://pypi.org/pypi"


# --------------------------------------------------------------------- 工具
def vnum(version):
    return [int(x) if x.isdigit() else 0 for x in re.split(r"[.\-+]", version)[:4]]


def satisfies(version, constraint):
    """conda 的版本约束：>= / <= / > / < / =，逗号分隔（全部都要满足）。"""
    if not constraint:
        return True
    v = vnum(version)
    for part in constraint.split(","):
        part = part.strip()
        if not part:
            continue
        m = re.match(r"(>=|<=|>|<|=)?\s*([0-9][0-9.]*)", part)
        if not m:
            continue
        op, raw = m.group(1) or "=", m.group(2)
        b = vnum(raw)
        n = min(len(v), len(b))
        vv, bb = v[:n], b[:n]
        if op == ">=" and not vv >= bb:
            return False
        if op == "<=" and not vv <= bb:
            return False
        if op == ">" and not vv > bb:
            return False
        if op == "<" and not vv < bb:
            return False
        if op == "=" and vv != bb:
            return False
    return True


def api_json(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return json.loads(r.read().decode("utf-8"))


def download(url, dest):
    if os.path.exists(dest):
        return dest
    print("  下载", os.path.basename(dest), flush=True)
    with urllib.request.urlopen(url, timeout=1800) as r, open(dest, "wb") as o:
        shutil.copyfileobj(r, o)
    return dest


def extract_conda(archive, dest):
    os.makedirs(dest, exist_ok=True)
    if archive.endswith(".conda"):
        with zipfile.ZipFile(archive) as z:
            for n in z.namelist():
                if n.startswith("pkg-") and n.endswith(".tar.zst"):
                    inner = archive + "." + os.path.basename(n)
                    with z.open(n) as s, open(inner, "wb") as d:
                        shutil.copyfileobj(s, d)
                    subprocess.check_call(["tar", "-xf", inner, "-C", dest])
    else:
        subprocess.check_call(["tar", "-xf", archive, "-C", dest])


def read_depends(archive):
    """读 info/index.json 的 depends（用 tar CLI 读 .tar.zst —— Python 的 tarfile 不认 zstd）。"""
    tmp = tempfile.mkdtemp(prefix="beam-deps-")
    try:
        if archive.endswith(".conda"):
            with zipfile.ZipFile(archive) as z:
                for n in z.namelist():
                    if n.startswith("info-") and n.endswith(".tar.zst"):
                        inner = os.path.join(tmp, os.path.basename(n))
                        with z.open(n) as s, open(inner, "wb") as d:
                            shutil.copyfileobj(s, d)
                        subprocess.check_call(["tar", "-xf", inner, "-C", tmp])
        else:
            subprocess.check_call(["tar", "-xf", archive, "-C", tmp])
        for dirpath, _dirnames, filenames in os.walk(tmp):
            if os.path.basename(dirpath) == "info" and "index.json" in filenames:
                with io.open(os.path.join(dirpath, "index.json"), "r", encoding="utf-8") as f:
                    return json.load(f).get("depends", [])
    except Exception as err:  # noqa: BLE001 - 只影响可选信息
        print("  (读 depends 失败：%s)" % err)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return []


# --------------------------------------------------------------------- 安装
def pick_ffmpeg(pkg_version):
    """挑 6.x 的 LGPL 构建：recipe 最新 ⇒ 带 NVENC（§3.6 硬编路径要它）。"""
    info = api_json("https://api.anaconda.org/package/conda-forge/ffmpeg")
    cands = [
        f
        for f in info["files"]
        if f["attrs"].get("subdir") == "win-64"
        and f["version"].startswith(pkg_version)
        and "lgpl" in f["attrs"].get("build", "")
    ]

    def build_no(b):
        tail = b.split("_")[-1]
        return int(tail) if tail.isdigit() else 0

    cands.sort(key=lambda f: (vnum(f["version"]), build_no(f["attrs"]["build"])))
    return cands[-1] if cands else None


def pick_build(pkg, constraint):
    info = api_json("https://api.anaconda.org/package/conda-forge/%s" % pkg)
    cands = [
        f
        for f in info["files"]
        if f["attrs"].get("subdir") == "win-64" and satisfies(f["version"], constraint)
    ]
    cands.sort(key=lambda f: vnum(f["version"]))
    return cands[-1] if cands else None


def install_ffmpeg(prefix, cache, pkg_version):
    root = os.path.join(prefix, "ffmpeg", "root")
    pick = pick_ffmpeg(pkg_version)
    if pick is None:
        raise SystemExit("找不到 ffmpeg %s 的 win-64 LGPL 构建" % pkg_version)
    print("ffmpeg:", pick["version"], pick["attrs"]["build"])
    archive = download(
        "%s/%s" % (CHANNEL, pick["basename"]),
        os.path.join(cache, os.path.basename(pick["basename"])),
    )
    extract_conda(archive, root)

    # 运行库依赖链（递归，尊重版本上下界）
    queue = [(d, "ffmpeg") for d in read_depends(archive)]
    seen = {"ffmpeg"}
    count = 0
    while queue:
        dep, _parent = queue.pop(0)
        name = dep.split()[0]
        if name in seen:
            continue
        seen.add(name)
        build = pick_build(name, dep[len(name) :].strip())
        if build is None:
            print("  跳过（没有匹配的 win-64 构建）：", dep)
            continue
        local = download(
            "%s/%s" % (CHANNEL, build["basename"]),
            os.path.join(cache, os.path.basename(build["basename"])),
        )
        extract_conda(local, root)
        count += 1
        queue.extend((d, name) for d in read_depends(local))
    print("  运行库包数：", count)


def install_libclang(prefix, cache):
    meta = api_json("%s/libclang/json" % PYPI)
    wheel = None
    for f in meta["urls"]:
        if f["filename"].endswith(".whl") and "win_amd64" in f["filename"]:
            wheel = f
    if wheel is None:
        raise SystemExit("PyPI 上没有 libclang 的 win_amd64 wheel")
    print("libclang:", wheel["filename"])
    dest = os.path.join(prefix, "libclang")
    shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(dest, exist_ok=True)
    archive = download(
        wheel["url"], os.path.join(cache, os.path.basename(wheel["filename"]))
    )
    with zipfile.ZipFile(archive) as z:
        z.extractall(dest)
    for dirpath, _dirnames, filenames in os.walk(dest):
        for n in filenames:
            if n.lower() == "libclang.dll":
                print("  libclang.dll:", os.path.join(dirpath, n))
                return os.path.dirname(os.path.join(dirpath, n))
    raise SystemExit("解出来的包里没有 libclang.dll")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--prefix",
        default=os.path.join(
            os.path.expanduser("~"), "Documents", "work", "beam-tools"
        ),
        help="工具链安装目录（默认：%%USERPROFILE%%\\Documents\\work\\beam-tools）",
    )
    ap.add_argument("--ffmpeg-version", default="6.1.2", help="FFmpeg 版本（§7：只用 6.x）")
    args = ap.parse_args()

    prefix = os.path.abspath(args.prefix)
    cache = os.path.join(prefix, "pkgs")
    os.makedirs(cache, exist_ok=True)

    install_ffmpeg(prefix, cache, args.ffmpeg_version)
    lc = install_libclang(prefix, cache)

    ff = os.path.join(prefix, "ffmpeg", "root", "Library")
    print()
    print("FFMPEG_DIR    =", ff)
    print("LIBCLANG_PATH =", lc)
    print("PATH          = %s\\bin;$env:PATH" % ff)
    print()
    print("换过版本后请执行：cargo clean -p ffmpeg-sys-next -p ffmpeg-next")


if __name__ == "__main__":
    main()
