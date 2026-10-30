#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""桌面端设计令牌核对门禁(产品方案 §16「设计规范」)。

依据:`docs/beam-product-plan.md` §16(第 215-229 行)—— 颜色、字号、间距、
圆角四张刻度表,以及「组件」「状态」两行要求里能用样式令牌表达的部分。

本脚本只做**可机械判定**的部分:

    1. §16 的每个令牌在 `apps/desktop/src/styles.css` 里都有同名 CSS 变量,
       且值**逐字**一致(深色为 `:root` 默认值,浅色为 `:root[data-theme='light']`);
    2. 字号四档(标题 18/600、正文 14/400、辅助 12/400、PIN 26/700)存在;
    3. 间距(卡片内边距 14、间距 12、页边距 20)与圆角(按钮 8、卡片 12、
       手机外框 30)存在;
    4. 字体栈含 `Microsoft YaHei UI` 与 `Segoe UI`,且不再把 Android 端的
       `Noto Sans CJK` 放在首位;
    5. 令牌**真的被用到**(`var(--token)` 出现)—— 防止「定义了却没落地」
       (UX-11 主色、UX-13 串流浮层当初就是这样);
    6. 主按钮不是渐变(UX-11:主色是纯色 `#5b7cff` / `#3d5cf5`)。

不依赖任何第三方库,只用标准库。

退出码:0 = 通过;1 = 门禁失败;2 = 用法 / IO 错误。
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# Windows 控制台默认 GBK,输出中文可能触发 UnicodeEncodeError。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except AttributeError:  # pragma: no cover - Python < 3.7
    pass

# ---------------------------------------------------------------------------
# §16 期望值(产品方案 docs/beam-product-plan.md:215-229)
# ---------------------------------------------------------------------------

#: §16 颜色令牌:深色(:root 默认)与浅色(:root[data-theme='light'])。
EXPECTED_COLORS: Dict[str, Dict[str, str]] = {
    "dark": {
        "--bg": "#0b0f17",  # 背景:页面底色
        "--panel": "#121826",  # 面板:卡片、侧栏
        "--line": "#222b3d",  # 分隔:描边
        "--text": "#e8edf6",  # 正文
        "--sub": "#8f9ab0",  # 次要文字
        "--pri": "#5b7cff",  # 主色:主按钮、选中
        "--ok": "#22c55e",  # 成功
        "--warn": "#f59e0b",  # 警告
        "--err": "#ef4444",  # 错误
        "--overlay": "#0a0e18",  # 串流浮层底色
        "--overlay-alpha": "85%",  # 串流浮层不透明度
    },
    "light": {
        "--bg": "#f4f6fb",
        "--panel": "#ffffff",
        "--line": "#dfe4ee",
        "--text": "#151b28",
        "--sub": "#5d6880",
        "--pri": "#3d5cf5",
        "--ok": "#16a34a",
        "--warn": "#d97706",
        "--err": "#dc2626",
        # §16:串流浮层「浅色 = 同深色」。
        "--overlay": "#0a0e18",
        "--overlay-alpha": "85%",
    },
}

#: §16 字号 / 字重四档(深浅两套主题共用,不随主题变化)。
EXPECTED_TYPOGRAPHY: Dict[str, str] = {
    "--font-title": "18px",  # 标题
    "--weight-title": "600",
    "--font-body": "14px",  # 正文
    "--weight-body": "400",
    "--font-helper": "12px",  # 辅助
    "--weight-helper": "400",
    "--font-pin": "26px",  # PIN
    "--weight-pin": "700",
}

#: §16 间距:卡片内边距 14、间距 12、页边距 20。
EXPECTED_SPACING: Dict[str, str] = {
    "--space-card": "14px",
    "--space-gap": "12px",
    "--space-page": "20px",
}

#: §16 圆角:按钮 8、卡片 12、手机外框 30。
EXPECTED_RADIUS: Dict[str, str] = {
    "--radius-btn": "8px",
    "--radius-card": "12px",
    "--radius-device": "30px",
}

#: §16 字体:Windows 用 `Microsoft YaHei UI` / `Segoe UI`。
EXPECTED_FONT_FAMILY = ("Microsoft YaHei UI", "Segoe UI")
#: Android 端的字体栈不应该再出现在桌面端的首位。
FORBIDDEN_FONT_FAMILY = "Noto Sans CJK"

