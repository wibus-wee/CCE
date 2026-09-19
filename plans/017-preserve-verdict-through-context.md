# Plan 017: 将 search verdict 和交付缺口传到 context

## Status

- Priority: P1；Effort: M；Risk: MED；Category: correctness / API contract。
- Status: TODO；Depends on: 016；012 用于评测接收新字段。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19；Confidence: HIGH。
- 本计划接续 008 中已实现的 SearchVerdict，不能按旧 008 重做它。
- 输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

search 能返回 WeakWitness 及缺口原因，context 输出却丢掉这项信息。
而且搜索发现的证据未必经过预算筛选进入 pack。下游需要同时知道“索引发现了什么”与
“这一包实际交付了什么”，不能把 corpus witness 复制成完整交付的保证。

## Current state

[retrieval.rs](../crates/cce-engine/src/retrieval.rs)，search 的 verdict 逻辑：

```rust
reasons = weak_witness_reasons(&frame, &witness);
if !reasons.is_empty() {
    state = VerdictState::WeakWitness;
}
```

[context.rs](../crates/cce-engine/src/context.rs)，约 146 行：

```rust
let uncertainties = search.missing_capabilities.iter().map(/* ... */).collect();
ContextPack {
    // 有 items / missing_capabilities，没有 search.verdict
}
```

[core/context.rs](../crates/cce-core/src/context.rs) 的 ContextPack 没有 verdict 字段。
`build_witness_report` 的 definitions、relations、bindings 可查询全 snapshot，
仅传入更短 hits 再调用一次，并不能证明所有 witness 都已交付。
`render_hit` 输出 snippet，而 snippet 可能只截取源码前 2,000 字符；原 source address
范围也不能直接当作实际交付范围。

约定：struct 字段 camelCase，enum snake_case；additive optional 字段使用
`#[serde(default, skip_serializing_if = "Option::is_none")]`。
序列化测试参照 `cce-core/src/retrieval.rs::tests::verdict_serializes_external_consumer_shape`。

## Scope / workflow

- In scope: `crates/cce-core/src/{context,lib}.rs`、`crates/cce-engine/src/context.rs`、
  `crates/cce-engine/tests/integration.rs`、
  `research/cce_research/{schema,adapters}.py` 与对应 adapter 测试、
  `docs/api.md`、`docs/benchmarking.md`、`research/output/p017-*`、本计划与 README。
- Out of scope: 改写 search verdict 规则、启用当前禁用的 corroboration gate、
  宣称精确 taint、UI 展示、额外自动检索、扩大 token 预算。
- Drift: `git diff --stat d7c2c2a -- crates/cce-core/src/context.rs crates/cce-engine/src/context.rs research/cce_research`
  并核对摘要；012/016 的预期修改需重新阅读，不能覆盖。
- 提交风格 `context: preserve search verdict and delivery gaps`；不 push/合并。

## Steps

### 1. 增加明确限定作用域的 wire 字段

给 ContextPack 增加可选 `search_verdict: Option<SearchVerdict>`（wire=`searchVerdict`），
逐字保留 search 返回的 verdict。默认 None 表示旧生产者没有提供；不能默认 Answered。
将 WeakWitness reasons 加入 uncertainties，capability 用固定 `evidence_witness` 类别，
不要把它伪装成某个 view 缺失。去重同一原因，保留原 missing_capabilities。

**Verify**: `cargo test -p cce-core --all-features`
→ 新旧 JSON 都能反序列化，optional 缺失为 None，WeakWitness 原因和 drill_downs 无丢失。

### 2. 增加交付报告，而非重新给整个索引作 verdict

