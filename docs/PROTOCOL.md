# 蓝牙传输线协议与不变量

本文描述助手私有协议 v1、OBEX 对象推送、接收半成品、发送 journal 与平台差异。字节序列以旧版 .NET 实现为兼容基线，参考 legacy/dotnet/src/BluetoothTransfer.Core/Protocol/AsstFrame.cs、AsstMessages.cs、Client/AssistantClient.cs、Server/AssistantServer.cs、AppServices/ObexPacket.cs 与 ObexClient.cs。

## 助手私有协议 v1

服务 UUID：9F3C0E2A-1B4D-4E6F-8A9B-0C1D2E3F4A5B。SDP 服务名 BluetoothTransfer，角色 server。

### 帧头

总长 7 字节，整数小端。

| 偏移 | 长度 | 字段 | 取值 |
| --- | --- | --- | --- |
| 0 | 4 | total length | 整帧字节数，含 7 字节帧头，范围 7 到 1048576 |
| 4 | 1 | magic | 固定 0x41 |
| 5 | 1 | version | 固定 1 |
| 6 | 1 | type | 1 HELLO、2 OFFER、3 DATA、4 ACK、5 DONE、6 CANCEL、7 ERROR |

接收端在解码时必须校验：长度与实际字节数一致、范围合法、magic 与 version 正确、type 已知。任一失败为协议错误，连接关闭；HELLO 之后的接收半成品保留。

分片上限：单帧 1048576 字节减去帧头 7 与 DATA 字段 12，得到 1048557 字节。配置上限与此一致。

### 字符串与整数

字符串为 u32 小端长度加 UTF-8 字节，长度不得超过剩余载荷。整数一律小端。旧实现按 Int32 读到长度后做边界检查，本实现额外要求长度不溢出且字符串为合法 UTF-8。

### HELLO

客户端首个消息必须是 HELLO。

| 顺序 | 类型 | 字段 | 约束 |
| --- | --- | --- | --- |
| 1 | string | transferId | 与 expectedSha256 同值，64 位十六进制 |
| 2 | string | fileName | 对端落盘文件名，接收端清洗为单段名称 |
| 3 | i64 | fileSize | 非负 |
| 4 | i32 | chunkSize | 1 到 1048557 |
| 5 | string | expectedSha256 | 64 位十六进制，大小写不敏感比较 |

接收端在发 OFFER 前校验 fileSize 非负、SHA 为 64 位十六进制、分片大小合法。非法时回 ERROR 码 3，随后关闭连接。

### OFFER

| 顺序 | 类型 | 字段 | 取值 |
| --- | --- | --- | --- |
| 1 | u8 | status | 0 Accept、1 Busy、2 Reject |
| 2 | i64 | resumeOffset | Accept 时为已确认落盘偏移，非负且不大于 fileSize |
| 3 | string | reason | Busy 或 Reject 时的说明 |

Busy 语义：接收端单活跃。第二个并发连接在读到 HELLO 之前即收到 OFFER(Busy, 0, 接收端忙)，随后连接关闭。发送端把 Busy 视为可重试失败，不做数据发送。

### DATA

| 顺序 | 类型 | 字段 | 约束 |
| --- | --- | --- | --- |
| 1 | i64 | offset | 必须等于接收端当前偏移 |
| 2 | i32 | payloadLength | 必须等于载荷剩余字节数，且不大于 fileSize 减 offset |
| 3 | bytes | payload | 分片内容 |

接收端写盘并 flush 到磁盘后才回 ACK。写入越过文件末尾或偏移不连续时回 ERROR 码 3，保留半成品。

### ACK

i64 偏移，表示接收端已落盘并确认的累计偏移。发送端要求 ACK 不小于当前发送偏移且不大于 fileSize，否则协议错误。

### DONE

| 顺序 | 类型 | 字段 | 取值 |
| --- | --- | --- | --- |
| 1 | u8 | ok | 1 成功，其余为失败 |
| 2 | string | hash | 实际文件 SHA-256 十六进制 |

接收端完成时计算整文件 SHA-256。匹配则把半成品改名为正式文件并回 DONE(1, hash)；不匹配删除半成品与 sidecar 并回 DONE(0, 实际 hash)。发送端收到 DONE(0) 视为可重试失败，下一次从 0 开始。

### CANCEL 与 ERROR

CANCEL 载荷为 string reason，表示发送端主动停止，接收端保留半成品。ERROR 载荷为 i32 code 加 string message，code 取值 1 Busy、2 DiskFull、3 Mismatch、4 HashMismatch、5 IoError。

HELLO 之后发生连接中断、协议错误、读取超时、对端 ERROR 或主动取消，接收端一律保留 .btpart 与 .btpart.meta，供下一次同文件名加同 SHA 加同大小重连续传。

### 助手通道状态机

发送端：HELLO → 等 OFFER → 若 Accept 则按 offset 读源文件、发 DATA、等 ACK，循环至 fileSize → 等 DONE。

