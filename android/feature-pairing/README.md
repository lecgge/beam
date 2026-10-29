# feature-pairing — 扫码与 PIN 配对

> 技术设计文档 **§3.12** 的 `:feature-pairing` 行：
> *职责*：CameraX + ML Kit 扫码、PIN 键盘；
> *对外接口*：`PairingViewModel`；
> *禁止*：—。

## 职责

两条配对路径：

1. **扫码**（§3.3 二维码载荷 `beam://pair?h=ip:port&fp=..&t=token`）：
   CameraX `ImageAnalysis` + ML Kit 条码识别 → 调 `BeamClient.pairQr`；
2. **PIN**（SPAKE2，4–6 位）：数字键盘 → 调 `BeamClient.pairPin`。

成功后把主机写入信任库（`:core-data` 的 `HostRepo`）。

## Kotlin 对外接口

| 类型 | 说明 |
|---|---|
| `PairingViewModel` | `state: StateFlow<PairingUiState>`；`startFor` / `onQrScanned` / `onDigit` / `onBackspace` / `onSubmit` / `reset` |
| `PairingUiState` | `peer` / `phase` / `pin` / `error` / `attemptsLeft` / `lockoutUntilMs` |
| `PairingPhase` | `IDLE` / `SCANNING` / `PIN_ENTRY` / `PAIRING` / `SUCCESS` / `FAILED` |
| `PairPayload` | 扫码载荷解析（纯逻辑，可单测） |
| `PairingScreen` | 纯展示 Composable（状态 + 回调） |

## 错误码（§3.3）

| 码 | 含义 | UI 文案 |
|---|---|---|
| `E3001` | PIN 错误 | 「PIN 错误，请核对主机屏幕上的数字（剩余 N 次）」 |
| `E3002` | 次数超限锁定 60 s | 「尝试次数过多，已锁定，请稍后再试」 |
| `E3003` | token 过期 | 「二维码已过期，请在主机上刷新后重新扫码」 |
| `E3004` | 指纹不匹配 | 「主机指纹与已信任的主机不一致，已拒绝连接」 |

## 禁止事项落实

| 约束 | 落实方式 |
|---|---|
| 不直接调 JNI | 全部经 `:core-net` 的 `BeamClient` |
| UI 与逻辑分离 | `PairingScreen` 只渲染状态并回调；配对流程在 ViewModel 内 |

## 权限

`CAMERA` 按需申请：进入配对页时才弹（见 `:app` 的 `BeamPermissions`），
不申请则无法扫码，但仍可走 PIN 路径。

## 测试

```bash
./gradlew :feature-pairing:test
```

`PairPayloadTest` 覆盖：合法载荷、协议不符、缺字段、端口非法、首尾空白。
