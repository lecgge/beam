# check-design-tokens —— 桌面端设计令牌门禁

对应产品方案 **§16「设计规范」**(`docs/beam-product-plan.md:215-229`),
覆盖差距清单里的 **UX-11 / UX-13 / UX-14 / UX-15 / UX-18 / UX-19 / UX-20 /
UX-21 / UX-22 / DS-07 / DS-12** 等「视觉规范没落地」的条目。

## 为什么需要这个目录

§16 的令牌是**可以机械判定**的:颜色是固定字面量、字号/间距/圆角是固定数字。
但桌面端的样式是手写的,一次改版就可能把 `#5b7cff` 改成 `#6d5bff`、
把卡片内边距从 `14` 改成 `16`、把主按钮改回渐变——这些**编译期不会报错**,
只有肉眼比对文档才能发现,而且很容易漏。

本脚本把「§16 的令牌逐字核对」变成一条可进 CI 的门禁。

## 脚本

### `check_design_tokens.py`

只依赖 Python 标准库(无第三方包),读取
`apps/desktop/src/styles.css`,解析出「令牌 → 实际值」,再与 §16 的期望值逐项比对。

| 参数 | 说明 |
|---|---|
| `--css PATH` | 要检查的 CSS 文件(默认:仓库根 `apps/desktop/src/styles.css`) |
| `--quiet` | 只输出结论(`::error::` 行仍会打印) |

```bash
# 门禁(默认路径)
python tools/ci/check-design-tokens/check_design_tokens.py

# 只看结论
python tools/ci/check-design-tokens/check_design_tokens.py --quiet

# 指定别的 CSS(比如临时改坏的样本)
python tools/ci/check-design-tokens/check_design_tokens.py --css apps/desktop/src/styles.css
```

## 查了什么

1. **颜色令牌(深色 + 浅色)**——§16 的每一行都有同名 CSS 变量,值**逐字**一致:

   | 令牌 | 深色 | 浅色 |
   |---|---|---|
   | `--bg` | `#0b0f17` | `#f4f6fb` |
   | `--panel` | `#121826` | `#ffffff` |
   | `--line` | `#222b3d` | `#dfe4ee` |
   | `--text` | `#e8edf6` | `#151b28` |
   | `--sub` | `#8f9ab0` | `#5d6880` |
   | `--pri` | `#5b7cff` | `#3d5cf5` |
   | `--ok` / `--warn` / `--err` | `#22c55e` / `#f59e0b` / `#ef4444` | `#16a34a` / `#d97706` / `#dc2626` |
   | `--overlay` + `--overlay-alpha` | `#0a0e18` + `85%` | 同深色 |

   深色是 `:root` 的默认值,浅色是 `:root[data-theme='light']`(桌面端既有的
   主题机制,`App.tsx` 依据设置项 `theme` 在 `<html>` 上写 `data-theme`)。

2. **字号四档**——`--font-title` / `--weight-title` = `18px` / `600`;
   `--font-body` / `--weight-body` = `14px` / `400`;
   `--font-helper` / `--weight-helper` = `12px` / `400`;
   `--font-pin` / `--weight-pin` = `26px` / `700`。

3. **间距**——`--space-card` = `14px`(卡片内边距)、`--space-gap` = `12px`、
   `--space-page` = `20px`。

4. **圆角**——`--radius-btn` = `8px`、`--radius-card` = `12px`、
   `--radius-device` = `30px`。

5. **字体栈**——`--font-family` 必须含 `Microsoft YaHei UI` 与 `Segoe UI`;
   若把 Android 端的 `Noto Sans CJK` 放在首位也会报错(UX-14)。

6. **令牌必须被用到**——`--bg` / `--panel` / `--line` / `--text` / `--sub` /
   `--pri` / `--ok` / `--warn` / `--err` / `--overlay` / `--overlay-alpha` /
   三个间距 / 两个圆角,都必须在样式里出现 `var(--token)`。
   这一条专门防「**定义了却没落地**」:UX-11(主色被渐变顶掉)和
   UX-13(串流浮层令牌根本没建)当初就是这样漏掉的。

7. **主按钮不是渐变**——`.btn.primary` 里出现 `gradient` 即失败
   (§16 的主色是纯色 `#5b7cff` / `#3d5cf5`)。

> `--radius-device`(手机外框 30)在桌面端目前**没有使用点**——§16 的「手机外框」
> 是 Android 端手机模型的圆角,桌面端没有这个形状。脚本只校验它的**取值**
> (保证两端共用同一套刻度),不强制它被引用。

## 退出码

| 码 | 含义 |
|---|---|
| `0` | 通过 |
| `1` | 门禁失败(逐条打印 `::error::`,列出实际值与 §16 期望值) |
| `2` | 用法错 / IO 错(文件不存在、读不到、找不到 `:root` 令牌块) |

## 已验证的反例

改动样式后脚本会逐条报出来,例如:

```
::error::dark 主题的 --pri = #6d5bff,与 §16 的 #5b7cff 不一致
::error::dark 主题的 --font-pin = 30px,与 §16 的 26px 不一致
::error::.btn.primary 用了渐变,§16 的主色是纯色:background: linear-gradient(90deg, ...)
::error::令牌 --overlay-alpha 定义了却没有任何地方用到(§16 的值没有真正落地)
```

## 一处文档内部张力(已按 §16 逐字表实现)

§16 同一行里写的是「**间距 4 的倍数**:卡片内边距 **14**、间距 12、页边距 20」——
但 `14` 本身不是 4 的倍数。本脚本按 §16 的**逐字表**校验(卡片内边距 = `14px`),
不额外强制「所有间距都是 4 的倍数」这条,避免与同一条规范自相矛盾。
