# check-no-perf —— release 产物「无 perf」可执行门禁

对应技术设计文档 §7:274-276 与差距清单 **X7-12 / X7-14 / X7-15 / X13-14**。

## 为什么需要这个目录

旧 CI 的 Android 检查是：

```bash
APK=$(ls app/build/outputs/apk/release/*/*.apk | head -1)
if unzip -l "$APK" | grep -qi "feature-perf\|perfstats"; then ... exit 1; fi
```

**这条门禁恒通过，等于没有门禁。** 原因：

- `unzip -l` 列出的是 APK 的 **zip 条目名**，而 JVM 类名存放在 `classes*.dex`
  的字符串池里，从不出现在条目名里；
- 实测 release APK 有 **419 个条目**（`classes.dex`、`lib/arm64-v8a/*.so`、
  `res/**`、`assets/dexopt/*` …），上面那条 grep 无论构建成什么样都是 0 命中。

正确的检查对象是 **dex 里的类名字符串**，本目录的两个脚本都基于这一点。

## 实测基线（本机，2026-10-29）

| 对象 | 结果 |
|---|---|
| debug APK | 576 个 zip 条目 / **16 个 dex**；`dev/beam/feature/perf` 64 处、`dev/beam/core/data/perf` 25 处、`dev/beam/core/net/Perf` 5 处、`dev/beam/app/perf` 9 处、`PerfSnapshotStore` 5 处、`PerfExportFormat` 8 处、`PerfRecorder` 28 处 → **必须报错** |
| release APK | 419 个 zip 条目 / **1 个 dex**；上述模式 **全部 0 命中** → **必须通过** |
| R8 `mapping.txt` | `dev.beam.core.data.perf.PerfSnapshotStore -> q3.b:`、`dev.beam.core.data.perf.PerfExportFormat -> q3.a:`、`dev.beam.core.data.perf.PerfUiState -> q3.c:`、`dev.beam.app.perf.PerfEntry -> m3.a:` |

> ⚠️ 注意最后一行：这些 perf 类**被 R8 改名保留了**，混淆名（`Lq3/b;` 等）就在
> release dex 里。只按原名扫 dex 会漏掉它们，所以脚本额外做 mapping 交叉验证。
> 详见下面「R8 映射表的判定」。

## 脚本

### `apk_no_perf.py` —— Android 侧

对 APK 内每个 `classes*.dex` 做字节流扫描（流式读进内存，**不解压到磁盘**），
再交叉验证 R8 映射表。

```bash
# release 必须通过（store / internal 两个 flavor 一起查）
python tools/ci/check-no-perf/apk_no_perf.py \
    --apk 'android/app/build/outputs/apk/store/release/*.apk' \
          'android/app/build/outputs/apk/internal/release/*.apk' \
    --mapping 'android/app/build/outputs/mapping/storeRelease/mapping.txt' \
             'android/app/build/outputs/mapping/internalRelease/mapping.txt'

# debug 必须被抓到（负样本自检）
python tools/ci/check-no-perf/apk_no_perf.py \
    --apk 'android/app/build/outputs/apk/store/debug/*.apk' \
          'android/app/build/outputs/apk/internal/debug/*.apk' \
    --expect perf
```

| 参数 | 说明 |
|---|---|
| `--apk` | APK 路径或 glob，**可给多个**；glob 展开成**全部**匹配，逐个判定 |
| `--mapping` | R8 `mapping.txt` 路径，可选；给 1 个则应用到全部 APK，给等量则按顺序 1:1 配对 |
| `--expect clean\|perf` | `clean`=不许出现 perf（默认，门禁用）；`perf`=必须出现（负样本自检） |
| `--strict-mapping` | 把 mapping 告警升级为硬失败（见下） |
| `--quiet` | 只输出结论 |

### 多 flavor 与「静默漏检」

`--apk` 的 glob 会展开成**全部**匹配并**逐个**判定；`--expect` 两种模式都要求
**每个 APK 都满足**：

