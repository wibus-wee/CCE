# Plan 018: 缓存 dense 索引并仅恢复命中文档

## Status

- Priority: P1；Effort: M；Risk: MED；Category: performance。
- Status: DONE (`2be4811`)；Depends on: 015，复用其正确正文契约；012 用于比较。
  证据：`research/output/p018-perf.json`（86 vs 254 文档语料，命中集固定 K=3 时每查询
  恢复 9.0 文档/读 9.0 artifact，索引解码与向量读取各 1 次），
  `p018-smoke-metrics.json`（dense daemon：recall@20 .7506、no_context_precision 1.0）。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19。
- Confidence: 全量工作模式 HIGH；实际耗时占比未测。
- 输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

暖 daemon 已复用 ONNX session，但每次 dense 查询仍读取、校验并解码完整向量文件，
随后为少量命中恢复全 snapshot 的文档正文。一个文件有许多 symbol 时，源码 artifact
还会被反复读取和哈希。应先消除与候选数量无关的正文工作，再考虑更复杂的向量索引。

这条路径不能解释 sparse 外部评测的全部延迟。Django/SymPy sparse 的约 23 秒中位数
需另做阶段 profile；本计划不承诺把该数值降低到某个目标。

## Current state

[retrieval.rs](../crates/cce-engine/src/retrieval.rs)，约 811 行：

```rust
let index = DenseIndex::decode(&self.store().artifacts().read(digest)?)?;
let dense_hits = index
    .search(&request.query, embedder, request.limit.saturating_mul(3))
    .await?;
let documents = self.store().documents_for_snapshot(&request.snapshot_id)?
    .into_iter().map(|document| (document.document_id.clone(), document))
    .collect::<HashMap<_, _>>();
```

`documents_for_snapshot` 逐文档恢复正文；`source_text` 每次查询 digest，再读取整份源文件后切片。
[artifact.rs](../crates/cce-store/src/artifact.rs) 的 `read` 会验证 BLAKE3，不能直接绕过校验。
每个 dense hit 又单独 `entity_by_id` 获取 symbol name，已有 `entities_by_ids` 可复用批量模式。

[engine.rs](../crates/cce-engine/src/engine.rs) 的 CceEngine 用 OnceCell 缓存 embedder/reranker，
没有 decoded index cache。向量目前是安全 Rust 的 flat dot-product；本计划保持该算法。

## Scope / workflow

- In scope: `crates/cce-engine/src/{engine,retrieval,dense}.rs`、
  `crates/cce-store/src/metadata.rs`、`crates/cce-engine/tests/integration.rs`、
  `research/output/p018-*`、必要的 `research/` 离线计时脚本、本计划与 README。
- Out of scope: ANN、unsafe/mmap、模型替换、graph/FTS 排序、全库永久正文缓存、公开参数扩张。
- Drift: `git diff --stat d7c2c2a -- crates/cce-engine/src crates/cce-store/src/metadata.rs`
  并核对输入摘要；015 已修改正文规则时必须复读该实现。
- 提交风格 `retrieval: materialize only dense hit documents`、`dense: reuse decoded index`；不 push/合并。

## Steps

### 1. 建立能归因的计数和时延基线

区分 resolve/index、lexical、dense artifact load/decode、query embedding、dot product、
document load、graph、witness、packing。已有临时 `eprintln!` 不能作为稳定 benchmark schema。
使用结构化 tracing 或仅测试诊断计数，不往用户正文塞内部实现信息。

记录读入的向量 artifact 次数、恢复的文档数量、不同正文 artifact 读取次数。
计时输出到 `research/output/p018-*`；质量比较固定二进制和语料摘要。

**Verify**: `cargo test -p cce-engine dense --all-features`
→ 诊断不改变结果；一次 warm query 仍能独立区分上述 dense 阶段，不能只报告总 wall time。

### 2. 添加按 document IDs 的批量读取

在 store 添加 snapshot-scoped `documents_by_ids(snapshot_id, ids)`，返回与
`documents_for_snapshot` 相同的 DocumentContent 语义。空输入不读全库；重复 ID 去重；
SQL 使用参数绑定，按 SQLite 参数上限分块，不能字符串拼接未转义 ID。

先一次／分块取 metadata，再按 source snapshot+path 解析 digest，以 digest 分组读取。
同一次查询里每个正文 artifact 只读／验证一次，再按地址切片。保留 015 的
representation-aware 正文语义；摘要的 source address 不能导致误读源码。
批量 entity lookup 填充 symbol_name，输出排名严格按原 dense_hits 顺序恢复，不能依赖
SQL IN 返回顺序。数据库与向量索引出现缺失 ID 时使用明确的错误或诊断，保留原有
降级约定，不静默绑定到其他 snapshot。