增加独立的可选 `delivery_report` 类型，至少包含：实际入包 hit/item IDs、未入包 hit IDs、
对 query distinguishing terms 的已交付正文覆盖／缺失、验证能力状态。
建议固定第一版字段为 `included_item_ids`、`omitted_hits`（document_id + reason）、
`delivered_terms`、`missing_terms`、`witness_verification`。
omission reason 使用 enum：budget、duplicate_range、file_cap、duplicate_entity；
只记录最终未入包的 hit，角色预选失败但后来成功的项不能仍列为 omitted。
可将内部 render/admission 返回值改成携带拒绝原因的 enum，不解析说明字符串。
第一版 verification 只允许 `not_verified` 与 `no_source_evidence`，不增加 Complete/Answered。
在 packer 内从成功 admission 的 hit 建立报告，不能把 orientation、标题、查询回显或
provenance explanation 当作实现证据；匹配对象是交付的 evidence snippet。

只报告可确定事实：哪些内容项被保留、哪些被预算/配额排除、哪些词在实际正文缺失。
source address 是引用，不自动证明引用范围的全文已交付；合成摘要也不能冒充源码。
对 definition/relation 等无法从当前内容和 provenance 精确确认的要求，标为
`not_verified`，不要创造“完整支持”布尔值。若所有 source evidence 被裁掉，应报告
`no_source_evidence`，即使 searchVerdict 是 Answered。

交付报告不提高原 search 的可信等级；它补充裁剪造成的不足。保留 corpus verdict
有助于后续 drill-down，但不能替代 delivery coverage。

**Verify**: `cargo test -p cce-engine context::tests --all-features`
→ tests-only 弱见证不被洗成强答案；唯一证据被预算裁掉出现交付缺口；orientation 中
出现查询词不能消除缺口；只存在于未交付源码尾部的词不算覆盖。

### 3. 保证适配器透传与时延含义正确

CLI/MCP/daemon 通过序列化 ContextPack 自然得到新字段；先验证这一事实，不复制引擎逻辑。
research adapter 保留 `searchVerdict.state` 与 delivery report，空结果和弱见证分别统计。
如果 `_result` 已被 012 修改，沿用其单一 normalization 入口，不另建解释层。

`context()` 当前将 `pack.latency_ms` 再设为 search.latency_ms，漏掉 backlinks 与 packing。
在 context 入口起计时，返回前写入 context 总耗时；search 自己的 latency 仍保留原意义。
如需要两者分段，使用 additive 字段，不能把 query_ms 改成引擎耗时。

**Verify**:

```bash
cargo test -p cce-engine --test integration --all-features
uv run --project research pytest research/tests/test_adapters.py research/tests/test_daemon_adapters.py research/tests/test_metrics.py -q
```

→ 引擎 context JSON 含新字段，旧 fixture 仍可读；latency 测试检查边界包含关系，
不要使用脆弱的毫秒阈值或 sleep。

## Final gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/p017-context.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p017-context.jsonl --output research/output/p017-context-metrics.json
```

先构建后评测，CLI/daemon 必须一起重建。`cce.yaml` 默认 dense local，首下载需披露；
离线控制可在 `research/output/p017-sparse.yaml` 固定 dense disabled。
预期退出 0。正式固定 baseline 按 [配对操作附录](kernel-paired-benchmark.md) 保存
两套二进制并使用 context adapter，比较 items、预算和检索质量，schema 透传不应改变选项顺序；
报告 search verdict 与 delivered 缺口，不以“非空”作为答案正确率。

## Done criteria

- [ ] searchVerdict 与搜索结果一致；旧 JSON 缺省不作答案承诺。
- [ ] WeakWitness 原因在 context 明确可见，不与 view failure 混淆。
- [ ] delivery report 仅来自实际交付正文；裁掉证据和截断尾部的测试通过。
- [ ] 无法验证的关系／定义支持明确为 not_verified，不调用全库 witness 冒充交付证明。
- [ ] adapter 保留两层信号；context latency 包括 backlinks 和 packing。
- [ ] 完整门禁、smoke 完成，修改范围正确，README 状态更新。

## STOP conditions / maintenance

若发现某 adapter 不是透传且必须修改其源码，先记录具体阻塞，再只扩展对应 adapter 范围；
不能在 Web 或 TypeScript 中重写判断逻辑。若需要精确交付字节区间，先设计
content-address 与 citation-address 的独立类型，不能复用现有大范围地址冒充精确性。
此计划交付的是可审查证据与缺口，不保证下游生成答案正确。
