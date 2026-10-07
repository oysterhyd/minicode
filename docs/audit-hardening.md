# Agent / TUI / Desktop 缺陷修复与性能审计

## 1. 范围与结论

- 基线：`release/v1.0.0`，`fd8d15d28545e333db1680b74ed75740b536f536`，开始时工作区干净。
- 覆盖共享 Agent Runtime、恢复/预算/后台任务、文件工具、CommandCode 流协议、SQLite/工件存储、Textual TUI、Python Desktop Bridge、Electron 信任与进程生命周期、React 消息列表与 Diff。
- 三个只读 subagent 分别初审 Agent、TUI、Desktop；父会话统一复现、修改、复核和验证，没有并行写入同一工作区。
- 修复下列 **19 项具体缺陷**，另完成有测量依据的热点优化。保持现有界面布局、权限模式、工具结果顺序与未知副作用不自动重放的原则。
- 新增 **51 项自动化测试**：Python 36 项、Node 15 项；还有随机/等价性断言在单个测试内部重复运行。
- 本轮未使用真实模型凭据、远程模型、生产会话数据库或发布安装包；修改未自动提交。

这是一轮有范围、有回归证据的源码审计，不是“所有漏洞均已消除”的证明。

## 2. 具体 bug 修复清单

P1 表示优先修复的可靠性/安全边界问题，P2 表示功能和边界正确性问题；不是 CVSS 评级。

