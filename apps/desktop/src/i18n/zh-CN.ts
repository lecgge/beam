/**
 * 中文(zh-CN)词条:默认且主要语言。
 *
 * 结构上与 `en.ts` 完全对齐(由 `typeof zhCN` 约束),切换语言只需换词表。
 */

/**
 * 支持的语言。
 *
 * 定义在这个**不依赖 React** 的模块里,好让 `ipc/errors.ts` 之类的
 * 纯逻辑模块也能引用同一个联合类型,避免两处各写一份而漂移。
 */
export type Language = 'zh-CN' | 'en'

export const zhCN = {
  app: {
    name: 'Beam',
    tagline: 'Windows 串流主机',
  },
  nav: {
    home: '主机',
    devices: '设备',
    connect: '连接其他主机',
    perf: '诊断',
    settings: '设置',
  },
  common: {
    ok: '确定',
    cancel: '取消',
    confirm: '确认',
    retry: '重试',
    loading: '加载中…',
    empty: '暂无数据',
    unknown: '未知',
    enabled: '开',
    disabled: '关',
    back: '返回',
    remove: '移除',
    save: '保存',
    saved: '已保存',
    copy: '复制',
    copied: '已复制',
    demo: '演示数据',
    errorFallback: '操作失败,请稍后重试',
    retryableHint: '可重试',
    close: '关闭',
    // 剩余可试次数(§16 状态行:错误 = 描红 + 剩余次数)。
    attemptsLeft: '剩余 {count} 次',
    rescan: '重新搜索',
    // 语言名按「语言自称」惯例固定,不随界面语言变化(两种语言下都是同一串)。
    languageZhCn: '简体中文',
    languageEn: 'English',
  },
  /**
   * 无障碍标签(§16 的组件与状态都要能被读屏软件念出来)。
   */
  a11y: {
    latencyChart: '端到端延迟曲线',
    pairingQr: '配对二维码',
  },
  home: {
    title: '本机已就绪',
    subtitle: '{name} · {ip} · 局域网自动发现已开启',
    ipUnknown: '未获取到局域网地址',
    standby: '待机中',
    hosting: '串流中',
    noDevice: '无设备连接',
    start: '一键开播',
    stop: '停止串流',
    started: '开播成功,等待客户端连接',
    stopped: '已停止串流',
    addDevice: '添加新设备',
    gpu: 'GPU',
    quality: '画质',
    bitrate: '码率',
    virtualDisplay: '虚拟显示器:自动',
    gamepad: '虚拟手柄:Xbox',
    audio: '音频:本机+串流',
    recent: '最近连接',
    lastSeen: '上次 {time}',
    avgLatency: '平均延迟 {ms} ms',
    never: '从未连接',
    pairTitle: '添加新设备',
    pairHint: '用 Beam 安卓 App 扫码,或输入 PIN',
    pinExpires: 'PIN {seconds} 秒后刷新',
    waiting: '等待设备…',
    encoder: '编码器',
    codec: '编码格式',
    auto: '自动',
    off: '关闭',
  },
  devices: {
    title: '已配对设备',
    subtitle: '共 {count} 台 · 证书指纹固定,删除后需重新配对',
    empty: '还没有配对的设备',
    emptyHint: '在手机上打开 Beam 扫码即可添加',
    pairNew: '配对新设备',
    permissions: '权限',
    disconnect: '断开',
    wake: '唤醒',
    removeConfirm: '移除后需要重新配对,确认移除「{name}」?',
    removeDone: '已移除 {name}',
    removeTitle: '移除已配对设备',
    removeAction: '确认移除',
    online: '在线',
    offline: '离线',
    streaming: '串流中',
    discovery: '局域网发现',
    requestPair: '请求配对',
    requestTrust: '要连这台主机,需先信任它的证书',
    allow: '允许并配对',
    deny: '拒绝',
    pairedAt: '配对于 {date}',
    fingerprint: '指纹 {fp}',
    allowed: '已允许 {name} 配对',
    denied: '已拒绝 {name} 的配对请求',
    removed: '已移除 {name}',
  },
  connect: {
    title: '连接其他主机',
    subtitle: '在局域网内搜索可用主机,点击即可连接',
    searching: '正在搜索局域网…',
    trusted: '已配对',
    untrusted: '未配对',
    notFound: '没有发现到主机',
    clickToConnect: '点击直接连接',
    clickToPair: '点击配对',
    connect: '连接',
    connecting: '连接中…',
    connected: '已连接到 {name}',
    disconnect: '断开连接',
    reconnecting: '连接中断,正在重连…(保留最后一帧)',
    waitingApproval: '等待主机确认配对…在主机上点「允许并配对」后会自动连上',
    disconnected: '已断开连接',
    showStats: '显示统计',
    hideStats: '隐藏统计',
    e2e: '端到端延迟',
    capture: '抓屏',
    encode: '编码',
    network: '网络 RTT/2',
    decode: '解码',
    render: '渲染/显示',
    resolution: '分辨率/帧率',
    loss: '丢包/FEC 修复',
    jitter: '抖动',
    fps: '帧率',
    bitrate: '码率',
    preview: '串流预览',
    previewHint: '视频帧不经 IPC,画面由客户端侧渲染;此处显示会话状态与统计。',
    overlayOff: '统计浮层已在设置里关闭',
    lastFrameKept: '保留最后一帧',
    pairTitle: '配对主机',
    pairHint: '请输入主机屏幕上显示的 4 位 PIN 码',
    pairAction: '配对',
    pairSubmit: '配对并连接',
    pinInvalid: 'PIN 码必须是 4 位数字',
    pinLocked: '尝试次数过多,已锁定 60 秒',
  },
  settings: {
    title: '设置',
    subtitle: '改动即时生效,串流中修改画质会在 1 秒内重协商',
    quality: '画质',
    encoding: '编码',
    network: '网络',
    audio: '音频',
    interface: '界面',
    input: '输入',
    shareInput: '把本机键鼠回传给主机',
    resolution: '分辨率',
    fps: '帧率',
    bitrate: '码率',
    bitrateAdaptive: '自适应 {min}–{max}',
    hdr: 'HDR',
    matchClient: '匹配客户端分辨率(虚拟显示器)',
    encoder: '编码器',
    codec: '格式',
    preset: '预设',
    captureBackend: '抓屏方式',
    fec: 'FEC 冗余',
    port: '端口',
    allowTailscale: '允许外网(Tailscale)',
    discovery: '局域网自动发现',
    audioMode: '音频模式',
    audioBitrate: '音频码率',
    language: '界面语言',
    theme: '主题',
    showOverlay: '串流统计浮层',
    minimizeToTray: '关闭窗口时收进托盘',
    launchOnStartup: '开机自启动',
    saved: '设置已保存',
  },
  perf: {
    title: '性能统计',
    debugOnly: '仅 Debug 构建',
    snapshot: '刷新快照',
    exportCsv: '导出 CSV',
    exportJson: '导出 JSON',
    exported: '已导出到 {path}',
    stage: '阶段',
    mean: '均值',
    p99: 'P99',
    e2eSeries: '端到端延迟 ms',
    unavailable: '性能统计仅在 Debug 构建可用(§3.13 perf-stats 门控)',
    samples: '采样 {count} 次',
    live: '实时',
    memory: '内存',
    openDebug: '打开 Debug 性能面板',
    debugEntryHint: '仅 Debug 构建可见',
  },
  backend: {
    stub: '本地桩实现',
    crates: '已接 crates/',
    stubHint:
      '当前后端是本地桩实现:设备列表与延迟统计为演示用合成数据,不是实测结果。等 crates/beam-host 等落地后会自动切换。',
  },
  /**
   * 错误码文案表(§3.11)。
   *
   * 后端失败时返回 `{code:"E2001", message, retryable}`,前端**按 code 查表**,
   * 查不到才退回兜底文案(见 `ipc/errors.ts`)。`message` 只用于日志与调试。
   *
   * 覆盖两类码:
   * - 应用外壳自有码(`src-tauri/src/error.rs`);
   * - crates 侧真实会透传上来的码 —— 传输层直接把 `beam-net` 等 crate 的
   *   `code_str()` 当作 code 上报(见 `src-tauri/src/transport.rs`),
   *   所以这里必须逐段覆盖 proto/net/pair/discovery/capture/encode/audio/
   *   input/session/decode-render/config/log/perf/ffi/gamestream。
   *
   * 键名与 `en.ts` 完全一致(由 `Dictionary` 类型在编译期约束)。
   */
  errors: {
    // E1xxx proto(beam-proto §3.1)
    E1001: '与对方的协议版本不兼容,请升级本机 Beam 或改用相同版本的客户端',
    E1002: '收到的网络消息无法解析,通常是版本不一致或数据被损坏,请重新连接',
    E1003: '对方发来了本机不认识的指令,通常是版本不匹配,请更新本机版本',
    E1004: '双方支持的编码格式/分辨率/帧率没有交集,请在设置里调整画质后重试',
    // E2xxx net(beam-net §3.2)
    E2001: '连接超时,请检查网络后重试',
    E2002: '对方拒绝了证书校验,请确认对方是可信主机',
    E2003: '当前网络不支持无连接数据报,串流不可用',
    E2004: '部分视频帧已过期丢弃,画面可能有卡顿',
    E2005: '画面分片重组失败,这一帧无法还原,画面会卡顿一下;请检查网络稳定性或降低码率',
    E2006: '画面丢包过多,冗余数据也不够恢复,画面会卡顿;请降低码率或改用有线网络',
    E2007: '本机缺少用于加密连接的证书,无法开始监听;请重新生成设备身份后重试',
    E2008: '网络读写失败(连接被中断或网卡异常),请检查网络后重试',
    // E3xxx pair(beam-pair §3.3)
    E3001: 'PIN 码不正确,请核对主机屏幕上的数字',
    E3002: 'PIN 尝试次数过多,已锁定 60 秒',
    E3003: '配对二维码已过期,请刷新后重新扫码',
    E3004: '设备指纹与预期不符,可能是中间人攻击,已拒绝',
    E3005: '配对握手消息格式非法(版本不兼容或数据损坏),请重新扫码配对',
    E3006: '无法读写本机证书与私钥,配对无法完成;请检查磁盘权限与剩余空间',
    E3007: '二维码内容不是有效的 Beam 配对码,请重新扫码',
    E3008: '尚未完成配对,等待主机上的用户确认',
    // E4xxx discovery(beam-discovery §3.4)
    E4001: '局域网搜索失败,请检查防火墙与网卡设置',
    E4002: '本机无法在局域网内注册服务,其他设备可能看不到本机,请检查防火墙设置',
    E4003: '无法浏览局域网内的主机,设备列表可能为空,请检查防火墙设置',
    E4004: '本机广播给局域网的信息不合法,主机不会出现在设备列表里',
    // E5xxx capture(beam-capture §3.5)
    E5001: '没有找到可抓取的显示器',
    E5002: '独占全屏应用无法抓取,已尝试切换到 DXGI 模式',
    E5003: '显示设备已丢失,请重新选择显示器',
    E5004: '抓屏后端未编译进当前版本,无法采集画面;请改用带抓屏后端的正式版本',
    // E6xxx encode(beam-encode §3.6)
    E6001: '没有找到可用的硬件编码器',
    E6002: '同时串流的会话数已达上限',
    E6003: '当前参数不被编码器支持,已自动降档',
    E6004: '编码后端未编译进当前版本,无法产生视频流;请改用带编码后端的正式版本',
    E6005: '编码器还在缓冲,本轮暂无输出(这是正常现象:硬编有一帧延迟,下一帧就会有画面)',
    // E7xxx audio(beam-audio §3.7)
    E7001: '音频设备打开失败,请检查默认输出设备是否被其他程序占用',
    E7002: '音频编码失败,本轮音频已丢弃;请检查音频设备后重试',
    E7003: '音频输出失败,声音可能中断;请检查系统音量与输出设备',
    E7004: '音频后端未编译进当前版本,本机不采集也不发送声音',
    // E8xxx input(beam-input §3.8)
    E8001: '未检测到 ViGEmBus 驱动,虚拟手柄不可用;请安装 ViGEmBus 后重启',
    E8002: '没有注入键鼠/手柄输入的权限,请以管理员身份运行 Beam',
    E8003: '输入后端未编译进当前版本,无法向客户端注入手柄输入',
    E8004: '按键码无法映射为虚拟键,这个按键不会被注入;客户端可能用了当前版本不支持的按键',
    // E9xxx session(beam-session §3.9)
    E9001: '当前状态不允许执行该操作',
    E9002: '已经有另一条串流会话,请先断开;也可能是会话依赖未注入',
    E9003: '重连失败,会话已结束(可能是对端掉线或网络中断)',
    E9004: '串流会话已经关闭,重新连接即可继续',
    E9005: '与主机的网络传输失败,画面已中断;请检查网络后重连',
    // E91xx 应用外壳自有的会话类码(crate 侧没有对应码位)
    E9101: '找不到该设备,它可能已经被移除',
    E9102: '找不到该主机,请刷新列表后重试',
    // E10xxx decode / render(beam-decode、beam-render §3.10)
    E10001: '本机没有可用的硬件解码器,无法显示主机画面',
    E10002: '画面解码失败,这一帧无法显示;请重连或降低码率',
    E10003: '图形设备被重置或拔出,解码器需要重建;请重连',
    E10004: '解码后端未编译进当前版本,无法显示主机画面',
    E10011: '渲染设备已丢失,画面无法上屏;请重连或切换显示器',
    E10012: '无法创建交换链,画面无法上屏;请更换显示器或重启 Beam',
    E10013: '画面帧超过上屏时限被丢弃,可能出现卡顿',
    E10014: '渲染后端未编译进当前版本,画面无法显示',
    E10015: '解码画面与交换链格式不匹配,画面无法上屏(当前构建缺少 4:2:0→BGRA 转换路径)',
    // E11xxx config(beam-config §3.14 独占该段)
    E11001: '配置文件解析失败,请检查配置文件格式',
    E11002: '配置项取值不合法,请修改后重试',
    E11003: '配置文件无法读写,请检查磁盘权限与路径',
    // E12xxx 应用外壳(§3.11 host-app)
    E12001: '未知的命令',
    E12002: '设置项取值不合法(分辨率/帧率/编码器等不在支持范围内)',
    E12003: '本地文件读写失败,请检查磁盘权限与路径',
    E12004: '性能统计仅在 Debug 构建可用;请改用 Debug 构建采集性能数据',
    // E13xxx perf(beam-perf §3.13)
    E13001: '性能统计模块未启用,本机无法采集性能数据',
    E13002: '性能数据导出失败,请检查目标路径是否可写',
    // E14xxx ffi(beam-ffi,Android 侧使用)
    E14001: '配对二维码内容无效',
    E14002: 'PIN 码校验失败',
    E14003: '连接主机失败',
    E14004: '无法发现局域网主机',
    E14005: '当前平台不支持该功能',
    // E15xxx gamestream(beam-gamestream)
    E15001: 'GameStream 协议版本不兼容,请升级客户端或改用原生 Beam 协议',
    E15002: '与 GameStream 主机的能力没有交集,无法串流',
    // E16xxx log(beam-log §3.15)
    E16001: '日志初始化失败,请检查日志目录的磁盘权限',
    // 兜底
    E9999: '发生未知错误,请查看日志了解详情',
  },
}

export type Dictionary = typeof zhCN