接收端：读首帧并校验 HELLO → 询问回调或直接接受 → 打开半成品并回 OFFER → 读 DATA 或 CANCEL 或 ERROR → 写盘后回 ACK → 累计到 fileSize 时校验并回 DONE。

### 续传命中条件

接收端半成品路径由清洗后的 fileName 决定，命中条件同时满足：

1. .btpart 与 .btpart.meta 都存在；
2. sidecar 中的传输标识等于 HELLO 的 expectedSha256；
3. sidecar 中的总长等于 HELLO 的 fileSize；
4. 半成品当前长度不大于总长。

命中时返回半成品实际长度作为 resumeOffset，并以实际长度回写 sidecar；未命中时删除残留并从头开始。

## 接收半成品 sidecar

文件名为清洗后的文件名加 .btpart，sidecar 为同名加 .meta，二进制定长 53 字节，小端。

| 偏移 | 长度 | 字段 |
| --- | --- | --- |
| 0 | 4 | 魔数 BTS1 |
| 4 | 32 | 传输标识 SHA-256 |
| 36 | 8 | 已落盘偏移 |
| 44 | 8 | 文件总长 |
| 52 | 1 | 状态位：bit0 暂停，bit1 已校验 |

完成时只做改名，正式文件不追加任何字节。7 天未更新的半成品与孤儿 sidecar 在接收服务启动时清理。

## OBEX 对象推送

OPP 服务 UUID：00001105-0000-1000-8000-00805F9B34FB。

### 包结构

请求与响应均为：1 字节操作码、2 字节大端总长、可选 CONNECT 固定字段、头序列。

CONNECT 固定字段 4 字节：版本 0x10、标志 0、最大包长 u16 大端。请求 CONNECT 携带固定字段；成功响应 0x20 或 0xA0 按带固定字段解析，失败时回退为不带固定字段解析。

操作码：CONNECT 0x80、DISCONNECT 0x81、PUT 0x02、PUT_FINAL 0x82、ABORT 0x7F。

响应码：0x20 OK、0xA0 Success Final、0x90 Continue、0x41 Unauthorized、0xC3 Forbidden、0xD3 Service Unavailable、0xE0 Internal Server Error。0x20、0xA0、0x90 视为成功。

### 头编码

头 ID 高两位决定编码：

| ID 范围 | 编码 | 长度字段 |
| --- | --- | --- |
| 0x00 到 0x3F | Unicode 字符串 | 2 字节大端，值长度加 3 |
| 0x40 到 0x7F | 字节序列 | 2 字节大端，值长度加 3 |
| 0x80 到 0xBF | 单字节值 | 无 |
| 0xC0 到 0xFF | 4 字节大端值 | 无 |

Name 头 ID 0x01，值为 UTF-16BE 后跟 0x0000，默认不带 BOM。开启 BOM 时前置 0xFE 0xFF。值总长不超过 250 字节，超长按 UTF-16 码元边界截断且不留下孤立高位代理。

常用头：Name 0x01、Type 0x42、Body 0x48、EndOfBody 0x49、Length 0xC3、ConnectionId 0xCB、AuthChallenge 0x4C、AuthResponse 0x4D。

### 两阶段 PUT

1. 首包 PUT 仅携带 Name、Type、Length，不携带 Body；等待 0x90 Continue。若对端直接返回成功，则按空对象成功处理。
2. 随后按分片发送 Body 包；非末包操作码 PUT 加 Body 头，末包操作码 PUT_FINAL 加 EndOfBody 头。
3. 每个 Body 包等待响应；Continue 继续，成功码提前结束，0xC3 等错误码终止。
4. 空文件：头包之后直接发 PUT_FINAL 加空 EndOfBody。
5. 完成后发 DISCONNECT；部分 Android 接收端在最终 PUT 后主动断开，DISCONNECT 失败不影响成功判定。

单包大小不超过对端 CONNECT 响应的 max packet length 减 512 字节预留，且不超过配置分片大小。Length 头限制单对象不超过 4 GiB。

### 认证挑战

CONNECT 收到 0x41 时解析 AuthChallenge 的 nonce 标签 0x00，长度 16。摘要为 MD5(nonce 字节 + 冒号 + UTF-8 密码)。AuthResponse 头值为 0x00 0x10 加 16 字节摘要加 0x02 0x10 加 16 字节 nonce。密码只从独立凭据文件读取，不进入配置、不写日志。

### OPP 重试

可重试错误：超时、忙、连接失败、对端提前关闭、0xC3、设备未就绪、中止。不可重试：取消、未配对、不支持、未声明 OPP、文件不存在、超过 4 GiB。重试间隔为 base 乘 2 的 n 次方，最多配置次数加重试次数次尝试。

## 发送 journal

文件为数据目录下 journal/send-journal.bin，格式 BTJ1：4 字节魔数加连续记录，每条记录为 u16 小端长度加条目字节。条目固定 91 字节头加变长文件名与载荷路径，单条不超过 256 字节。