| ID | 优先级 | 原问题 / 可达触发 | 修复与主要源码 | 回归证据 |
| --- | --- | --- | --- | --- |
| B01 | P1 | 运行后调整累计 token 上限，恢复只读取首次 `SESSION_START`，新上限丢失或恢复为较大的旧值 | `runtime/loop.py` 持久记录 `BUDGET_CHANGED`；`runtime/recovery.py` 读取最新设置，保留默认零不覆盖已保存上限和负值显式关闭的语义；显式恢复覆盖也落库 | `test_audit_runtime.py`：从不限→99、从不限→1、1000→99、1000→-1；验证恢复及耗尽后暂停 |
| B02 | P1 | 后台完成事件已提交，但在 observer await 处取消，结果消息尚未写入；整批通知先被 drain，后续结果一起丢失 | `runtime/loop.py` 原子提交事件+消息，先同步内存再 publish；`tasks/background.py` 新增 peek/ack，仅提交成功后消费通知 | 两个完成任务，首次完成 observer 抛取消，继续投递后数据库/内存各恰好保留一次输出；恢复无失联任务 |
| B03 | P1 | 大文件 edit 同步扫描/复制阻塞事件循环，超过 deadline 仍替换目标文件 | `tools/files.py` 在 UTF-8 验证、分窗查找、复制与最终替换前增加协作取消点；取消只清理临时文件，不让后台线程延迟提交写入 | 慢 decoder 注入超时，目标不变；创建临时文件后立即取消，目标不变且临时文件清理 |
| B04 | P1 | 只含工具参数的 SSE 流在最后才被限额，长行和大量工具 index 可提前耗尽资源；参数 `+=` 反复复制 | `providers/commandcode.py` 在读取/存储阶段检查限额，片段列表最后 join；`core/limits.py` 共享 16,000,000 字节内容、128 工具、16,000,000 字节单行、64,000,000 字节累计 SSE 传输限额；HTTP 错误正文仅采集首 4096 字节 | 参数-only 流在限额处停止；第 129 个工具立即拒绝；未结束长行与 reasoning-only 传输也拒绝，流被关闭 |
| B05 | P2 | 小文件 edit 的 universal-newline 读取把未修改的 CRLF/CR 改为 LF，精确 CRLF old_text 匹配失败；大小阈值两侧行为不同 | `tools/files.py` 精确编辑读取使用 `newline=""`，保留原始换行 | CRLF/混合换行、包含 CRLF 的 old_text，在小/大文件路径均按字节精确替换 |
| B06 | P2 | 前置任务已经 done 后再创建 dependent，仍初始化为 pending，没有未来完成事件解锁它 | `tasks/taskstore.py` 在原有依赖校验事务中收集状态，所有依赖 done 时直接 ready | 已完成依赖可立即 claim；done/running 混合与失败依赖仍 pending |
| B07 | P1 | TUI 两个缓冲 Submitted 事件在 worker 启动前均看见 busy=false，第二个 worker 报错并把真正运行的任务标成 idle，第二条输入丢失 | `ui/app.py` 统一 `_start_turn` 在调度前同步保留 busy；普通输入、排队、continue 都走同一入口 | 实际 Textual 消息队列连续提交两条，第二条排队，两个请求最终各执行一次 |
| B08 | P1 | Ctrl+Q/unmount 未等待当前取消清理就关闭 Runtime/数据库；运行中 aclose 抛 SessionBusyError，晚到回调访问已拆除组件 | `ui/app.py` 受控退出、关闭标志、停止队列、取消并 await owning worker 后关闭资源；shutdown 回调不再改 UI，退休 provider close 任务纳入跟踪，提前结束的 cleanup 异常也主动取回并记录 | 带 50 ms 异步取消清理的 provider，在有排队输入时退出；只执行第一个请求，provider 已关闭、会话 cancelled、无 worker/close 异常 |
| B09 | P2 | 审批超时正常返回 TIME_BUDGET，未经过显式取消分支；审批上方有 Inspector 时取消只检查栈顶，遗留失效弹窗 | `ui/app.py` 保存确切 ApprovalModal；handler finally 在成功/拒绝/超时/取消均清理该实例及其上方临时覆盖层 | 等待审批直到 deadline、Inspector 覆盖审批后 Ctrl+C，均没有残留 ApprovalModal，文件未写入 |
| B10 | P2 | TUI 展开大日志同步读取页面，晚页还反复扫描前缀，卡住输入与取消；晚到页面可能覆盖已经折叠的卡片 | `ui/app.py` 使用 `aread_page` worker、请求 generation、mounted/expanded 检查；reader 绑定原工件 store 和 session，而不是未来被替换的 services | 异步页面等待期间可折叠；迟到结果不显示；原有 60k 命令日志 Pilot 分页用例通过 |
| B11 | P1 | draft 会话 ID 尚未同步时 `/skill` 等请求携带 null，Bridge 替换正在运行的 runtime；旧 close 抛错前引用已清空，审批和 Stop 路由损坏 | `desktop/bridge.py` 同工作区 busy 请求将缺失 ID 绑定现有任务，拒绝 busy 时切换 owner；成功 close 后才移除引用；未知 session 明确拒绝 | 真正等待 bash 审批的 router，stale-ID `/skill` 不改变 runtime/registry；跨工作区替换被拒绝，session-scoped cancel 仍成功 |
| B12 | P1 | Electron 仅凭 `/dist/index.html` 后缀信任任意本地页面，IPC 不校验 sender/frame/URL，未授权页面可触及特权 API | `electron/git-utils.cjs` 比较实际 app index 的规范路径、拒绝非本地主机；`main.cjs` 三个 IPC handler 校验所属 webContents、mainFrame、批准 URL；`src/main.tsx` 阻止文件 drop 默认导航，不阻止 composer mention 处理 | 外来匹配后缀页面、UNC URL、foreign sender、child frame、foreign main-frame URL 全部拒绝；实际 app hash 导航允许。未进行原生拖拽攻击演示 |
| B13 | P1 | Electron quit 只 kill Python，Windows 工具孙进程可以继续修改工作区，Python 无法完成会话清理 | 新增 `electron/bridge-shutdown.cjs`：stdin EOF 后等待退出；5 秒 deadline 后终止整棵树，Windows taskkill /T /F、POSIX 后序终止；`main.cjs` 阻止首次 quit，清理后再 quit，并处理 stdin 异步错误 | EOF 正常退出不强杀、超时只终止一次；当前 Windows 真 Python parent/child 都启动后触发 fallback，子进程不再写延时 marker |
| B14 | P2 | 新 git init 仓库 Changes 能列文件，但 Diff 固定 `git diff HEAD`，首次提交前预览全部失败 | `electron/main.cjs` 仅确认 unborn HEAD 时采用空基线的有界文件预览，普通 Git 错误仍抛出 | 真 Git 仓库中 staged+working-tree 修改与 untracked 文件，在首个 commit 前均正确显示全文件增加 |
| B15 | P2 | unified hunk 中内容以 `-- ` / `++ ` 开头，被误当 `---` / `+++` 文件头；行号和计数错误；末尾 split 空串凭空增加 context | `src/lib/diff.ts` 按 hunk old/new 剩余长度解析内容，hunk 外才识别 metadata，忽略文本终结分隔符 | 类文件头内容、零长度旧 hunk、无末尾换行标记、空内容行、多文件 patch 的行号和类型正确 |
| B16 | P2 | 空字符串 snippet 被视为一行空内容，空文件 diff 出现虚构增删/context 行 | `src/lib/diff.ts` 空输入按零行处理 | empty→empty 返回零行；empty↔hello 只产生真正的单条增/删 |
| B17 | P2 | SQLite transaction 只捕获 yield 内异常，COMMIT 抛错后连接仍处于未回滚事务，后续写入可能落入残留 savepoint | `storage/sqlite_store.py` 把 commit/release 放入同一 try，提交失败时根据活动事务状态回滚 | 延迟外键约束精确注入 commit failure，连接退出事务、失败数据为零，后续写入正常；嵌套回滚保留外层写入。未模拟真实磁盘满 |
| B18 | P2 | 二进制日志前 8192 字节恰好切开 UTF-8 中文/emoji，错误回退为本地编码，归档乱码 | `storage/artifacts.py` 采样用增量 UTF-8 decoder，允许尚未完结的尾部字符 | 8191 ASCII 后接中文+emoji，强制 locale latin-1，归档仍逐字符一致 |
| B19 | P2 | Windows 文本归档默认换行转换把 CRLF 写成 CRCRLF；读取再次归一化，空行与字符 offset/size 不一致 | `storage/artifacts.py` 写、读、分页均显式保留换行；新分页 reader 同样使用 `newline=""` | 普通 spill 和 binary spill 的混合 CRLF/LF/CR 全文、字符数及拼页均与原文完全一致 |

