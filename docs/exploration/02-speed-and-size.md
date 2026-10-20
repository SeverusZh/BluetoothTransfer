# 探索报告 02：传输速率提升与发布体积缩小方案

> 任务：第 5 项探索（Slice D2），只做分析与方案设计，**不写生产代码、不实际改造、不运行 dotnet 构建**（本报告中的构建产物尺寸来自工作区既有的 Release 交叉编译产物，见 §4.1）。
> 仓库：`SeverusZh/BluetoothTransfer` @ `2f9dbfb`（2026-10-20，main）。
> 说明：所有「现状」结论均对应到具体源码位置（`文件:行号` 基于 HEAD）；代码事实与任务描述/README 不一致处在文中标注「**代码事实修正**」。
> 预期收益/体积数字凡标「估算」的，均给出依据与公式，**最终需 Windows 真机/实测校准**（§5.5）。

---

## 1. 结论摘要（TL;DR）

### A. 提速

1. **现状主要瓶颈是「经典蓝牙 EDR 空口带宽 + 每包停等的往返开销」，不是磁盘/CPU。**
   OPP 通道每个 32KB PUT 后都要等 `CONTINUE`（`ObexClient.cs:143-164`），助手通道每 32KB DATA 后都要等 ACK 且接收端**先 fsync 再回 ACK**（`AssistantClient.cs:49-74`、`AssistantServer.cs:121-143`、`AsstFileSink.cs:72-82`）。
   EDR 3-DH5 理论 3 Mbps、应用层约 2.1 Mbps（≈262 KB/s）；社区实测经典蓝牙文件传输天花板约 **265.9 KiB/s（≈2.18 Mb/s）**。也就是说，**如果 OPP 已跑到 200–250KB/s，它已经接近空口上限，调参空间只有个位数～十几个百分点。**
2. **保守优化（低风险，建议做）：** OPP chunk **32KB → 64KB（OBEX 包长 16 位，理论上限约 65KB，不能到 128KB）**（`ObexClient.cs:118-121, 204-228`）；
   助手通道 chunk 可到 128–256KB（帧上限 1MB，`AsstConst.cs:18-19`）；
   设备/服务探测与连接按「队列级」复用（现在每个文件都重新发现/探测/连接，`OppPushService.cs:198,282-290`、`AssistantPushService.cs:148,272-277`）；
   SHA-256 从「预扫描一遍再传一遍」改为与连接/发送重叠或按 (路径,大小,mtime) 缓存（`OppPushService.cs:221`、`AssistantPushService.cs:169`、`AsstFileSink.cs:94`）；
   把 OPP 的「总超时 300s」改为**空闲超时**（`OppPushService.cs:286-303`）——否则大文件在慢链路上必然超时，README 宣称的 4GB 上限实际不可达（**代码事实修正**）；
   小文件批量场景把「每文件一次连接」改为一次会话/一次打包。
3. **进阶优化（中风险）：** 助手通道做**滑动窗口 + 累计 ACK**（去掉每 32KB 一次的停等 RTT，理论上可回收 10–20% 的往返开销）、**压缩协商**（文本/日志/源码 2–10× 有效吞吐；已压缩格式跳过）、接收端批量 fsync。
4. **激进方案（产品级，收益数量级提升）：** **蓝牙只做发现/引导，数据走局域网（LocalSend 式 TCP+TLS）**，同 LAN 吞吐 10–100×（100Mbps≈11MB/s vs 蓝牙≈0.2MB/s）。
   **不建议**：BLE L2CAP CoC（Windows WinRT 无公开 API，GATT 吞吐更低）、并行多 RFCOMM 通道（共享同一条 ACL，无带宽增益且接收端单活跃）、Nearby Sharing 集成（无第三方发送 API）、BLE GATT 替代。
5. **不建议为「提速」先做的大改造**：NativeAOT、WinRT 重写、多通道并发。

### B. 缩体