- `clean`：任何一个 APK 命中 perf → 整体失败；
- `perf`：任何一个 APK 没命中 → 整体失败（说明该 flavor 的 debug 依赖图漂移了）。

原因：Android 侧现在有 `store` / `internal` 两个 flavor（§7 分发渠道），产物路径是
`apk/<flavor>/<buildType>/`。旧版 `--apk` 只取 glob 的**第一个**匹配，单 flavor 时看不出
问题，两个 flavor 之后就会漏检一个 —— 门禁从「恒通过」退化成「随机通过」。

mapping 与 APK 的配对规则：给 1 个 → 应用于全部；给 N 个且 N == APK 数 → 按顺序 1:1
（store 用 `storeRelease/mapping.txt`，internal 用 `internalRelease/mapping.txt`）；
其余组合按用法错误退出 2。

### `rust_no_perf.py` —— Rust 侧

扫描 release 产物里的 perf 采集符号，两条通道：

1. **字节级**（跨平台，Windows 可用）：找只可能由 `perf-stats` 开启产生的符号；
2. **`nm -C`**（文档 §7:275 的口径，Linux/macOS 才有 `nm`）：反混淆后再查一遍。

```bash
# release 必须通过
python tools/ci/check-no-perf/rust_no_perf.py --target-dir target/release

# Linux 上叠加 nm 口径（找不到 nm 直接失败）
python3 tools/ci/check-no-perf/rust_no_perf.py --target-dir target/release --require-nm

# 负样本自检：feature 开启的产物必须被检出
CARGO_TARGET_DIR=target/tmp/perf-on cargo build --release -p beam-perf --features perf-stats
python tools/ci/check-no-perf/rust_no_perf.py --target-dir target/tmp/perf-on/release --expect perf
```

| 参数 | 说明 |
|---|---|
| `--target-dir` | release 产物目录，默认 `target/release` |
| `--require-nm` | 必须存在 `nm` 且 nm 至少解析出一行符号，否则失败（防止 nm 通道形同虚设） |
| `--no-nm` | 只做字节级扫描 |
| `--expect clean\|perf\|nm-perf` | `clean`=不许出现（默认，门禁用）；`perf`=必须出现（负样本自检）；`nm-perf`=必须由 `nm -C` 通道检出（Linux 上验证 nm 口径本身有效） |

### 「旧产物残留」与误报

`cargo build --release -p beam-perf --features perf-stats` 产出的是**另一个** rlib，
它会永久留在 `target/release/deps/` 里 —— cargo 不会清理不同 feature 组合的旧产物。
之后扫描默认产物时，那个 perf-on 的 rlib 会让门禁**误报**（旧产物残留，不是代码回归）。

脚本按 cargo 的构建元数据区分二者：

- 每个产物在 `<target>/.fingerprint/<包名>-<hash>/lib-<库名>.json` 里记着它**实际**
  编译时启用的 feature 集合；
- `--expect clean`（默认 release 门禁）会**跳过**由 `perf-stats` 开启构建出来的产物
  —— 它们不属于「默认 release 构建」；
- `--expect perf`（负样本自检）**不**跳过：那里就是要找到 perf-on 产物。

读不到元数据时**不跳过** —— 宁可误报也不漏报。

> 命名差异提醒：产物文件用**库名**（下划线，`libbeam_perf-<hash>.rlib`），
> fingerprint 目录用**包名**（连字符，`beam-perf-<hash>`）。脚本按 hash 定位，
> 不靠猜目录名。

**仍然建议**：负样本自检用**独立的 `CARGO_TARGET_DIR`**（CI 已经是这样），
不要往默认 target 目录里塞 perf-on 产物。

```bash
# 负样本自检：feature 开启的产物必须被检出（独立 target 目录）
CARGO_TARGET_DIR=target/tmp/perf-on cargo build --release -p beam-perf --features perf-stats
python tools/ci/check-no-perf/rust_no_perf.py --target-dir target/tmp/perf-on/release --expect perf
```

## 判据是怎么定的（为什么不是 `grep "perf::"`）