表内 Python 源码路径均相对 `src/minicode/`；Desktop 源码路径相对 `desktop/`。新增 Python 回归文件都位于 `tests/test_audit_*.py`，Node 回归位于 `desktop/tests/*.test.cjs`。

## 3. 算法与数据结构优化

| 热点 | 原实现 | 新实现 / 复杂度 |
| --- | --- | --- |
| 工件定位 | 取全 manifest、排序、构造所有字典后线性找一个 ID；O(A) 数据搬运，排序可带来 O(A log A)，O(A) Python 空间 | 用 `(session_id, artifact_id)` 复合主键查单个 relpath；O(log A) B-tree 搜索、O(1) 返回空间，不缓存失效 manifest |
| Sidebar 活动时间 | 遍历全部事件并 GROUP BY；O(E) | `(session_id,timestamp)` 覆盖索引与每会话 MAX 搜索；约 O(S log E)，保持最大 timestamp 语义，不能用最大 seq 代替（时钟可能回退） |
| 工件连续分页 | 每页从文件头扫描到 offset；完整读取 N 字符、页长 P 时约 O(N²/P) | `storage/paging.py` 用 Unicode 文本 seek cookie 的稀疏索引和一个续读 cursor，顺序遍历 O(N)；有索引的重读在 anchor 覆盖区间约 O(log K+P+64Ki字符)。LRU 最多 32 个文件、每文件最多 4096 个 anchor，带锁及 stat 签名失效 |
| 单任务上下文归档 | 每个闭合 exchange 都重新检查全历史 pairing；O(B²) | Counter/hash 预索引、只检查当前两消息的局部计数；正常 closed-exchange 扫描 O(B)，额外 O(不同调用 ID) 索引。对 duplicate/orphan ID 用随机等价性测试保留原安全语义 |
| 保留近期工具结果 | 每个近期 message 再扫描全部 result 位置 | 先求 recent_start 边界，再做一次 result 判断，避免 M×R 扫描；保留最近结果规则不变 |
| SSE 参数拼接 | 每段 `+=` 重建累积字符串；等长碎片近似 O(B×碎片数) | 有界片段列表，最后一次 join；O(B)，有明确内容/行/传输/工具数量上限 |
| React Feed 位置查找 | 每个普通 row 执行 `items.indexOf(item)`；每次 render 最坏 O(n²) | grouping 时直接携带 source position，render O(n)；thinking row 复用 active-tool 判断。不声称已完成 DOM 虚拟化 |
| snippet diff | N×M LCS 矩阵；大于 250k cells 就把整文件当删除/新增 | 公共前后缀剪裁 + 有界 Myers shortest-edit path，类似文件约 O((N+M)D)、typed traceback O(D²)；探索工作和 trace cells 均有 250k 上限，极端输入退化为仅 changed middle 的 remove/add；输出空间仍 O(N+M) |
| TUI Inspector 历史 | load 全事件后才裁成 deque 200 | 扩展 indexed event-page API，在 SQLite 中 LIMIT 200、倒序读取后恢复正序；只构造 200 个 Event |