#: 这些令牌必须**被用到**,否则就是「定义了却没落地」(UX-11 / UX-13 的老毛病)。
MUST_BE_USED = (
    "--bg",
    "--panel",
    "--line",
    "--text",
    "--sub",
    "--pri",
    "--ok",
    "--warn",
    "--err",
    "--overlay",
    "--overlay-alpha",
    "--space-card",
    "--space-gap",
    "--space-page",
    "--radius-btn",
    "--radius-card",
)

# ---------------------------------------------------------------------------
# CSS 解析(只用标准库;styles.css 是平铺的,没有嵌套规则)
# ---------------------------------------------------------------------------

COMMENT_RE = re.compile(r"/\*.*?\*/", re.S)
BLOCK_RE = re.compile(r"([^{}]+)\{([^{}]*)\}", re.S)
DECL_RE = re.compile(r"(--[A-Za-z0-9_-]+)\s*:\s*([^;]+);")


def strip_comments(css: str) -> str:
    """去掉 CSS 注释,避免注释里的花括号/分号干扰解析。"""
    return COMMENT_RE.sub("", css)


def parse_blocks(css: str) -> List[Tuple[str, str]]:
    """把 CSS 拆成 [(选择器, 声明块)]。"""
    return [
        (match.group(1).strip(), match.group(2))
        for match in BLOCK_RE.finditer(strip_comments(css))
    ]


def parse_variables(body: str) -> Dict[str, str]:
    """从声明块里取出自定义属性(值已 strip)。"""
    variables: Dict[str, str] = {}
    for match in DECL_RE.finditer(body):
        variables[match.group(1)] = match.group(2).strip()
    return variables


def parse_themes(css: str) -> Dict[str, Dict[str, str]]:
    """按主题取出自定义属性。

    - ``dark``  = ``:root``(默认,深色)
    - ``light`` = ``:root[data-theme='light']``
    """
    themes: Dict[str, Dict[str, str]] = {"dark": {}, "light": {}}
    for selector, body in parse_blocks(css):
        if "data-theme" in selector and "light" in selector:
            themes["light"].update(parse_variables(body))
        elif selector.startswith(":root"):
            themes["dark"].update(parse_variables(body))
    return themes


def find_rule_body(css: str, needle: str) -> Optional[str]:
    """找到选择器里含 ``needle`` 的第一条规则,返回它的声明块。"""
    for selector, body in parse_blocks(css):
        if needle in selector:
            return body
    return None


# ---------------------------------------------------------------------------
# 各项检查
# ---------------------------------------------------------------------------


class Report:
    """收集检查结果。"""

    def __init__(self, quiet: bool = False) -> None:
        self.quiet = quiet
        self.failures: List[str] = []
        self.checks = 0

    def info(self, message: str) -> None:
        if not self.quiet:
            print(f"[beam] {message}")

    def fail(self, message: str) -> None:
        self.failures.append(message)
        print(f"::error::{message}")

    def expect_token(self, theme: str, name: str, expected: str, actual: Optional[str]) -> None:
        """比对一个令牌的取值。"""
        self.checks += 1
        if actual is None:
            self.fail(f"{theme} 主题缺少令牌 {name}(§16 要求 {expected})")
            return
        if actual.strip().lower() != expected.lower():
            self.fail(
                f"{theme} 主题的 {name} = {actual},与 §16 的 {expected} 不一致"
            )

    def expect_present(self, theme: str, name: str, expected: str, actual: Optional[str]) -> None:
        """只要求存在(用于「必须被用到」之外的宽松项)。"""
        self.expect_token(theme, name, expected, actual)


def check_tokens(themes: Dict[str, Dict[str, str]], report: Report) -> None:
    """比对 §16 的颜色 / 字号 / 间距 / 圆角令牌。"""
    for theme, expected_map in (
        ("dark", EXPECTED_COLORS["dark"]),
        ("light", EXPECTED_COLORS["light"]),
    ):
        actual_map = themes.get(theme, {})
        for name, expected in expected_map.items():
            report.expect_token(theme, name, expected, actual_map.get(name))

    dark = themes.get("dark", {})
    for name, expected in EXPECTED_TYPOGRAPHY.items():
        report.expect_token("dark", name, expected, dark.get(name))
    for name, expected in EXPECTED_SPACING.items():
        report.expect_token("dark", name, expected, dark.get(name))
    for name, expected in EXPECTED_RADIUS.items():
        report.expect_token("dark", name, expected, dark.get(name))