`crates/beam-perf` 的采集实现（`collect.rs`、`Span`、`perf_span!` 的实体分支）全部由
`#[cfg(feature = "perf-stats")]` 门控，因此「只可能由 feature 开启产生的符号」就是判据。
实测（rustc 1.86 / MSVC）：

| 关键字 | feature 关 | feature 开 | 能否当判据 |
|---|---|---|---|
| `beam_perf7collect`（混淆名 `_ZN9beam_perf7collect6recordE`） | 0 | 41 | ✅ |
| `beam_perf4Span`（混淆名 `_ZN9beam_perf4Span3newE`） | 0 | 3 | ✅ |
| `perf_span`（宏名） | 6 | 7 | ❌ 两边都有（`macro_rules!` 空实现照样导出） |
| `beam_perf4Perf`（stub 类型） | 有 | 有 | ❌ 只是空实现，没有采集逻辑 |
| `perf::`（文档 §7:275 的模式） | — | — | ❌ 反混淆后 `beam_perf::Perf` 也含 `perf::`，恒命中 |

`cargo bloat` 没有采用：需要 nightly + 额外安装，比的是体积差而不是符号，对本仓库
「一个 crate 一个 feature」的结构收益有限。

## R8 映射表的判定

文档 §7:276 要求「检查 R8 映射表中无 perf 相关类名」。**字面照做不可行**：
R8 会给所有处理过的类留痕，被删掉的类也在 mapping 里，所以 mapping 里有 perf 类名
是**常态**，直接判失败等于永远红。

脚本改成回答更有意义的问题：**这些类最终还在 dex 里吗？**
做法是解析 mapping 的类级行（`原始类 -> 混淆名:`），把混淆名转成 dex 类型描述符
（`q3.b` → `Lq3/b;`）再去 dex 里找。分两档：

| 命名空间 | 判定 | 理由 |
|---|---|---|
| `dev.beam.feature.perf.*` | **硬失败** | `:feature-perf` 是 debug 专属模块（`debugImplementation`），它出现在 release 就是构建配错了 |
| `dev.beam.core.data.perf.*` / `dev.beam.core.net.Perf*` / `dev.beam.app.perf.*` | **告警 + 列出类名** | 来自 `:core-data` / `:core-net` / `:app` 这些**始终参与 release** 的模块；彻底去掉必须先改 android 侧（例如把性能面板挂到 `BuildConfig.PERF_STATS` 开关后面），超出本目录职责 |

被 R8 **移除**（混淆名不在 dex 里）的 perf 类不告警。

android 侧把性能面板关进 `BuildConfig.PERF_STATS` 之后，可以用 `--strict-mapping`
把告警升级成硬失败。

## 已知例外（不要「顺手补上」）

- **`PerfUiState`**：`:core-data` 里一个 Kotlin data class，编译器生成的 `toString()`
  会把 `"PerfUiState(fps=0.0, bitrateKbps=0.0, ...)"` 当成**字符串常量**写进 dex。
  R8 混淆的是类名、改不了这个字面量，所以 release dex 里必然残留 1 处命中 ——
  它不代表 perf 采集代码进了包，因此**不**纳入禁用集。
- `perform*`、`performanceMode` 等 AndroidX/Compose/CameraX 的既有标识符与 perf 无关。

## 单元测试

```bash
python tools/ci/check-no-perf/test_check_no_perf.py
```

19 个用例，只依赖标准库，不需要 cargo / gradle / APK 产物；覆盖：干净样本不误报、
三个命名空间 + 三个类型名都能抓到、`PerfUiState` 不误报、mapping 两档判定、
R8 字节扫描的跨块边界、`nm -C` 通道，以及多 flavor 场景下的
「glob 展开成全部匹配 / mapping 配对规则 / 每个 APK 都要判定」，
以及 cargo fingerprint 的 feature 集合解析（区分默认构建与 perf-stats 开启构建）。

## 退出码

`0` 通过 / `1` 门禁失败 / `2` 用法或 IO 错误。