符号：A=工件数，E=总事件数，S=会话数，B=消息块规模，D=编辑距离，K=缓存 anchor 数。

**边界**：首次访问一个未索引的很大 character offset、超过 anchor 覆盖上限或索引被 LRU 淘汰后仍须扫描前缀；不是把 character offset 当 byte offset 直接 seek。异常历史下早期归档候选的回退仍可能多次检查，不能把所有 compaction 路径笼统称为 O(B)。Myers 达到工作上限时不保证最短 diff，但始终可重建输入且不会分配无界矩阵。

## 4. 本机前后性能指标

原始数据、重复次数、min/max、内存测量和派生指标见 [`../reports/audit-performance.json`](../reports/audit-performance.json)。

环境：Windows 11 build 26200，Python 3.12.4，Node v22.22.0。基线由 `git archive fd8d15d` 提取真实源码；前后基准串行运行，没有同时运行构建或 pytest。

- Python 每项 3 或 5 次，报告中位数；另外单独进行 tracemalloc 峰值测量，避免它污染时间。
- Node 使用实际 TypeScript 实现，在 VM 中编译执行；2 次预热、7 次采样、显式 GC。
- 排除建库、数据生成、磁盘写入 fixture 等 setup；工件 reader 每轮重新创建，seek 索引为冷状态，但不清空 OS 文件缓存。
- Feed 只测 grouping + 原始位置查找；不包含 JSX、Markdown、浏览器 layout/paint。

| 工作负载 | 基线中位数 | 修复后中位数 | 热点加速 |
| --- | ---: | ---: | ---: |
| 100 会话 / 15 万事件的活动时间查询 | 38.8606 ms | 0.3755 ms | 103.49× |
| 2 万条 manifest 定位一个工件 | 40.8823 ms | 0.1072 ms | 381.36× |
| 100 万多字节字符，连续 50 页 | 88.1000 ms | 11.7335 ms | 7.51× |
| 单请求 1000 组 closed tool exchanges 压缩 | 596.4471 ms | 9.6672 ms | 61.70× |
| 8000 个 256-byte 参数片段，总约 2 MB | 1311.2807 ms | 6.4437 ms | 203.50× |
| 3 万条 Feed grouping + 位置查找 | 99.1775 ms | 0.5882 ms | 168.61× |
| 400 行 / 2 处修改的 snippet diff | 24.6780 ms | 0.0837 ms | 294.84× |

### 内存、额外存储与不利结果

不能只展示有利的指标：

- 工件定位的 **Python traced peak**：10.2470 → 0.0007 MiB。不是进程 RSS；SQLite native cache 不在此测量中。
- 连续分页 traced peak：1.5685 → 0.4747 MiB，降低约 **69.74%**。
- 参数聚合 traced peak：3.9070 → 2.0182 MiB，降低约 **48.34%**，结果受当前碎片 fixture 的对象复用影响。
- Counter 压缩索引换取时间，traced peak **0.4923 → 0.5420 MiB（增加约 10.10%）**。
- 新索引需要额外存储及写维护。在同一 15 万事件 / 2 万工件 metadata fixture 上，SQLite page_count×page_size **22.4375 → 33.15625 MiB（+10.71875 MiB，+47.77%）**。本轮没有宣称数据库空间下降。
- 1 万行 / 3 处修改，旧实现直接 fallback 为 10000 增+10000 删，根本不保留 context；新实现只有真正的 3 增+3 删。输出数组行数 **20000 → 10003（降低 49.985%）**，但计算 **0.8977 → 1.0169 ms（慢约 13.28%，绝对 +0.1192 ms）**。这是换取正确、有用的 diff，不是该场景的计算加速；也不能把数组行数当实际 DOM 渲染行数。

以上均为特定热点的微基准，**不能外推为整应用 100× 加速、UI FPS 增长、真实冷盘吞吐或模型 token/账单节省**。