1. **产物最大头不是应用代码，而是两项固定开销：**
   - **WinRT 投影 `Microsoft.Windows.SDK.NET.dll` ≈ 23.7MiB（24,877,600 B）**（本地构建产物实测，见 §4.1）——GUI、btrecv、btcli、Tests 都因 `net8.0-windows10.0.19041.0` 而带；
   - **.NET 8 自包含运行时 + WPF 共享框架**：社区实测空 WPF 自包含单文件约 **130MB**（.NET 9 数据，[lindexi 博客](https://blog.lindexi.com/post/WPF-%E5%9F%BA%E4%BA%8E-.NET-5-%E6%A1%86%E6%9E%B6%E5%92%8C-.NET-6-%E7%9A%84-SDK-%E8%BF%9B%E8%A1%8C%E5%AE%8C%E5%85%A8%E5%8D%95%E6%96%87%E4%BB%B6%E5%8F%91%E5%B8%83)）；本应用再加约 26.6MB 依赖，**估算未压缩 ≈150–160MB**。
2. **投入产出比最高的动作（P0）：**
   - 两个发布产物都加 `<EnableCompressionInSingleFile>true</EnableCompressionInSingleFile>`：实测同类 WPF 空应用 130MB → **55MB（≈42%）**；本应用估算 **自包含 ≈60–70MB、框架依赖 ≈8–10MB**（WinRT 投影是压缩率最高的 managed 程序集，约 4:1）；
   - **发布说明主推「框架依赖 + 单文件压缩」产物**（约 8–10MB 下载，用户需装 .NET 8 Desktop Runtime），自包含仅服务免安装/离线场景；
   - **`btrecv-cli-only.zip` 加单文件压缩**：它本身就是靠蓝牙 OPP 传到对端的引导包（README），当前约 6MB（README 数据），压缩后估算 **3–4MB**，直接缩短 Windows 对 Windows 的引导时间（6MB→3MB 在 150KB/s 下约从 40s 降到 20s）；
   - Release 发布排除/内嵌 PDB（`-p:DebugType=None`），顺手去掉 zip 里多余符号；
   - `SatelliteResourceLanguages` 收敛到 `zh-Hans;en`（自包含估算省 1–3MB）。
3. **看起来能省、实际不建议做（P3/不做）：**
   - `InvariantGlobalization`：**Windows 用 NLS，win-x64 自包含不携带 ICU 数据，体积收益≈0**，而行为风险大（日期/数字格式、非 ASCII 大小写、排序）——**代码事实修正：任务假设「去 ICU 可显著缩体」在 Windows 目标上基本不成立**；
   - 去 SQLite 换 JSON/LiteDB：只省约 **1.86MB 未压缩 / ≈0.8MB 压缩后**，却要重写 `StorageService`（2 张表、迁移、导出/筛选、并发）+ 兼容旧库，**不值得只为一个体积目标做**；
   - 去 Hardcodet 托盘：只省 ~100KB；
   - GUI 开 `PublishTrimmed`：**WPF 不支持裁剪，SDK 直接禁用**（官方文档）；
   - `PublishReadyToRun` 为缩体：体积反而 +20–40%，只有启动速度诉求才考虑；
   - NativeAOT：WPF 不可行；btrecv CLI 需 CsWinRT/AOT 投影支持（高风险、收益不确定，未必打得过 6MB 裁剪包）；**不建议**。
4. **真正能把「框架依赖」产物再砍一个数量级的，是自定义最小 CsWinRT 投影（24.9MB → 估算 1–3MB）**，但需要生成/维护投影、验证 ABI，属于高风险实验（P2，先做实验再决策）。

### C. 投入产出比排序（详见 §5）

| 优先级 | 事项 | 收益 | 成本/风险 |
|---|---|---|---|
| P0 | 单文件压缩（全部产物）+ 主推框架依赖产物 + cli-only 压缩 | 体积 -50%～-70%（下载量） | 极低 |
| P0 | OPP 超时改空闲超时 | 修复大文件「必然失败」，解锁 4GB 上限 | 低 |
| P0 | 队列级设备/服务探测复用 + 连接复用（助手多文件会话，可选） | 小文件批量 5–30% | 低–中 |
| P0 | Release 排除 PDB、卫星语言收敛 | 0.2–3MB | 极低 |
| P1 | chunk 调大（OPP 64KB；助手 128–256KB） | OPP 4–8%、助手 5–15% | 低（需兼容性实测） |
| P1 | 单遍/重叠哈希 | 小文件启动延迟；大文件 pre-delay | 低 |
| P1 | 助手通道滑动窗口 + 累计 ACK | 10–20%（助手通道） | 中（协议版本兼容） |
| P1 | 压缩协商/流式压缩（文本优先） | 可压缩内容 2–10× | 中 |
| P1 | 接收端批量 fsync / 常开 FileStream | 高吞吐传输 2–10%（蓝牙阶段 <2%） | 低 |
| P2 | 自定义最小 CsWinRT 投影 | 框架依赖产物 8–10MB → ~2–4MB（估算） | 高 |
| P2 | 局域网直传（蓝牙引导） | 10–100× | 高（新架构） |
| P3/不做 | InvariantGlobalization、去 SQLite、去托盘、GUI Trim、R2R、BLE L2CAP、Nearby Sharing、并行 RFCOMM | — | — |

---

## 2. 方法与证据

- **代码阅读**（HEAD）：
  - 协议：`Core/Protocol/AsstConst.cs`、`AsstFrame.cs`、`AsstMessages.cs`
  - 助手通道：`Core/Client/AssistantClient.cs`、`Core/Server/AssistantServer.cs`、`AsstFileSink.cs`、`AsstHash.cs`、`IO/SocketAsstTransport.cs`、`Server/AsstRfcommService.cs`
  - OPP 通道：`AppServices/ObexClient.cs`、`ObexPacket.cs`、`SocketObexTransport.cs`、`OppPushService.cs`
  - 编排/队列/配置：`OppSendQueue.cs`、`AssistantPushService.cs`、`SendResumeService.cs`、`TransferJournal.cs`、`PendingPackageStore.cs`、`FileChecksum.cs`、`TransferSpeedTracker.cs`、`AppConfig.cs`、`AssistantDetector.cs`、`OppDiscoveryService.cs`
  - 发布：`src/*/*.csproj`、`tools/publish-*.ps1`
- **本地产物实测**（工作区 Release 交叉编译输出，非我本人运行构建）：
  - `src/BluetoothTransfer/bin/Release/net8.0-windows10.0.19041.0/` 目录合计 **50MB**；
  - `Microsoft.Windows.SDK.NET.dll` = **24,877,600 B（23.7MiB）**，`WinRT.Runtime.dll` = 528,944 B；
  - `Microsoft.Data.Sqlite.dll` = 173,088 B；`SQLitePCLRaw.*` 托管 ≈92KB；`e_sqlite3.dll`（win-x64 原生）= **1,691,648 B**；`Hardcodet.NotifyIcon.Wpf.dll` = 103,424 B；
  - 该目录含 `runtimes/` 下 **19 个 RID** 的 SQLite 原生库（框架依赖、未指定 RID 的构建特点）；`dotnet publish -r win-x64` 只会带 win-x64 一份。
- **官方文档**：单文件压缩、裁剪不兼容性、Native AOT 限制、Windows 全球化（NLS）、Win11 菜单等，链接见文末。
- **不能做的事**：本机无蓝牙硬件、无 Windows，无法实测吞吐/真机行为；不运行 dotnet 构建（PLAN 规定由 verifier 统一构建）；因此吞吐与体积数字以「理论 + 公开实测 + 本地产物测量 + 明确估算」呈现。

---

## 3. A. 提速

### 3.1 现状链路与瓶颈定位

#### 3.1.1 两条通道的协议时序

**OPP（OBEX over RFCOMM）**：每次发送 = 设备发现/解析 → RFCOMM 连接 → SDP 预检 → OBEX CONNECT → 两阶段 PUT → DISCONNECT。
两阶段 PUT：先发「仅含 Name/Type/Length 头的 PUT」，等 `CONTINUE`；随后每个 32KB 分片发一个 PUT（0x02），末片用 PUT Final（0x82），**每个分片都同步等一个响应**。

- 代码：`OppPushService.cs:282-327`（单次推送）、`ObexClient.cs:84-116`（头包）、`ObexClient.cs:118-165`（分片循环：`WriteAsync` → `ReadResponseAsync` → 必须 `Continue`）。
- 分片实际上限：`maxBody = min(chunkSize, MaxPacketLength − 512)`（`ObexClient.cs:118-121`），`MaxPacketLength` 来自 CONNECT 响应（`ObexClient.cs:60-64`）；OBEX 包长字段 16 位，超 65535 直接抛异常（`ObexPacket.cs:217-219`）→ **OPP 分片理论上限约 64KB（65535−512=65023）**。

**助手通道（私有分片协议 over RFCOMM）**：HELLO → OFFER(续传偏移) → 循环 { DATA(32KB) → ACK } → DONE。
明确是 **stop-and-wait**：发送端发一个 DATA 后阻塞等 ACK（`AssistantClient.cs:51-73`）；接收端收到 DATA 后**写盘、`FlushAsync` + `Flush(flushToDisk:true)` 落盘，才写 ACK**（`AssistantServer.cs:121-143`、`AsstFileSink.cs:72-82`）。

#### 3.1.2 瓶颈清单（含量化）

| # | 瓶颈 | 代码证据 | 影响量级（公式/估算） |
|---|---|---|---|
| B1 | **经典蓝牙空口带宽**（硬上限） | 两条通道最终都走 RFCOMM over ACL | EDR 3-DH5 理论 3 Mbps；应用层约 2.1–2.18 Mbps ≈ **250–266 KB/s**（社区实测天花板 265.9 KiB/s）。OPP 实测常见 **100–250 KB/s**（手机端接收确认/弹窗、Android OPP 实现差异） |
| B2 | **逐包停等（OPP）**：每 32KB 一次 OBEX 往返 | `ObexClient.cs:143-164` | 开销 `N × R_obex`，`N=ceil(Size/chunk)`。R_obex≈5–20ms（对端处理+RFCOMM/基带往返）。100MB/32KB=3200 包 → 16–64s；空口本身约 400–1000s → **2–6%**；若对端处理慢（R 40ms）则最高 ~25% |
| B3 | **逐包停等（助手通道）**：每 32KB 等 ACK + ACK 前 fsync + 每块 open/close | `AssistantClient.cs:51-73`、`AssistantServer.cs:133-137`、`AsstFileSink.cs:72-82` | 同上 `N × R_asst`；额外每块 fsync（SSD 0.3–3ms）+ 文件 open/close（约 0.1–0.5ms）+ WinRT 调度。32KB/200KB/s=160ms 空口 → 额外开销 **约 2–5%**（蓝牙阶段）；**若将来链路提速到 1MB/s+，每块 fsync 会变成 20–30% 的大头** |
| B4 | **每次发送重新建连/探测** | OPP：`OppPushService.cs:198`（按地址重新发现）+ `:282-290`（连接+CONNECT）；助手：`AssistantPushService.cs:148`（重新解析设备）+ `:272-277`（AEP/SDP 探测 + 连接） | 每文件固定开销 **约 1.5–4s**（设备枚举 0.3–1.5s、SDP/AEP 0.3–1.5s、RFCOMM/OBEX 连接 0.3–1s）。小文件批量时占比极高：20 个 1MB 文件，传输 20MB/200KB/s≈100s，固定开销 30–80s → **+23–44%** |
| B5 | **SHA-256 多遍读盘** | 发送前预扫：`OppPushService.cs:221`、`AssistantPushService.cs:169`；zip 产物还要多一趟写+读（`OppPushService.cs`（`PushSendHelper` 类）`:429-487`、`PendingPackageStore.cs:28-50`）；接收端完成时整文件再算一遍 `AsstFileSink.cs:94`；恢复时 `TransferJournal.ValidatePayload` 再算一遍 `TransferJournal.cs:457-469` | 数据量：OPP = 2 遍读（hash+发送）；助手 = 2 遍读 + 接收端 1 写 1 读；zip = 3 遍（写 zip、hash、发送）。NVMe 1–2GB/s、HDD 100–200MB/s：1GB 文件 pre-hash **0.5–10s**。相对蓝牙传输时间（1GB≈70–170min）可忽略，但**用户感知的「点了发送后半天不动」主要来自这一遍** |
| B6 | **zip 打包 CPU + 临时文件** | `OppPushService.cs`（`PushSendHelper`）`:436-442`、`PendingPackageStore.cs:35-49`，均为 `CompressionLevel.Optimal` | Deflate Optimal 单线程约 10–30MB/s；1GB 文件夹打包 **30–100s**，相对传输 70–170min <2%，但**用户等待前置时间**明显；对已压缩内容（jpg/png/mp4/docx/zip）打包几乎不缩体、纯耗 CPU |
| B7 | **发送队列逐文件串行 + 每任务重复解析/探测** | `OppSendQueue.cs:170-271`（顺序）、`OppViewModel.cs:459-530` | 顺序本身正确（Android OPP 同时只接受一个传输），但每个 job 都重复 B4；一次队列运行只在开始时探测一次助手（`OppViewModel.cs:479-500`），但 `AssistantPushService` 内部仍按文件重连 |
| B8 | **超时参数导致大文件永远失败（正确性 bug 级）** | `OppPushService.cs:286-303`：连接 30s、**发送阶段总超时 300s** | 上限 ≈ `300s × 吞吐`：100KB/s→30MB；150KB/s→45MB；250KB/s→75MB。README 宣称单文件上限 4GB（OBEX 约束）且建议 ≤2GB，**在当前默认配置下不可能完成**。每个重试都从头传（OPP 无续传），所以重试也救不了 |
| B9 | **每块多次分配/拷贝** | DATA 发送链路：`new byte[chunk]`（`AssistantClient.cs:58`）→ EncodeData 复制（`AsstMessages.cs:116-124`）→ AsstFrame.Build 复制（`AsstFrame.cs:20-33`）→ `WriteBytes(buffer.ToArray())` 复制（`SocketAsstTransport.cs:45-50`）；接收侧 `ReadFrameAsync` 两次分配 + `ReadAsync` 再复制一次（`AsstFrame.cs:49-61`、`SocketAsstTransport.cs:52-61`）。OPP 同样：`ObexHeader.Body(...ToArray())`（`ObexClient.cs:141`）→ `ObexPacket.ToBytes()`（MemoryStream 再复制）→ `WriteBytes(ToArray())` | 每 32KB 约 4 次 memcpy ≈128KB；250KB/s 时 ≈1MB/s 内存带宽，**<0.1% CPU，可忽略**；但若走 10MB/s 的 LAN 传输，分配+复制会变成 GC/CPU 的可见成本（因此列为 LAN 前置改造） |
| B10 | **WinRT 传输层开销** | `DataReader.LoadAsync`/`DataWriter.StoreAsync` 每帧一次调用（`SocketAsstTransport.cs:45-61`、`SocketObexTransport.cs:72-88`） | 每帧 0.1–0.5ms 级；32KB 一帧时占比 <0.5%。**注意：不是主要瓶颈** |

#### 3.1.3 「瓶颈排序」结论

对当前默认配置（32KB chunk、逐包停等、每文件重连）：

1. **B1 空口带宽**（不可调，唯一数量级提升途径是换传输介质）；
2. **B4 建连/探测固定开销**（小文件、多文件批量时占比最高，也是最能「无痛」优化的一项）；
3. **B2/B3 停等往返**（大文件 2–25%，取决于对端处理速度；助手通道可用滑动窗口基本消除）；
4. **B8 超时**（不是速度问题，是大文件可用性问题，优先修）；
5. B5/B6/B7/B9/B10 次要（用户感知/高吞吐场景才重要）。

#### 3.1.4 建议的量化指标（后续实验用）

- 有效吞吐：`bytes_sent / (t_done − t_start)`，同时记录分通道、分设备、分文件大小；
- 建连耗时分解：`设备发现/解析`、`SDP/AEP 探测`、`RFCOMM connect`、`OBEX CONNECT` 各自的毫秒数（现有日志有部分信息：`OppPushService.cs:103-136`、`AssistantPushService.cs:272-277`）；
- 停等开销：`N_chunks`、平均每包往返（日志加时间戳）；
- 接收端落盘：每块 fsync 耗时、总 fsync 占比；
- 前置延迟：`点击发送 → 第一个字节上线`（hash/zip 占用）；
- 资源：进程 CPU、GC 次数/分配字节、峰值内存；
- 稳定性：10 分钟长传的吞吐衰减、重试次数、断线恢复偏移。

### 3.2 分级方案

> 每项给出：预期收益量级（依据/公式）、代价与风险、兼容性、验证方法。
> 收益公式统一为：`T = Setup + Size/BW + N_chunks × Overhead_per_chunk`，`N_chunks = ceil(Size/chunk)`。

#### 3.2.1 保守级（低风险、可配置/可回滚）

**S1. OPP chunk 32KB → 64KB（上限 65023B）**

- 依据：`OppChunkSize` 默认 32768（`AppConfig.cs:9`），实际 body 受 `MaxPacketLength−512` 限制（`ObexClient.cs:118-121`）；OBEX 包长 16 位（`ObexPacket.cs:217-219`）。
- 收益：`N` 减半。若 R_obex=15ms、BW=200KB/s：100MB 由 3200 包降到 1600 包，省 24s / 总约 524s → **约 4.4%**；R=40ms 时约 **8–10%**。
- 代价/风险：部分接收端对较大 OBEX 包处理更慢或直接拒绝（Android OPP 实现差异）；`MaxPacketLength` 由对端决定，配置调大也可能被压回；大包在弱信号/重传下更容易整包重传（RFCOMM 有重传，但更大帧增加单帧丢失代价，影响小）。
- 兼容性：协议层合法（OBEX 允许 ≤65535），但需按设备矩阵实测。
- 验证：同文件同设备 A/B（32 vs 64KB），记录有效吞吐与失败率；重点测 Android/Windows 两种接收端。

**S2. 助手通道 chunk 32KB → 128–256KB（帧上限 1MB）**

- 依据：`AsstHello.ChunkSize` 由发送端决定（`AssistantPushService.cs:279` 用 `OppChunkSize`），接收端按 DATA.Offset 顺序校验（`AssistantServer.cs:126-136`）；`AsstConst.MaxFrameSize=1MB`（`AsstConst.cs:18-19`）。
- 收益：停等次数 `N` 降 4–8×，同时减少 fsync/文件 open-close 次数。R_asst≈20–30ms（含 fsync）时，100MB 由 3200×25ms=80s 降到 800×25ms=20s → **总时间约 -10%**；若用 512KB–1MB 再降，但内存与单帧风险上升。
- 代价/风险：单帧内存峰值（1MB×2 方向）、对端旧版本是否接受大 chunk（HELLO 已带 ChunkSize，接收端当前实现按自身缓冲区读，理论上兼容；但旧接收端 `AsstFrame.MaxFrameSize=1MB` 相同）；断线时丢失量增大 → 续传重传窗口变大（当前逐块 ACK，掉线最多丢 1 块；大块掉线丢一整块，可接受）。
- 验证：Linux harness 已有内存传输测试可先验证正确性（`tests/...MemoryAsstTransportTests` 等）；真机 A/B 吞吐。

**S3. 队列级设备解析/探测缓存 + 连接复用**

- 现状：每个文件 `ResolveDeviceAsync`（OPP：`OppPushService.cs:198` → `OppDiscoveryService.FindByAddressAsync` → 全量 `DiscoverAsync`，含配对表 + 每台已配对设备的 SDP 查询，`OppDiscoveryService.cs:50-97`；助手：`AssistantPushService.cs:148` → 同样的 OPP 发现回退 + `AssistantDetector` AEP/SDP）。
- 方案：
  1. `OppSendQueue` 启动时解析一次设备（拿到 `DeviceId`），把 `DeviceId` 传给每个 job；`SendFileAsync` 增加可选 `device`/`deviceId` 参数避免二次发现；
  2. 助手服务在同一队列运行期间缓存 `RfcommDeviceService`（注意 WinRT 句柄生命周期，见 `AssistantDetector.cs` 的保持父设备存活注释）；
  3. OPP 若设备支持同一 OBEX 会话多 PUT，可在 DISCONNECT 前连续发送多个对象；**先做单设备实验验证**（Android OPP server 可能每对象断开，Windows 接收端行为待测）。
- 收益：每文件省 0.5–2s；20 个 1MB 文件场景 **5–20%**；小文件（10KB×50）场景可达 **数倍**（传输本身仅几秒）。
- 代价/风险：设备状态/已配对状态可能变化，需要失效策略（失败重连时清缓存）；连接复用受对端实现约束，不可强求。
- 验证：队列级 A/B（同一批 20 个文件）测总时长；失败注入（对端忙/断线）验证缓存失效正确。

**S4. 单遍/重叠哈希**

- 现状：发送前完整读一遍算 SHA（OPP/助手），随后发送再读一遍；zip 产物还要先写一遍（`PushSendHelper`/`PendingPackageStore`）；接收端完成时整文件读一遍（`AsstFileSink.cs:94`）；恢复时再读一遍（`TransferJournal.cs:457-469`）。
- 方案（按成本从低到高）：
  1. **把 pre-hash 与连接建立并行**：先启动 SDP/连接（不依赖 hash，助手 HELLO 才需要 hash），hash 与 connect 重叠 → 省掉「串行等待」中的较小者；
  2. **hash 结果缓存**：key = (规范化路径, size, lastWriteTimeUtc)，只在文件未变时复用（用于重试、暂停后继续、重复发送同一文件；注意 journal 已有 SHA，可直接复用 `entry.Sha256`，避免 `ValidatePayload` 恢复时全量重算——可改为「先比对 size/mtime，必要时再 hash」）；
  3. **发送时边读边算**（只能用于不需要预置 SHA 的场景，例如 OPP 的记录校验、或协议 v2 把 SHA 移到 DONE 校验；助手协议 v1 的 HELLO 需要 SHA，受协议约束）。
- 收益：1GB 文件 pre-hash 0.5–10s（介质相关）；对 10 个 500MB 文件 ≈ 每次省 0.5–5s；小文件收益小。
- 代价/风险：缓存键必须包含 mtime/size 且容忍网络盘 mtime 精度；「边读边算」改协议要版本协商，暂不做。
- 验证：用大文件（1–4GB）测「点击发送 → 首字节上线」延迟；正确性用例覆盖文件被改动/同 size 不同内容。

**S5. 空闲超时替代总超时（正确性）**

- 现状：`OppPushService.cs:292-294` `sendCts.CancelAfter(300s)` 覆盖整个 PUT 循环；`OppConnectTimeout=30s`。
- 方案：把发送超时改为「每包/每 30s 无进展才超时」；连接超时保持；重试策略按错误类型（已是 `OppRetryPolicy`）。
- 收益：不是百分比吞吐，而是**让 >30–75MB 的文件在慢链路可能成功**；同时避免「传输 90% 后超时重来」。
- 代价/风险：卡死对端时检测变慢（空闲窗口 30–60s）；需区分「大文件低速」与「真卡死」（阈值取 60s 无字节进展较稳）。
- 验证：限速/弱信号场景下传 200MB+；对端拔蓝牙模拟卡死，确认 60s 内报错。

**S6. 接收端写盘优化（为高吞吐预留）**

- 现状：每 32KB 打开一次 `FileStream`（`FileShare.None`）、`FlushAsync` + `Flush(flushToDisk:true)` 每块一次（`AsstFileSink.cs:72-82`）。
- 方案：接收过程中**保持 FileStream 打开**（单次传输单句柄），每 N 块（如 8 块/256KB）或每 200ms 批量 fsync 一次；ACK 语义仍保证「ACK 偏移 ≤ 已落盘偏移」（批量 fsync 后 ACK 覆盖到该偏移）。或使用 `FileOptions.WriteThrough`/`FileStream.Flush(true)` 降低调用次数。
- 收益：蓝牙阶段 <2%（每块 fsync 0.3–3ms vs 160ms 空口）；**若将来链路提速/局域网传输，可省 20–30%**；同时减少 32KB 级 open/close 系统调用。
- 代价/风险：掉电时最多丢「批量窗口」数据，续传偏移从最后 ACK 开始即可（协议本来就允许最多丢 1 块，改批量后最多丢 N 块，重传代价可控）；实现要小心「ACK 偏移」与「落盘偏移」一致。
- 验证：本地内存传输 harness 可测；真机测 fsync 耗时分布（PerfView/ETW 或代码埋点）。

**S7. 压缩率预估，跳过已压缩格式**

- 现状：文件夹默认 `CompressionLevel.Optimal`；单文件仅 `--zip` 时压缩（`OppPushService.cs`（`PushSendHelper` 类）`:429-487`）。
- 方案：按扩展名/内容采样熵判断：jpg/png/gif/mp4/mov/mp3/flac/zip/7z/rar/pdf(部分)/docx/xlsx/pptx 等 → 直接 `CompressionLevel.NoCompression`（Store）或以「不压缩」方式打包；对文本/日志/源码/CSV/JSON 保持 Optimal 或 Fastest。
- 收益：对不可压缩内容只省 CPU/前置延迟（不改变传输字节数）；对可压缩内容无直接收益（zip 已经压），但避免「打包 1GB 视频白等 60s」。
- 代价/风险：启发式可能误判（如已压缩容器里放文本）；可给配置开关。
- 验证：抽样测量各类型打包耗时与压缩率（本地即可，无需蓝牙）。

#### 3.2.2 进阶级（中风险、协议/架构改动）

**A1. 助手通道滑动窗口 + 累计 ACK**

- 现状：DATA/ACK 一比一停等（`AssistantClient.cs:51-73`、`AssistantServer.cs:121-143`）。
- 方案：协议 v2（HELLO/帧头增加窗口协商，`AsstConst.Version` 已是 1，可协商后升 2）：
  - 发送端允许最多 W 个未确认 DATA（W=4–8），ACK 改为「累计确认偏移」，接收端每收到 K 块（或每 T ms）回一次 ACK；
  - 接收端顺序写盘；ACK 只覆盖「已 fsync 的偏移」；
  - 断线续传：从 `last_acked_offset` 重传（当前实现要求 DATA.Offset==current，改造后发送端需回退到 acked 位置）；
  - 背压：接收端 ACK 里带「可继续窗口」，或发送端以未确认字节上限流控。
- 收益：把 `N_chunks × R_asst` 的停等开销摊薄。R_asst≈20–30ms、32KB/200KB/s=160ms 时，理论吞吐可恢复 **10–20%**；若 chunk 同时调到 128KB，停等开销可忽略（<2%）。
- 代价/风险：协议兼容（旧接收端必须能拒绝/降级；建议 HELLO 里带 `Features` 位，旧字段顺序不变）；内存（W×chunk，W=8×256KB=2MB 可接受）；错误处理/续传语义变复杂（最容易出 bug 的地方）；需要大量内存传输单测（现有 MemoryAsstTransport 很适合）。
- 兼容性：新旧互通 = 新发送端对旧接收端自动降级为 v1 停等；旧发送端连新接收端仍走 v1。
- 验证：无硬件 harness 先跑窗口/乱序/断线/取消用例；真机测 1GB 文件吞吐；断线重连续传偏移正确性。

**A2. 压缩协商（助手通道）/ 流式压缩**

- 现状：助手通道无压缩字段（`AsstMessages.cs:34` `AsstHello(TransferId, FileName, FileSize, ChunkSize, ExpectedSha256)`）；文本/文件夹靠「先写包再发」。
- 方案：
  1. 在 HELLO 增加可选字段（协议 v2）：`Compression`（none/deflate/gzip/brotli）、`UncompressedSize`；SHA 口径改为「压缩后字节」或「原始字节」需明确（建议：传输完整性用压缩后 SHA，业务内容校验用原始 SHA 二选一或双字段）；
  2. 发送端对可压缩内容流式压缩写临时文件（当前已有先例：zip 打包），或直接流式压缩到待发送文件；接收端边接收边解压（或收完解压）——需要 `AsstFileSink` 扩展（当前是「写 .btpart → 完成后哈希 → 改名」）。
  3. 更简单的 MVP：只对 `.txt/.log/.json/.csv/.md/.cs/.xml` 等提供「作为 .zip 压缩后走助手通道」选项，接收端由用户解压（零协议改动，效果 = 用户手动 `--zip`）。
- 收益：`T_eff = Size/BW`，压缩比 r 后变为 `Size/(r×BW) + CPU_compress`。文本/日志 3–10×，源码 2–5×，JSON/CSV 5–15×；不可压缩内容 r≈1 → 无收益甚至更慢（CPU+前置）。
- 代价/风险：CPU（Deflate 10–30MB/s 单线程，Brotli 更慢但压缩率更高）；接收端兼容（必须双方升级）；SHA 语义要重新定义（断点续传的 `.btpart` 是压缩后字节，续传偏移是压缩流偏移——可做，但要写清楚）；旧版本回退。
- 兼容性：只与新版本互通时启用；建议「自动 + 配置开关」，默认仅对明显可压缩类型启用。
- 验证：1GB 文本/日志、100MB JSON、100MB JPEG 三组 A/B；测端到端时间、CPU、接收端解压正确性、断线续传。

**A3. 助手通道多文件会话（连接复用）**

- 现状：一次连接 = 一个对象（HELLO…DONE 后连接关闭）；队列逐文件重连（`AssistantPushService.RunTransferAsync` 每次新连接）。
- 方案：协议 v2 增加「会话层」：一次 RFCOMM 连接内顺序执行多个 HELLO/OFFER/DATA/ACK/DONE（对象之间重置状态），或增加 `MULTI_OFFER`。
- 收益：批量小文件省去每文件 0.5–2s 建连 + 探测（与 S3 叠加后基本消除固定开销）；50 个 100KB 文件场景可提升数倍。
- 代价/风险：协议与状态机改动大；失败隔离（一个对象失败是否断开整个会话）；旧版兼容（协商）。
- 验证：内存传输单测 + 真机批量文件；断线/取消/忙状态用例。

**A4. 流式 zip（打包与发送重叠）**

- 现状：先 `Task.Run` 打包完整 zip，再开始发送（`OppPushService.cs`（`PushSendHelper`）`:436-447`、`PendingPackageStore.cs:28-50`）。
- 方案：对助手通道，可在打包写 zip 的同时把已产出的字节流给发送端（需要「边写边发」+ 长度未知的前置协商；或先打包到临时文件，发送线程从文件读，与打包线程并行——即流水线）。OPP 需要 Length 头，未知长度更麻烦，可保持现状或先打包（`--zip` 可接受）。
- 收益：把 1GB 文件夹的 30–100s 打包时间与传输重叠，**端到端 -2–5%（蓝牙）**；局域网场景 -20–40%。
- 代价/风险：复杂度、临时文件清理、断点续传要求「同一份字节可复现」（当前 journal + pending 包设计就是为此，流式会破坏可复现性——**与现有续传设计冲突，慎做**）。
- 验证：仅局域网/大文件夹场景值得；先做 PoC。

**A5. 接收端哈希校验与 fsync 的批量化（A1/S6 配套）**

- 现状：`CompleteAsync` 整文件再读一遍算 SHA（`AsstFileSink.cs:94`）。
- 方案：A1 滑动窗口下，接收端可在写盘同时增量算 SHA（`IncrementalHash`），完成时直接比对，省掉最后一次全量读；代价是写入路径多一次哈希 CPU（SHA-256 约 400MB/s–1GB/s，远高于蓝牙）。
- 收益：1GB 文件省 1–5s；小文件 <1s。
- 风险：低（哈希失败才需要重读校验）。
- 验证：内存传输 + 损坏数据注入。

#### 3.2.3 激进级（数量级提升；产品/架构级）

**R1. 局域网直传（LocalSend 式）：蓝牙只做发现/引导**

- 思路：两台 Windows 设备先通过现有助手通道（或 BLE/二维码）交换「IP:port + 一次性 token/证书指纹」，然后走 TCP + TLS（或 QUIC）传文件；继续复用现有「分片/续传/journal」协议思路（把 `IAsstTransport` 抽象换一个 TCP 实现即可——这是现有架构的巨大优势：`Core` 已把传输抽象成 `IAsstTransport`，`AssistantClient/Server` 只依赖抽象）。
- 收益：100Mbps LAN ≈ 11MB/s（≈5–11MB/s 有效）→ 对比蓝牙 0.2MB/s **约 50×**；1Gbps ≈ 100×；Wi‑Fi 5 实测 30–60MB/s。1GB 文件从 70–170min → 20–100s。
- 代价/风险：
  - 发现/配对：同 LAN 可用 mDNS/UDP 广播，或用蓝牙通道交换端点（蓝牙此时只传 ~1KB 引导信息）；跨 VLAN/企业网络可能失败；
  - 安全：必须 TLS + 证书 pinning/token + 接收端确认（当前应用层无加密，LAN 上更危险）；防火墙首次放行弹窗；端口固定/随机；
  - 兼容性：对端必须运行新版本（btrecv 也要支持 LAN 监听）；「接收端无需安装软件」的 OPP 场景不受影响（继续走 OPP）；
  - 工作量：2–4 周（传输层实现 + 发现 + TLS + UI + 测试 + 双端发布）；风险中等。
- 验证：同 LAN 1GB/10GB 传输时间、断线续传、TLS 校验、防火墙/网络异常矩阵。

**R2. Wi‑Fi Direct（P2P）**

- 思路：用 WinRT `Windows.Devices.WiFiDirect` 建立 P2P 组，再在其上跑 TCP/TLS。
- 收益：无 AP 场景可用，吞吐通常 2–10MB/s（受设备/信道影响）；比蓝牙仍数量级提升。
- 代价/风险：WinRT Wi‑Fi Direct API 行为复杂（配对/组主/防火墙/兼容性）、Windows 11 上支持状况需实测；UX 比「同 LAN」更重（需要在两端确认、可能打断现有网络）；不建议作为第一优先，R1 的「同 LAN 优先 + 蓝牙引导」更简单。
- 验证：真机双机、无路由器热点场景。

**R3. BLE L2CAP CoC / BLE 传输（评估后：不做）**

- 事实：Windows 公开 WinRT API 提供 BLE GATT（`Windows.Devices.Bluetooth.GenericAttributeProfile`），**没有面向桌面应用的 BLE L2CAP CoC 公共 API**；GATT 通知吞吐受 MTU/连接间隔/逐包事件开销限制，典型远低于经典蓝牙 OPP。即使能拿到 CoC，BLE 4.2/5 的 2M PHY 应用层理论上限约 1.3–1.4 Mbps，与 EDR 3Mbps 接近或更低；BLE 5.x 的 Coded PHY 更慢。
- 结论：**不做**。唯一可能的未来方向（Windows 原生 L2CAP CoC socket API、或第三方驱动）不在本项目可控范围。
- 依据：微软 WinRT 蓝牙 API 文档范围（GATT/RFCOMM）；BLE 吞吐公开数据。需 Windows 实测确认 API 现状，但预期不变。

**R4. 并行多 RFCOMM 通道（评估后：不做）**

- 事实：RFCOMM 多个通道复用同一条 ACL 链路，空口带宽共享；并行不能突破 EDR 上限；当前接收端 `AssistantServer` 单活跃传输（`_active` 标志，忙时回 OFFER(Busy)），且发送端队列顺序发送是刻意设计（Android OPP 同时只接受一个传输）。
- 结论：**不做**。若未来做 LAN 传输，才需要多连接并行（TCP 多流）；蓝牙阶段并行只会增加复杂度。

**R5. Windows「邻近共享 / Nearby Sharing」集成（评估后：不做）**

- 事实：Windows 的 Nearby Sharing 是系统功能（蓝牙发现 + Wi‑Fi 传输），第三方应用**没有公开 API 以程序方式发起/接收**；应用可通过 Share 对话框（`DataTransferManager`）作为分享源参与，但「选择 Nearby Sharing 目标并发送」仍由系统 UI/用户操作完成，无法做到「右键 → 自动走 Nearby Sharing → 某设备」。
- 结论：**不做**。可作为用户文档建议（「同一局域网的两台 Windows 可用系统邻近共享」）。

**R6. 其他激进项（简述，不建议）**

- BLE GATT 作为主通道：吞吐低、无断点续传优势 → 不做；
- USB/网络共享（用户手动）→ 不属于应用职责；
- 上传云盘中转 → 与「蓝牙传输」定位不符、隐私风险 → 不做。

### 3.3 提速推荐路线（不执行，按投入产出排序）

1. **P0（正确性 + 低风险收益）**：S5 空闲超时；S3 队列级探测复用；S4 重叠哈希；S7 压缩率预估；
2. **P0.5（体积/引导时间）**：见 §4（cli-only 压缩直接改善 Windows 互传引导）；
3. **P1（A/B 实验后上线）**：S1（OPP 64KB）、S2（助手 128–256KB）、S6（接收端批量 fsync）、A5（接收端流式哈希）；
4. **P1.5（协议 v2）**：A1 滑动窗口 + 累计 ACK（收益最大的单一协议改造）；A2 压缩协商（文本优先）；A3 多文件会话；
5. **P2（产品级）**：R1 局域网直传（蓝牙引导 + TCP/TLS），预期 10–100×，2–4 周；
6. **不做**：R3/R4/R5/R6。

---

## 4. B. 缩体

### 4.1 现状产物构成（实测 + 估算）

**本地产物实测（HEAD Release 交叉编译，非我运行构建）**

| 文件/目录 | 大小 | 说明 |
|---|---|---|
| `src/BluetoothTransfer/bin/Release/net8.0-windows10.0.19041.0/` 合计 | **≈50MB** | 框架依赖、未指定 RID 的 build 输出（含 19 个 RID 的 SQLite 原生库） |
| `Microsoft.Windows.SDK.NET.dll` | **24,877,600 B（≈23.7MiB）** | WinRT 投影（`net8.0-windows10.0.19041.0` 隐含，所有引用 Core 的项目都会带） |
| `WinRT.Runtime.dll` | 528,944 B | CsWinRT 运行时 |
| `Microsoft.Data.Sqlite.dll` | 173,088 B | 托管 |
| `SQLitePCLRaw.batteries_v2/core/provider` | ≈92KB | 托管 |
| `runtimes/win-x64/native/e_sqlite3.dll` | **1,691,648 B** | 仅 win-x64 一份；build 目录里另有 18 个 RID 副本 |
| `Hardcodet.NotifyIcon.Wpf.dll` | 103,424 B | 托盘 |
| 应用自身 dll（GUI/Core/AppServices） | ≈0.45MB | — |
| `src/BluetoothTransfer.Receiver/bin/.../` 合计 | **≈25MB** | btrecv（无 SQLite）几乎全是 WinRT 投影 |

**未实测、按公开数据估算的部分**

| 项 | 估算 | 依据 |
|---|---|---|
| 空 WPF 自包含单文件（未压缩） | ≈130MB | 社区实测（.NET 9，win-x86，[lindexi 博客](https://blog.lindexi.com/post/WPF-%E5%9F%BA%E4%BA%8E-.NET-5-%E6%A1%86%E6%9E%B6%E5%92%8C-.NET-6-%E7%9A%84-SDK-%E8%BF%9B%E8%A1%8C%E5%AE%8C%E5%85%A8%E5%8D%95%E6%96%87%E4%BB%B6%E5%8F%91%E5%B8%83)） |
| 本应用自包含单文件（未压缩） | **≈150–160MB** | 130MB + WinRT 投影 23.7MiB + SQLite 1.6MiB + 应用/托盘 ≈ 155MB |
| 自包含 + 单文件压缩 | **≈60–70MB** | 空 WPF 130→55MB（≈42%）；新增的 managed 投影压缩率约 4:1 |
| 框架依赖单文件（未压缩） | **≈27–28MB** | 投影 23.7MiB + WinRT.Runtime 0.5MiB + SQLite 1.9MB + 应用 0.5MB |
| 框架依赖 + 单文件压缩 | **≈8–10MB** | managed 程序集 4:1 左右 + 原生 e_sqlite3 约 2:1 |
| `btrecv-cli-only.zip` | 约 6MB（README 数据） | 已 `PublishTrimmed=true`，裁剪掉绝大部分 WinRT 投影 |
| `btrecv-cli-only` + 单文件压缩 | **≈3–4MB（估算）** | 6MB 中主要为托管程序集，压缩率 50–60% |

**发布脚本现状（`tools/publish-gui.ps1`、`tools/publish-receiver.ps1`）**

- GUI：自包含 / 框架依赖两种，均只加 `-p:PublishSingleFile=true`；**没有** `EnableCompressionInSingleFile`、**没有** `IncludeNativeLibrariesForSelfExtract`、没有 R2R、没有裁剪（WPF 也不能裁）。
  - **代码事实修正**：README 称自包含产物为「单文件」，但脚本未设 `IncludeNativeLibrariesForSelfExtract=true`，因此输出实际是「exe + 若干原生 DLL（WPF 原生库、e_sqlite3 等）」的目录，不是真正单文件。若追求真单文件需加该参数（代价见 §4.2）。
- btrecv：自包含 / 框架依赖 / **cli-only 自包含（`PublishCliOnly=true` + `PublishTrimmed=true`）** / cli-only 框架依赖（**未裁剪，且框架依赖模式本身不能 `PublishTrimmed`**）。
  - cli-only 框架依赖 ≈ 全量投影 25MB 级，作为「引导包」没有意义（比 cli-only 自包含 6MB 还大），建议从发布矩阵移除或明确标注实验性质。
- 各 csproj：无 `InvariantGlobalization`、无 `SatelliteResourceLanguages`、无 `PublishReadyToRun`；`Microsoft.Data.Sqlite 8.0.0` 在 GUI/AppServices/Cli/Tests 4 个项目重复引用（同一包，不产生多份体积，但可清理）；`Hardcodet.NotifyIcon.Wpf 1.1.0` 仅 GUI 使用。

### 4.2 缩体方案表

> 体积变化均为「未压缩 → 估算压缩后」的口径；风险/优先级按「只为一个体积目标」判断。

| # | 方案 | 预估体积变化 | 风险 | 兼容性 | 推荐优先级 |
|---|---|---|---|---|---|
| Z1 | **启用 `EnableCompressionInSingleFile=true`**（GUI 两种 + btrecv 两种 + cli-only） | 自包含 ≈150–160MB→60–70MB；框架依赖 ≈28MB→8–10MB；cli-only 6MB→3–4MB | 低：启动时解压到内存，启动 +100–500ms（WPF 本来启动就慢，可接受）；个别杀软对压缩大 exe 扫描更慢 | 无（.NET 8 支持） | **P0** |
| Z2 | **主推框架依赖 + 压缩产物**（README/发布页把 ~8–10MB 作为默认下载，自包含仅离线/免安装场景） | 下载量 -50%～-70% | 低：用户需装 .NET 8 Desktop Runtime（README 已说明） | 高 | **P0** |
| Z3 | **Release 排除 PDB**（`-p:DebugType=None` / 不把 *.pdb 打进 zip） | ≈0.2–0.5MB | 极低（发布包无符号可接受；崩溃分析用单独 symbol 包） | 无 | **P0** |
| Z4 | **`SatelliteResourceLanguages` 收敛**（如 `zh-Hans;en`） | 自包含估算 -1–3MB | 低：非中英文异常消息/资源回退英文；本应用 UI 文案是硬编码中文，不受影响 | 无 | **P1** |
| Z5 | **把 GUI 的 WinRT 投影砍掉 → 自定义最小 CsWinRT 投影** | 24.9MB → 估算 1–3MB（裁剪后 CLI 能到 6MB 说明投影可裁掉 90%+）；框架依赖压缩产物 8–10MB → **估算 2–4MB** | 高：需自建投影/验证 ABI/WinRT 激活；SDK 升级要维护；与 WPF 不能裁剪无关（这是「依赖裁剪」不是 IL trimming） | 中（API 覆盖范围需完整验证） | **P2（先实验）** |
| Z6 | **SQLite → JSON/LiteDB** | 仅 ≈1.86MB 未压缩 / ≈0.8MB 压缩后 | 中：`StorageService` 有 2 张表、迁移、导出筛选、并发/事务语义；旧库迁移；测试面大 | 中 | **P3（不建议仅为体积）** |
| Z7 | **去掉 Hardcodet，改用 Shell_NotifyIcon P/Invoke** | ≈0.1MB | 低但无意义 | 高 | 不做 |
| Z8 | **`InvariantGlobalization=true`** | **≈0（Windows 用 NLS，win-x64 自包含不携带 ICU 数据）** | 高：日期/数字格式、非 ASCII 大小写（仅 ASCII 生效）、字符串比较变 ordinal、排序变化；中文 UI 潜在显示/行为变化 | — | **不做（体积收益不成立）** |
| Z9 | **`PublishReadyToRun`** | 体积 +20–40% | 中（启动略快，但与缩体相反） | 无 | 不做（仅启动优化时单独评估） |
| Z10 | **GUI `PublishTrimmed`** | —（不可用） | — | WPF 不支持裁剪，SDK 禁用（官方文档） | **不可行** |
| Z11 | **btrecv CLI-only 继续裁剪/压缩/AOT** | 已 6MB；压缩 3–4MB；AOT 不确定 | AOT：CsWinRT AOT 需新投影 + 验证；收益未必超过 6MB 裁剪包 | 中 | 压缩=P0；AOT=不做 |
| Z12 | **真·单文件（加 `IncludeNativeLibrariesForSelfExtract=true`）** | 体积不变（不压缩时）；压缩后仍 ≈60–70MB（自包含） | 低–中：首次启动解压到 `%TEMP%\.net`，启动更慢、AV 更敏感；解压目录可被篡改的风险（官方文档提醒） | 无 | P1（若必须单文件分发） |
| Z13 | **精简 PackageReference / 清理无用依赖** | 0（同一包只带一份） | 极低 | 无 | P2（顺手做） |
| Z14 | **架构拆分：GUI（无 WinRT）+ 独立裁剪 Worker 进程** | 框架依赖 GUI 可小到几 MB + Worker 裁剪自包含 6–10MB；自包含 GUI 仍受 WPF 运行时限制 | 高（进程间协议/部署复杂度） | 中 | 不建议（除非要 GUI 也走 AOT/裁剪路线） |

**关于 InvariantGlobalization 的专门说明（代码事实修正）**

任务假设「去 ICU 可显著缩体」，但事实是：
- .NET 官方文档明确「Windows 和 macOS 的全球化数据由操作系统提供」，Windows 上 .NET 使用 **NLS**，自包含 win-x64 运行时包**不包含 ICU 数据文件**（ICU 是 Linux/macOS 的问题）；
- 因此 `InvariantGlobalization=true` 在 Windows 目标上对体积**几乎没有影响**（可能只省下极少量托管代码），真正影响的是行为（[官方 invariant 模式文档](https://github.com/dotnet/runtime/blob/main/docs/design/features/globalization-invariant-mode.md)）；
- 本应用是中文 UI，`DateTime.ToString("o")`、数值/文件大小格式化、字符串比较遍布 UI/存储/排序，开启后有真实回归风险 → **不建议**。

**关于 NativeAOT 的可行性结论**

- GUI（WPF）：官方文档明确 WPF 大量依赖反射，**不支持裁剪**，更不可能 NativeAOT；Windows Desktop 也不在 NativeAOT 支持范围内 → **不可行**。
- btrecv CLI-only：理论上无 WPF，依赖 CsWinRT（`Microsoft.Windows.SDK.NET.dll` + `WinRT.Runtime`）与 WinRT 蓝牙/套接字 API。CsWinRT 官方 AOT 文档要求 **较新的 Windows SDK 投影/CsWinRT 版本**（AOT 兼容的投影与源生成器），且 NativeAOT 在 Windows 上**不支持 built-in COM**（官方限制），WinRT 投影/激活路径存在不确定性；需升级 `WindowsSdkPackageVersion`/TFM 并大量验证。即使成功，体积也未必明显优于 6MB 裁剪包，且失去 JIT 下的调试/兼容性 → **不建议**（可作为独立 spike，但收益低）。
- 若未来 LAN 传输引入纯 .NET networking（无 WinRT），btrecv LAN 形态倒是可以尝试 NativeAOT，这是另一话题。

### 4.3 推荐发布矩阵（体积/启动权衡）

| 产物 | 当前（估算/README） | 建议 | 目标体积（压缩后） | 用户场景 |
|---|---|---|---|---|
| `BluetoothTransfer-frameworkdependent.zip` | ≈28MB（未压缩） | **默认下载**；开单文件压缩；主推 README | **≈8–10MB** | 绝大多数用户（需装 .NET 8 Desktop Runtime） |
| `BluetoothTransfer-selfcontained.zip` | ≈150–160MB（未压缩，非真单文件） | 开压缩；可选真单文件 | **≈60–70MB** | 免安装/离线/受限环境 |
| `btrecv-selfcontained.zip` | ≈150–160MB（估算） | 开压缩 | **≈60–70MB** | 免安装接收端 |
| `btrecv-frameworkdependent.zip` | ≈28MB（未压缩） | 开压缩 | **≈8–10MB** | 已装运行时的接收端 |
| `btrecv-cli-only.zip` | ≈6MB（README） | **开压缩**（P0，因为它是通过蓝牙 OPP 传过去的引导包） | **≈3–4MB** | Windows→Windows 引导/最小化部署 |
| `btrecv-cli-only-frameworkdependent` | ≈25MB（未裁剪，且框架依赖不能裁剪） | **移除或在脚本中标注实验** | — | 无实际价值 |

> 启动速度权衡：压缩单文件首次启动需解压到内存，WPF 冷启动约 1–2s 的基线可能 +100–500ms。建议在 Release 说明中如实标注；如对启动极敏感，可只对 cli-only 与框架依赖产物开压缩，自包含暂不压缩（体积换启动）。

---

## 5. C. 结论

### 5.1 投入产出比排序（只列「值得做」的）

| 排名 | 事项 | 类型 | 预期收益 | 成本 | 依据 |
|---|---|---|---|---|---|
| 1 | 单文件压缩（Z1/Z2/Z3） | 体积 | 下载量 -50%～-70%；cli-only 引导包 6→3–4MB | 极低（改脚本/README） | §4.2 |
| 2 | OPP 超时改空闲超时（S5） | 提速/正确性 | 解锁 >30–75MB 文件；消除「传到 99% 失败重来」 | 低 | §3.1 B8 |
| 3 | 队列级设备/探测复用（S3） | 提速 | 小文件批量 5–30%；50 文件场景数倍 | 低–中 | §3.2.1 |
| 4 | 卫星语言/PDB 收敛（Z3/Z4） | 体积 | 1–3MB | 极低 | §4.2 |
| 5 | chunk 加大 + 接收端批量 fsync（S1/S2/S6） | 提速 | OPP 4–8%；助手 5–15% | 低（需真机 A/B） | §3.2.1 |
| 6 | 单遍/重叠哈希（S4） | 提速（感知） | 大文件前置延迟 -0.5–10s | 低 | §3.2.1 |
| 7 | 压缩率预估（S7） | 提速（感知/CPU） | 不可压缩内容的无谓打包延迟 | 低 | §3.2.1 |
| 8 | 助手通道滑动窗口 + 累计 ACK（A1） | 提速（协议 v2） | 助手通道 +10–20% | 中 | §3.2.2 |
| 9 | 压缩协商（A2） | 提速（内容相关） | 文本/日志 2–10× | 中 | §3.2.2 |
| 10 | 最小 CsWinRT 投影（Z5） | 体积 | 框架依赖压缩产物 8–10MB → 估算 2–4MB | 高 | §4.2 |
| 11 | 局域网直传（R1） | 提速（数量级） | 50–100× | 高（2–4 周） | §3.2.3 |

### 5.2 不建议做的事

- ❌ `InvariantGlobalization=true` 作为缩体手段（Windows 上用 NLS，体积收益≈0，行为风险高）；
- ❌ GUI 开 `PublishTrimmed`（WPF 明确不支持）；
- ❌ `PublishReadyToRun` 用于缩体（体积反增）；
- ❌ 为缩体去 SQLite/去 Hardcodet/重写 WinRT（收益 0.1–2MB 级，成本数天～数周）；
- ❌ NativeAOT（GUI 不可能；btrecv 高风险低收益）；
- ❌ BLE L2CAP CoC / BLE GATT 作为提速主通道（Windows 无公开 CoC API，GATT 更慢）；
- ❌ 并行多 RFCOMM 通道（共享 ACL + 接收端单活跃，无收益）；
- ❌ Nearby Sharing 程序化集成（无公开 API）；
- ❌ 为提速先做「接收端多活跃传输」（Android OPP 同时只接受一个；Windows 助手协议也设计为单活跃，收益存疑、风险高）。

### 5.3 后续可执行实验清单（只列，不执行）

> 建议按顺序做，前 4 项成本极低、可先做；每项都给出验收指标。

1. **发布矩阵实验（体积）**：对 GUI 自包含/框架依赖 × `EnableCompressionInSingleFile` × `DebugType=None` × 卫星语言 组合发布，记录 zip 体积与冷启动时间（3 次取中位数）。验收：框架依赖压缩产物 ≤10MB；自包含压缩产物 ≤70MB。
2. **cli-only 压缩实验（引导时间）**：`btrecv-cli-only.zip` 开压缩，实测「通过 OPP 传到对端」的端到端时间。验收：体积 ≤4MB；引导时间同比 -30% 以上。
3. **超时配置实验（正确性）**：把发送超时改为空闲超时（PoC 用配置模拟），传 200MB/1GB 文件，弱信号/限速下验证不误超时、真卡死能在 60s 内报错。
4. **探测/连接耗时打点**：在 `OppPushService`/`AssistantPushService` 现有日志基础上加时间戳，量化「设备发现 / SDP / AEP / RFCOMM connect / OBEX CONNECT」各段耗时（20 个文件批量场景）。
5. **chunk 大小 A/B（真机）**：OPP 32 vs 64KB；助手 32 vs 128 vs 256KB；每档至少 3 种接收端（Android/Windows/btrecv），记录吞吐/失败率/接收端兼容性。
6. **助手通道滑动窗口 PoC（无硬件）**：用 `MemoryAsstTransport` 在 harness 中实现 W=2/4/8、累计 ACK、断线重传、取消，跑正确性用例；再上真机测 1GB 吞吐。
7. **压缩协商 PoC**：先只做「文本类自动 zip 后走助手通道」的轻量版（零协议改动），测 1GB 文本的端到端时间；再评估协议字段。
8. **LAN 直传 PoC**：把 `IAsstTransport` 换成 TCP 实现（`Core` 的抽象已就绪），先做固定 IP + 明文，再补 TLS/token；同 LAN 测 1GB/10GB。
9. **最小 CsWinRT 投影 spike**：用 cswinrt 为实际用到的命名空间生成最小投影，替换 `Microsoft.Windows.SDK.NET.Ref`，验证 GUI/btrecv 编译与真机运行，记录体积/维护成本。
10. **接收端写盘基准**：在目标机（SSD/HDD）测 32KB 逐块 fsync vs 批量 fsync 的吞吐差异（为未来 LAN 传输和 A1 做准备）。

### 5.4 需要 Windows 实测才能定论的项

1. 真实 OPP/助手通道吞吐（分设备、分文件大小、分 chunk 大小）——本报告只有理论上限与社区数据；
2. 典型接收端（Android 各厂商 / Windows 系统 OPP / btrecv）对各档 chunk 的接受度（`MaxPacketLength`、拒绝大包、处理延迟）；
3. OBEX CONNECT 协商出的 `MaxPacketLength` 实际分布（决定 OPP chunk 能到多大）；
4. 每块 fsync 在目标磁盘（SSD/HDD/USB）上的耗时分布；
5. 断线/重传行为与 RFCOMM 的 L2CAP MTU/帧大小实际值；
6. 单文件压缩后的冷启动时间与杀软影响；
7. CsWinRT AOT / 最小投影在 Windows 上的真实可行性（需 Windows SDK/CsWinRT 版本配合）；
8. Android OPP 是否支持同一 OBEX 会话多 PUT（连接复用收益的关键）；
9. Wi‑Fi Direct / LAN 传输在 Windows 防火墙、企业网络、双网卡下的表现。

### 5.5 风险与前提提醒

- 本报告的「收益百分比」基于 `T = Setup + Size/BW + N×Overhead` 模型和公开数值，**不是实测**；真机可能因设备差异显著偏离；
- 任何协议改动（A1/A2/A3）都必须先定义**版本协商与旧端降级**，否则会破坏「两台 Windows 互传」的现有兼容承诺；
- 提速方案与现有「断点续传 + SHA-256 校验 + 可复现打包」设计强耦合（`TransferJournal`/`PendingPackageStore`），改动前必须重读其不变量；
- 体积方案与「免安装」定位耦合：主推框架依赖产物需要用户装 .NET 8 Desktop Runtime，需在 README/发布页明确。

---

## 6. 附录

### 6.1 源码位置索引（HEAD `2f9dbfb`）

| 主题 | 位置 |
|---|---|
| OPP 分片/停等/包长上限 | `src/BluetoothTransfer.AppServices/ObexClient.cs:118-165, 204-228`；`ObexPacket.cs:194-228` |
| OPP 每次发送重连/超时 | `OppPushService.cs:282-327`（发送）、`:230-279`（重试）、`:198`（设备解析）、`:103-136`（SDP 预检）、`:286-294`（超时） |
| 助手通道停等/ACK/fsync | `Core/Client/AssistantClient.cs:49-74`；`Core/Server/AssistantServer.cs:121-143`；`Core/Server/AsstFileSink.cs:72-109` |
| 助手通道建连/探测 | `AppServices/AssistantPushService.cs:261-347`；`Core/Discovery/AssistantDetector.cs:120-247` |
| 帧/载荷/拷贝 | `Core/Protocol/AsstFrame.cs:20-61`；`AsstMessages.cs:116-133`；`Core/IO/SocketAsstTransport.cs:45-61`；`AppServices/SocketObexTransport.cs:72-88` |
| 哈希/Journal 恢复 | `AppServices/FileChecksum.cs`；`Core/Server/AsstHash.cs`；`TransferJournal.cs:445-469` |
| zip 打包 | `AppServices/OppPushService.cs`（`PushSendHelper` 类）`:429-487`；`PendingPackageStore.cs:28-82` |
| 队列/配置 | `OppSendQueue.cs:83-297`；`AppConfig.cs:8-45`；`TransferSpeedTracker.cs` |
| 发布脚本 | `tools/publish-gui.ps1`、`tools/publish-receiver.ps1` |
| 项目依赖 | `src/BluetoothTransfer/BluetoothTransfer.csproj`、`AppServices/…csproj`、`Receiver/…csproj`、`Cli/…csproj`、`tests/…csproj` |

### 6.2 参考链接

- [Create a single file for application deployment（含 `EnableCompressionInSingleFile`）](https://learn.microsoft.com/en-us/dotnet/core/deploying/single-file/overview)
- [Known trimming incompatibilities（WPF 不支持裁剪）](https://learn.microsoft.com/en-us/dotnet/core/deploying/trimming/incompatibilities)
- [Native AOT deployment overview（限制：无 built-in COM、不支持 WPF 等）](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)
- [.NET AOT support in C#/WinRT（CsWinRT AOT 要求新投影/版本）](https://github.com/microsoft/CsWinRT/blob/main/docs/aot.md)
- [.NET Core Globalization Invariant Mode（Windows 用系统 NLS，ICU 只在 Linux/macOS）](https://github.com/dotnet/runtime/blob/main/docs/design/features/globalization-invariant-mode.md)
- [WPF is not trim-compatible（dotnet/wpf #3811）](https://github.com/dotnet/wpf/issues/3811)
- [WPF 完全单文件发布与压缩实测（130MB → 55MB）](https://blog.lindexi.com/post/WPF-%E5%9F%BA%E4%BA%8E-.NET-5-%E6%A1%86%E6%9E%B6%E5%92%8C-.NET-6-%E7%9A%84-SDK-%E8%BF%9B%E8%A1%8C%E5%AE%8C%E5%85%A8%E5%8D%95%E6%96%87%E4%BB%B6%E5%8F%91%E5%B8%83)
- 蓝牙实测天花板（约 2.18 Mb/s ≈ 265.9 KiB/s，Bluetooth 6.0 设备对照）：[腾讯云开发者社区文章](https://cloud.tencent.cn/developer/article/2742888)
- [Windows 11 周边共享/上下文菜单扩展设计说明](https://blogs.windows.com/windowsdeveloper/2021/07/19/extending-the-context-menu-and-share-dialog-in-windows-11/)（与 NearBy Sharing 能力边界相关）

---

## 7. 附：代码事实修正清单（相对任务描述/README）

| # | 描述/假设 | 代码事实 | 影响 |
|---|---|---|---|
| 1 | 「OPP 分片 32KB 可加大到 128KB」 | OBEX 包长字段 16 位（`ObexPacket.cs:217-219`），且预留 512B 头，**OPP 分片上限约 64KB**，128KB 不可行 | OPP chunk 方案只能到 64KB；助手通道可到更大 |
| 2 | 「SHA-256 两遍读盘」 | 发送端确为 2 遍（预扫 + 传输），但接收端还有 1 遍（完成校验）、zip 有 3 遍、恢复时再多 1 遍 | 优化点比「两遍」更多；收益要分场景 |
| 3 | 「去 ICU 可显著缩体」 | Windows 目标用 NLS，win-x64 自包含不带 ICU 数据；InvariantGlobalization 体积收益≈0 | 该缩体项应取消，改为行为风险项 |
| 4 | 「自包含单文件产物」 | 发布脚本未设 `IncludeNativeLibrariesForSelfExtract=true`，实际为 exe + 原生 DLL 目录 | README 表述需修正；真单文件需加参数 |
| 5 | 「单文件上限 4GB，建议 ≤2GB」 | OBEX 4GB 是协议上限，但默认发送总超时 300s，实际吞吐下 **30–75MB 就会超时**；且 OPP 无续传，重试从头来 | 需先修超时，否则大文件承诺不成立 |
| 6 | 「btrecv-cli-only 约 6MB」 | README 数据，本机未复测（不运行构建）；但同目录框架依赖 btrecv 25MB 主要是投影，裁剪版 6MB 量级合理 | 该结论方向可信，精确值需 verifier/发布侧测量 |
| 7 | 「助手通道 chunk 默认 32KB」 | `AsstConst.DefaultChunkSize` 常量存在（`AsstConst.cs:15-16`），但实际发送用的是 `AppConfig.OppChunkSize`（`AssistantPushService.cs:279`） | 调大 chunk 需改 `OppChunkSize` 或给助手通道独立配置项（建议后者） |