**Verify**: `cargo test -p cce-store --all-features`
→ 空/重复/缺失/跨 snapshot/超过单批上限的 ID 用例通过；对于任意目标子集，正文及
metadata 等于旧全量路径投影；多个片段共用源 artifact 时每次请求只读取一次。

### 3. 缓存已验证的不可变 DenseIndex

先采用每个 CceEngine 一个最近使用的缓存槽，键为 artifact digest + embedding profile，
值为 `Arc<DenseIndex>`；比无界全仓库缓存更容易证明内存生命周期。
并发请求对同一 digest 共享初始化，完整 `ArtifactStore::read` 与 decode 成功后才能发布。
可用小范围 mutex 保护槽和共享 OnceCell；不能持同步锁跨 `.await`，同步 I/O 不能阻塞
异步 executor。不同 digest 的进行中请求持有各自 Arc，切换槽不使旧请求失效。

首次读取损坏文件仍报错，不把失败作为成功缓存。缓存中的已验证不可变字节可继续服务；
不要求每次查询重新访问磁盘验证同一内容地址。明确单槽缓存不限制并发旧请求的短期内存，
并记录大索引的峰值；本计划不引入无限历史索引保留。

**Verify**: `cargo test -p cce-engine dense --all-features`
→ 同 digest 两次/并发查询只初始化一次；A→B→A 不串模型和文档；加载失败不留下
可用缓存；profile mismatch 仍被拒绝；未选 dense 的请求不初始化缓存。

### 4. 在可比较负载上验证收益和零质量改变

使用同一完整索引，先预热再记录至少 30 次请求的阶段计时和 p50/p95；独立列冷启动。
大 fixture 增加无关文档，固定命中 K，证明正文 materialization 不再随全 corpus N 增长。
向量 dot-product 本身仍是 O(Nd)，报告中必须保留这个边界。

固定版本分别运行旧/新实现，交错或重复实验避免缓存顺序造成假收益。
确定性测试固定时间输入，比较 hit ID、顺序、score、source address、verdict，计时字段除外应相同。
当前 search 的 recency 使用 `SystemTime::now()`；IssueLocalization/History 跨秒就可能
变分。将时间作为内部 temporal-prior helper 的测试参数注入，生产仍使用真实当前时间，
不新增用户配置。固定 clock 的 replay 才要求逐值一致；真实 run 记录时间因素并报告
自然 recency 变化，不能把它归因于缓存，也不能忽略无时间原因的排名变化。
若出现排名变化，先查读取语义或排序顺序，不把它包装成性能优化的自然代价。

**Verify / final gates**:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-daemon-dense-jina-code.yaml . WORKTREE research/output/p018-candidate.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p018-candidate.jsonl --output research/output/p018-smoke-metrics.json
```

上述为 smoke；正式实验使用 [固定语料操作附录](kernel-paired-benchmark.md) 的两套绝对
binary 路径、同一 corpus/dataset 与独立 data dir，模型/索引 profile 固定；015 变化不能混入这次对照。
先构建两个二进制，后串行 benchmark；本地 dense 首次下载披露，测试使用确定性本地 embedder。
预期所有退出 0。给出阶段差值和 I/O 计数，不规定未经基线支持的倍数目标。

## Done criteria

- [ ] warm 同 digest 不重复读/解码向量 artifact；cache 首次发布前完成完整性校验。
- [ ] dense 查询正文恢复只处理命中文档；每个引用 artifact 每请求至多读取一次。
- [ ] 固定 clock 的 replay 排名、内容、引用与 verdict 一致；真实 run 报告时间输入影响。
- [ ] profile mismatch/corrupt/offline 契约不变。
- [ ] 并发初始化和快照切换测试通过，记录峰值内存与缓存槽行为。
- [ ] 全套 Rust 门禁、smoke、分阶段 cold/warm 计时和 I/O 计数完成。
- [ ] 修改范围正确，README 状态更新。

## STOP conditions / maintenance

若 profile 显示瓶颈主要在 sparse FTS/witness，保留结果并另立优化任务，不顺手调 clause cap。
若需要 unsafe、替换 SQLite 或改变 dense 算法，超出本计划。
若批量 API 与 015 正文契约冲突，先统一正文读取 helper，不能留两套 representation 规则。
后续 ANN 可复用同一按 ID 物化路径，但需独立衡量候选 recall、选择和 packing。