def check_font_family(css: str, themes: Dict[str, Dict[str, str]], report: Report) -> None:
    """字体栈必须含 `Microsoft YaHei UI` 与 `Segoe UI`,且 Noto Sans CJK 不在首位。"""
    report.checks += 1
    stack = themes.get("dark", {}).get("--font-family", "")
    if not stack:
        report.fail("缺少 --font-family 令牌(§16 要求 Windows 用 Microsoft YaHei UI / Segoe UI)")
        return
    for expected in EXPECTED_FONT_FAMILY:
        if expected not in stack:
            report.fail(f"--font-family 里没有 {expected}:{stack}")
    if FORBIDDEN_FONT_FAMILY in stack:
        first = stack.split(",")[0].strip().strip("'\"")
        if FORBIDDEN_FONT_FAMILY.lower() in first.lower():
            report.fail(
                f"--font-family 把 Android 端的 {FORBIDDEN_FONT_FAMILY} 放在首位:{stack}"
            )


def check_token_usage(css: str, report: Report) -> None:
    """令牌必须真的被 `var(--token)` 用到。"""
    for name in MUST_BE_USED:
        report.checks += 1
        if f"var({name})" not in css:
            report.fail(f"令牌 {name} 定义了却没有任何地方用到(§16 的值没有真正落地)")


def check_primary_button(css: str, report: Report) -> None:
    """主按钮必须是纯色主色,不能是渐变(UX-11)。"""
    report.checks += 1
    body = find_rule_body(css, ".btn.primary")
    if body is None:
        report.fail("找不到 .btn.primary 规则(§16 要求主按钮使用主色)")
        return
    if "gradient" in body:
        report.fail(f".btn.primary 用了渐变,§16 的主色是纯色:{' '.join(body.split())}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="check_design_tokens.py",
        description="核对桌面端 styles.css 的设计令牌是否符合产品方案 §16",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例:\n"
            "  # 门禁(默认路径:仓库根/apps/desktop/src/styles.css)\n"
            "  python tools/ci/check-design-tokens/check_design_tokens.py\n"
            "  # 只看结论\n"
            "  python tools/ci/check-design-tokens/check_design_tokens.py --quiet\n"
            "  # 指定别的 CSS\n"
            "  python tools/ci/check-design-tokens/check_design_tokens.py --css apps/desktop/src/styles.css\n"
        ),
    )
    parser.add_argument(
        "--css",
        default=None,
        help="要检查的 CSS 文件(默认:仓库根/apps/desktop/src/styles.css)",
    )
    parser.add_argument("--quiet", action="store_true", help="只输出结论(错误仍会输出)")
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)

    # 仓库根 = 本脚本上溯三层(tools/ci/check-design-tokens/ → 仓库根)。
    repo_root = Path(__file__).resolve().parents[3]
    css_path = Path(args.css) if args.css else repo_root / "apps" / "desktop" / "src" / "styles.css"

    if not css_path.is_file():
        print(f"::error::找不到样式文件:{css_path}", file=sys.stderr)
        return 2

    try:
        css = css_path.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"::error::读取样式文件失败:{exc}", file=sys.stderr)
        return 2

    report = Report(quiet=args.quiet)
    report.info(f"样式文件:{css_path}")

    themes = parse_themes(css)
    if not themes.get("dark"):
        print("::error::样式文件里没有找到 :root 令牌块", file=sys.stderr)
        return 2
    if not themes.get("light"):
        print("::error::样式文件里没有找到 :root[data-theme='light'] 令牌块", file=sys.stderr)
        return 2

    report.info(
        f"深色令牌 {len(themes['dark'])} 个、浅色令牌 {len(themes['light'])} 个"
    )

    check_tokens(themes, report)
    check_font_family(css, themes, report)
    check_token_usage(css, report)
    check_primary_button(css, report)

    if report.failures:
        print(
            f"::error::设计令牌核对未通过:{len(report.failures)} 项不符"
            f"(共检查 {report.checks} 项)",
            file=sys.stderr,
        )
        return 1

    report.info(f"结论: 通过 —— {report.checks} 项检查全部符合产品方案 §16")
    return 0


if __name__ == "__main__":
    sys.exit(main())
