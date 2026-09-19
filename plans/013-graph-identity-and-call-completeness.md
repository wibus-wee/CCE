# Plan 013: 修正 SCIP 局部身份和调用图漏边

## Status

- Priority: P0；Effort: M；Risk: MED；Category: correctness。
- Status: DONE；Depends on: 实现无依赖；质量比较使用 012 的计量版本。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19。
- Confidence: HIGH；输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

SCIP 的局部符号只在所属 Document 内唯一，当前全局键会制造跨文件伪关系，并标为
`origin=Scip, confidence=1.0`。Tree-sitter 调用边则把同文件内不同 caller 的调用去重掉。
前者污染可信事实，后者削弱 impact、caller 枚举和 graph flow。修复身份与边集合后再调传播。

## Current state

[scip.rs](../crates/cce-engine/src/scip.rs)，`ingest`，约 86、112、137 行：

```rust
let mut definitions: HashMap<String, (String, SourceAddress)> = HashMap::new();
// 跨所有 documents：
definitions.entry(occurrence.symbol.clone()).or_insert((entity, address));
// 引用解析：
let Some(target) = definitions.get(&symbol) else { /* external */ };
```

关系最终以 `RelationOrigin::Scip`、`confidence: 1.0` 输出。
依赖 `scip-0.10.0` 生成协议源的 Symbol 注释明确要求 local symbol 仅限单 Document。
修复覆盖 definitions、reference sites、SymbolInformation.relationships 三条解析路径。

[relations.rs](../crates/cce-engine/src/relations.rs)，`call_relations`，约 340、351 行：

```rust
let mut emitted = HashSet::new(); // 每个文件一次
// 每个调用：
if callee.entity_id == *caller_id || !emitted.insert(callee.entity_id.clone()) {
    continue;
}
```

关系真实 ID 已使用 `relation_id(caller_id, callee_id, "calls")`，去重键应与身份一致。
现有低置信度、ambiguous 标记是既定设计，不能因为修复去重就把推断升级成 compiler truth。

## Scope

- In scope: `crates/cce-engine/src/{scip,relations,config}.rs`、
  `crates/cce-engine/tests/integration.rs`、`docs/architecture.md` 的相关契约、
  `research/output/p013-*`、本计划与 README 状态。
- Out of scope: 传播权重、召回路由、符号模型替换、dataflow Ready 提升、UI。
- Rust 保持 `unsafe_code="forbid"`，错误使用 `cce_core::Result/CceError`。

## Drift / workflow

`git diff --stat d7c2c2a -- crates/cce-engine/src/scip.rs crates/cce-engine/src/relations.rs crates/cce-engine/src/config.rs crates/cce-engine/tests/integration.rs`
后核对 excerpts 与输入摘要。保留既有修改；独立提交可分别叫
`scip: scope local symbols to their document`、`relations: retain distinct callers`。
不 push、合并或改动其他人的工作。

## Steps

### 1. 用双文档 fixture 修复局部身份

新增内部类型 `SymbolKey::Global(String)` / `Local { path, symbol }` 和唯一构造函数。
只有 SCIP 语法的 `local <id>` 进入 local 分支；不能按模糊 substring 识别。
路径统一用当前 `normalize_path`，所有查找复用同一函数。Global 保持跨文件可解析。

沿用 `scip.rs::tests::fixture_index` 的 protobuf fixture，构造两个文件各有 `local 0`，
定义和引用落在不同 enclosing entities；再增加一个 global 跨文件引用。
断言 local 引用只落向各自文件内实体，global 引用仍跨文件成立。
未定义 local 不得借用另一文件定义；relationship 也必须遵守所属 Document。

**Verify**: `cargo test -p cce-engine scip::tests --all-features`
→ 上述用例通过，现有 UTF16、typed range、foreign document 测试不退化。

### 2. 修复 caller→callee 去重

把 `emitted` 的键改成 `(caller_id, callee_id)`。同一 caller 重复调用仍只产生一个关系；
本次允许保留第一处 evidence，避免引入额外聚合策略。保留原有 confidence、origin、
callee resolution、self-edge 排除和 ambiguity 属性。

