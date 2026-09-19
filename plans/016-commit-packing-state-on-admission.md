# Plan 016: 只有成功装包的候选才能占用配额

## Status

- Priority: P1；Effort: S；Risk: LOW；Category: correctness / packing。
- Status: TODO；Depends on: 实现无依赖，预算评测依赖 012；应先于 017 合并。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19；Confidence: HIGH。
- 输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

packing 中放不下的候选会提前消耗 range、entity 和每文件四项配额。后续本可装入的短证据
因此被排除，即使预算还有空间。此修复只改变候选失败后的状态，不改变检索、排序或配额值。

## Current state

[context.rs](../crates/cce-engine/src/context.rs)，`render_item`，约 276–296 行的顺序：

```rust
// 先修改状态：
!seen_ranges.insert(key.clone())
*count += 1;
let first_entity_occurrence = seen_entities.insert(hit.entity_id.clone());
// 最后检查预算：
let body = render_hit(hit);
let tokens = estimate_tokens(&body);
if tokens > budget_tokens.saturating_sub(used_tokens) {
    return None;
}
```

`pack()` 先遍历角色配额，再遍历剩余 hits；两个阶段共享这些集合。
同一 range 的长表示失败后，短表示也可能因 seen_ranges 被拒绝。
现有 integration `context_pack_is_bounded_and_source_linked` 只验证总体上限与引用，
不能发现有空余预算却漏掉可装证据。

## Scope / workflow

- In scope: `crates/cce-engine/src/context.rs`、`crates/cce-engine/tests/integration.rs`、
  `research/output/p016-*`、本计划与 README。
- Out of scope: ranker、token estimator、角色顺序、per-file cap 数字、自动截断正文、公开 schema。
- Drift: `git diff --stat d7c2c2a -- crates/cce-engine/src/context.rs crates/cce-engine/tests/integration.rs`
  并核对输入摘要。保留工作区已有修改。
- 提交风格：`context: commit quotas only for admitted items`；不 push/合并。

## Steps

### 1. 拆成只读判定与成功提交

去重阶段只用 `contains`、`get` 判断，不 insert、不增加计数。
渲染正文并计算 token 后，确认剩余预算足够，再一次性提交 range、entity、文件计数。
`None` 路径必须对三个状态容器都无副作用。保留 address=None 的 entity 去重行为，
也保留带地址的同实体不同 range 可入包的既有规则。

不需要大型 policy 抽象；一个局部 helper 或明确的两段代码即可。作用域内沿用
`Option<ContextItem>` 和纯函数模式，不新增 store 访问。

**Verify**: `cargo test -p cce-engine context::tests --all-features`
→ 新增 `rejected_item_preserves_packing_state` 验证三个容器在拒绝前后相同。

### 2. 测试真实装包行为

在 context 测试模块构造 SearchResult/Hit，沿用 retrieval 测试中的 fixture 结构，
不加载 ONNX、不索引真实仓库。覆盖：

1. 同文件前四个候选太长，后续短候选能进入 pack。
2. 同 range 长表示失败，短表示成功。
3. address=None 的长候选失败，同 entity 短候选仍能进入。
4. 成功的重复 range 仍被排除；已成功装入四项后第五项仍被排除。
5. 角色预选拒绝的候选不会污染随后 rank-order 填充。
6. 零预算、恰好装满、orientation 占用预算、成功项 used_tokens 加和准确。

预算用 fixture 的实际 `estimate_tokens` 计算，避免硬编码一个依赖序列化长度的魔法数字。

**Verify**: `cargo test -p cce-engine context::tests --all-features`
→ 至少覆盖上述六类行为且全部通过；随后执行
`cargo test -p cce-engine --test integration context_pack --all-features` → 既有契约通过。

### 3. 在固定 search hits 上区分 packing 增益

先用同一组 synthetic/保存的 hits 对比前后 pack，排除 retrieval 变化。
再用自测集中现有预算派生案例验证 2K/4K/8K。使用 012 的 item 计量；旧地址展开后的
`budget_compliant` 不能证明超预算或修复成功。
此计划不保证 greedy pack 的覆盖随预算严格单调，只要求拒绝项不消耗状态、上限保持正确。

**Verify / final gates**:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce.yaml . WORKTREE research/output/p016-candidate.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p016-candidate.jsonl --output research/output/p016-smoke-metrics.json
```

上述为 smoke；正式前后比较按 [固定语料操作附录](kernel-paired-benchmark.md) 保存
两套 binary，选择 context adapter 模板，检索同一个 frozen corpus 与 dataset。
`cce.yaml` 使用 frontend 的
默认 dense local；有缓存可离线跑，首次下载需披露。需要纯离线控制时复制适配器到
`research/output/p016-sparse.yaml` 并明确写 `dense: disabled`，两侧使用同一副本。
先构建两个二进制，再跑 benchmark。预期退出 0；报告 item token 和 gold evidence 保留情况。

## Done criteria

- [ ] 所有失败路径对 seen_entities、seen_ranges、ranges_per_file 无副作用。
- [ ] 成功路径仍遵守四项配额、去重与预算；上面的短候选实际装入。
- [ ] used_tokens 等于所有输出 item 的 estimated_tokens 总和且不超预算。
- [ ] 全套门禁、固定 hits 测试、预算派生 smoke 完成，未改 ranking/配额策略。
- [ ] 修改范围正确，README 状态更新。

## STOP conditions / maintenance

若要靠降低 token estimate、提高 file cap 或改变角色顺序让测试通过，停止并缩回本修复。
若发现 pack 中引用地址覆盖了未交付正文，记录给 017，不在此扩大 schema。
将来增加新的拒绝条件，都必须放在 admission 状态提交之前。