## 5. 验证结果

| 验证 | 基线 | 最终 |
| --- | ---: | ---: |
| `.venv/Scripts/python.exe -m pytest tests` | 570 passed | **606 passed，131.42 s** |
| `npm test --prefix desktop` | 28 passed | **43 passed，0 failed，0 skipped** |
| `npm run build --prefix desktop` | 未作为初始门运行 | **TypeScript 检查和 Vite 构建通过** |
| 真实 Windows 子进程树测试 | 初审重现 parent.kill 后 child 继续写 marker | 修复后的 deadline fallback 测试通过；确认 child 已启动，再检查不再写延时 marker |
| 随机/等价性 | 缺少这些测试 | 300 组随机 snippet 与参考 LCS 比较最短编辑数和输入重建；80 组含重复/悬空 ID 的 pairing 历史对参考算法逐区间比对；Unicode 随机 offset、异步并发 paging、LRU 上限、索引查询计划断言 |

新增边界回归先在旧实现上失败，再在修复后通过；其中 Agent 回归还对 archived baseline 重新执行确认旧版本失败。

构建仍有原有 `theme-init.js` 非 module 脚本的 Vite 提示；构建成功，未为消除提示改变早期主题初始化语义。`git diff --check` 使用仓库正常的换行配置；不把 Windows CRLF 与强行关闭 autocrlf 后出现的纯换行差异当成源码缺陷。

### 复跑

日常验证：

```bash
.venv/Scripts/python.exe -m pytest tests
npm test --prefix desktop
npm run build --prefix desktop
```

当前代码热点基准（输出到临时文件，不覆盖交付报告）：

```bash
.venv/Scripts/python.exe evals/benchmark_audit.py --output /tmp/minicode-python-after.json
node --expose-gc desktop/tests/benchmark-audit.cjs . /tmp/minicode-desktop-after.json
```

Windows Git Bash 中准备基线：

```bash
mkdir -p /tmp/minicode-audit-baseline
git archive fd8d15d src/minicode desktop/src/lib/diff.ts desktop/src/components/feed/SessionFeed.tsx | tar -x -C /tmp/minicode-audit-baseline
BASELINE="$(cygpath -w /tmp/minicode-audit-baseline)"
.venv/Scripts/python.exe evals/benchmark_audit.py --source-root "$BASELINE" --output /tmp/minicode-python-before.json
node --expose-gc desktop/tests/benchmark-audit.cjs "$BASELINE" /tmp/minicode-desktop-before.json
```

POSIX 环境改用对应虚拟环境 Python，并直接传绝对 baseline 路径，不使用 cygpath。基准会生成自己的临时 SQLite/工件，不读取用户会话。

## 6. 尚未验证或仍存在的风险

1. 没有原生 Electron 恶意拖拽利用演示、打包安装回归、Linux/macOS 本轮验证；IPC 使用 VM 中的真实 main-process 源码验证，Windows 进程树使用真实 OS 子进程验证。
2. 没有真实 provider/MCP 负载或 billing 测量。CommandCode 的提前流量限额有 mock 洪流回归；Anthropic SDK 的底层参数积累没有本轮提前限额压力测试，仍受共享终结响应验证而不是这里的 CommandCode 读取限额保护。
3. 操作系统硬杀、断电、磁盘满、访问权限拒绝等不能靠 Python finally 保证清理。新 graceful shutdown 与 tree fallback 不是 OS sandbox；既有权限机制不能替代沙箱。
4. 旧版本已经出现 completion event 但结果消息缺失的会话，本轮没有自动补造历史结果。未归档的完整输出无法无损重建，继续任务前仍应核实实际副作用。
5. Feed 没有整体虚拟化，长 Markdown、DOM 行数、Bridge.statistics 全量历史读取仍值得真实 UI profile；这轮只消除了已确认的平方级位置查找。
6. 工件冷随机高 offset 首次读取仍扫描前缀；非常长的 edit needle、慢 fsync/文件系统，以及不遵守协作取消的第三方插件仍需后续压力测试。
7. 数据库覆盖索引明显增加存储；如果后续历史规模进一步增长，可评估每 session 的持久化活动时间聚合及增量统计，在严格保持 crash/recovery 语义后减少 O(E) 索引存储。该替代没有在本轮实施。