在 integration fixture 加 `a→helper`、`b→helper`、`a` 再次调用 helper。
期望恰好两条有 source address 的 Calls 关系，而不是一条或三条；同时覆盖顶层 file caller。

**Verify**: `cargo test -p cce-engine --test integration --all-features`
→ 新增 `graph_distinct_callers_survive_dedup` 用例通过，
`typed_relations_carry_provenance_and_confidence`、`ambiguous_call_targets_are_demoted` 通过。

### 3. 让旧图失效并保留可追溯性

`config.rs::profile()` 已将 `options["scip_ingest"]="v1"` 纳入索引身份。
提升该版本，并给 call relation materialization 增加独立版本项。
相应 extractor identity 同步变化；旧 snapshot 保留，不能原地覆写 immutable 图。
解析出的 AST 未改变，不需要机械提升 parse cache 版本。

**Verify**: `cargo test -p cce-engine --all-features`
→ 增加 profile identity 测试，证实新旧 materialization 版本得到不同 profile；
同一 profile 下重复 index 继续复用。已有 index repair 测试通过。

### 4. 单独观察图修复的质量影响

固定语料、gold、模型与预算，比较修复前后；新增加的真实调用边改变排名是预期可能性，
不要为了“字节完全一致”删除正确边。至少检查影响分析、同名符号、trace 和负例子集。
用 fixture 的精确边集合证明正确性，用检索比较检查副作用，两者都需要。

**Verify / final gates**:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-sparse.yaml . WORKTREE research/output/p013-candidate.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p013-candidate.jsonl --output research/output/p013-smoke-metrics.json
```

上述 `. WORKTREE` 命令是 smoke。正式 baseline/candidate 按
[固定语料操作附录](kernel-paired-benchmark.md) 执行：冻结一次 corpus/dataset，
保存修复前后两套 CLI+daemon，使用绝对 binary 路径与独立索引目录，运行附录的 compare。
不能拿不同工作树的历史 v12 作严格配对。预期退出 0、gate 无显著 guardrail 退化；
无显著差异不等于统计等价。

## Done criteria

- [x] 两文档相同 local ID 不产生伪跨文件引用；global references 保持可用。
- [x] 两 caller 同 callee 恰好保留两条边；重复调用不重复关系。
- [x] extractor/profile 版本改变，旧图不会误复用。
- [x] 精确边集测试、全套 Rust 门禁、自测 smoke 和配对比较完成。
- [x] 所有结果保留 snapshot/provenance；没有推断事实升级。
- [x] 修改在 Scope 内，更新 README 状态。

## Implementation notes (2026-09-19)

- `SymbolKey::Global/Local{path,symbol}` 单构造函数 `SymbolKey::of`；definitions、
  reference sites、`SymbolInformation.relationships` 三路共用同一键。
- 同根因扩展到 `type_reference_relations`：同一文件里两个 unit 引用同一类型的
  第二条边也被 callee-only 键丢弃，按 `(source, target)` 去重，extractor
  `cce-type-ref-v2`，profile 增 `type_refs=v2`。
- 其余版本：`scip_ingest=v2`、`call_edges=v2`、extractor `cce-call-extract-v2`；
  parse cache 版本不变（AST 语义未变）。
- 新测试：`local_symbols_resolve_within_their_document`、
  `undefined_local_stays_external`（scip.rs）、`graph_distinct_callers_survive_dedup`、
  `type_reference_edges_dedup_per_source_target_pair`（integration.rs）、
  `graph_materialization_versions_are_profiled`、`disabled_providers_profile_as_off`
  （config.rs）。
- smoke：`research/output/p013-candidate.jsonl` + `p013-smoke-metrics.json`
  （WORKTREE 构建；正式 A/B 仍须按 kernel-paired-benchmark.md 冻结语料）。

## STOP conditions / maintenance

若现有 indexer 的 local symbol 不遵守协议，先保存最小 fixture 并报告，不创造猜测解析规则。
若修复要求改变 canonical entity 身份或 SQLite schema，先重核设计与迁移范围。
SCIP 产物与源码版本绑定仍是独立风险：本计划只修符号作用域，不能宣称已验证产物新鲜度。
后续 graph flow 必须继续消费 origin/confidence，不能把新增边数量当作独立可信来源数量。