条目字段：版本、8 字节条目 ID、6 字节对端地址、32 字节内容 SHA-256、总长、已确认偏移、租约到期、持有者 PID 与启动标识、创建与更新时间、尝试次数、状态、来源类型、是否打包产物、文件名长度、载荷路径长度、文件名、载荷路径。文件名上限 110 字节；载荷路径相对 pending 目录存储，放不下时拒绝记录并继续发送。

幂等键为对端地址加内容 SHA-256 加落盘文件名，与接收端半成品命中条件一致。状态为 pending、sending、paused。

并发规则：

- 所有读改写都在跨进程文件锁内完成，先写同目录临时文件并 fsync，再原子改名替换。
- 认领条目写入 sending、持有者 PID 与启动标识、租约到期。
- 持有者进程仍存活时条目为忙，任何进程不得重复发送；进程已死则立即允许接管，不必等租约到期；无法判定进程存活时退化为租约判断。
- 租约 6 分钟，长传输每分钟续租。
- 条目 7 天未更新、尝试次数达到 6、载荷大小或 SHA 变化、载荷不存在时放弃。
- 条目上限 200，超出按创建时间淘汰最旧的非忙条目。
- journal 文件损坏时改名留证为 .corrupt.时间戳 并当作空日志，不使启动失败。
- 完成或取消条目时删除其打包产物；原始用户文件永不删除。

## 版本兼容策略

- 助手协议版本固定为 1。接收端拒绝非 1 版本；发送端不发送其他版本。字段顺序、整数宽度与字节序不可变更。
- 新增消息类型会使旧接收端拒绝，因此只能在提升版本号后引入；当前无计划。
- 解码器对 HELLO 等载荷末尾的多余字节保持容忍，对长度字段与实际不符的帧严格拒绝。
- journal 与 sidecar 带魔数与版本字节。sidecar 长度变化即视为不兼容并重新开始；journal 版本字节不匹配按损坏处理并留证。
- 配置只写非默认值，未知配置项被忽略，不阻塞启动。

## 平台后端能力矩阵

| 能力 | Linux BlueZ | Windows WinRT | Mock |
| --- | --- | --- | --- |
| 设备发现 | Adapter1.StartDiscovery 加 ObjectManager 轮询 | DeviceWatcher | 预置列表 |
| 配对 | Agent1 加 Device1.Pair | DeviceInformationCustomPairing | 可脚本化 |
| 配对提示 | Agent1 回调 | PairingRequested 回调 | 回调 |
| 按 UUID 查通道 | L2CAP SDP 查询 | GetRfcommServicesForIdAsync | 可配置 |
| RFCOMM 连接 | AF_BLUETOOTH 加 BTPROTO_RFCOMM | StreamSocket | 内存流 |
| 连接保护级别 | BT_SECURITY socket 选项 | SocketProtectionLevel | 忽略 |
| 连接超时 | 非阻塞 connect 加 poll | 后台线程等待加超时 | 忽略 |
| 服务注册 | ProfileManager1 加 Profile1.NewConnection | RfcommServiceProvider 加 StreamSocketListener | 内存 handler |
| 读超时 | SO_RCVTIMEO | 无 API，依赖上层空闲超时 | 可配置 |
| 通道号返回 | 0 表示由 SDP 决定 | 0 表示由服务句柄决定 | 固定 3 |

## 未验证项

本机为 Linux，无蓝牙硬件、无 BlueZ 守护进程、无系统 D-Bus 的 org.bluez、无 Windows 运行环境。以下路径只完成编译检查或字节级单元测试，未做真机验证：

- BlueZ D-Bus 发现、配对、Agent 提示、Profile 注册与 NewConnection 文件描述符传递。
- AF_BLUETOOTH RFCOMM 地址字节序、连接、服务端数据流。
- L2CAP SDP 查询与真实设备返回的属性结构。
- WinRT DeviceWatcher、配对提示、Rfcomm 服务广播、StreamSocket 收发与读超时行为。
- 与旧版 .NET btrecv 的跨实现互传；当前兼容性依据是逐字段字节布局与旧测试用例。
- OPP 与真实 Android、Windows、车机接收端的互操作。

验收这些路径需要一台有蓝牙适配器的 Linux 主机与一台 Windows 主机，并记录设备型号与操作步骤。

## 体积实测

测量方法见 `docs/ARCHITECTURE.md` 的体积一节。当前实测值：接收最终文件额外字节 0；.btpart.meta 53 字节；journal 条目典型 182 字节、最坏 253 字节；默认配置 0 字节；单条文本记录的 JSONL 行 205 字节、CSV 数据行 94 字节；助手协议开销 0.00725%；OPP 协议开销 0.00923%。10 万条 JSONL 导出的行数由 bt-store 测试断言为 100000，无截断；字节数随记录字段变化。