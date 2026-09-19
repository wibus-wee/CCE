# Plan 015: 持久化正确的检索正文并修复注释提取

## Status

- Priority: P1；Effort: M；Risk: MED；Category: correctness / retrieval。
- Status: TODO；Depends on: 012 用于质量验收；与 013 都改 `config.rs`，应顺序合并。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19；Confidence: HIGH。
- 输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

FTS 收到的是 SymbolSummary 描述，dense 收到的却是原始源码。FileDescriptor 也被恢复为
整份源码，而非有界描述。两个通道没有使用同一种 representation 的正文，因此不能从现有
模型比较推断“摘要桥梁已经无效”。修复后每个 representation 的存储、读取和 embedding
输入必须一致，同时保留独立的 canonical source address。

## Current state

[engine.rs](../crates/cce-engine/src/engine.rs)，约 1106 行：

```rust
let summary_body = symbol_descriptor(&file.relative_path, file_text, parsed, index);
// RetrievalDocument:
representation: RetrievalRepresentation::SymbolSummary,
body_artifact_digest: file_artifact.clone(),
address: Some(unit_address.clone()),
// IndexedDocument:
body: summary_body,
```

[metadata.rs](../crates/cce-store/src/metadata.rs)，`documents_for_snapshot`，约 2280 行：

```rust
let text = if let Some(address) = &address {
    self.source_text(address)?
} else {
    let bytes = self.artifacts.read(&digest)?;
    String::from_utf8(bytes) /* 错误转换略 */
};
```

`build_dense_view` 用上述恢复结果调用 `DenseIndex::build`；后者直接 embed `document.text`。
FileDescriptor 同样引用 file artifact，且 address=None。
现有正确范例是 `engine.rs` 的 RoleSummary：先 `put_bytes`，把 artifact 加进
`records.artifacts`，metadata 事务引用它。不要把大正文再加进 SQLite 业务表。

另一个输入问题：parser 的 `start_line=row+1`，`leading_doc_comment` 却从
`cursor=start_line` 减一后读取声明行，遇到普通声明立即退出，漏掉上一行注释。

## Scope / workflow

- In scope: `crates/cce-engine/src/{engine,config,dense}.rs`、
  `crates/cce-store/src/{metadata,artifact}.rs`、`crates/cce-core/src/retrieval.rs` 的正文契约注释、
  `crates/cce-engine/tests/integration.rs`、`docs/architecture.md`、`research/output/p015-*`、
  本计划与 README。`dense.rs` 仅用于验证 embedding 输入，不改相似度算法。
- Out of scope: 更换模型、训练、权重调参、ANN、schema 大迁移、把摘要当源码事实。
- Drift: `git diff --stat d7c2c2a -- crates/cce-engine/src crates/cce-store/src crates/cce-core/src/retrieval.rs`
  后核对 excerpts 与输入摘要，不覆盖已有改动。
- 提交风格 `index: persist symbol descriptor bodies`，注释提取另一个逻辑提交；不 push/合并。

## Steps

### 1. 固定正文与引用的契约

先为生产中实际持久化的每个 representation 列出读取规则。
RawCode/TestBehavior 的正文来自 source artifact 的精确片段；FileDescriptor、
SymbolSummary 的正文来自独立 descriptor artifact；已有 Role/Module/Knowledge/
Commit 类正文保持其专有 artifact 语义。

推荐最小改动：为 FileDescriptor、SymbolSummary 保存独立正文 artifact，读取端按
representation 明确选择正文，不能再用 `address.is_some()` 决定正文来自哪里。
source address/region 继续指向源码，绝不能改成摘要的字节偏移。
用已持久化的 `generated_by` 区分旧、新正文格式：新写入的 file/symbol descriptor
分别使用 `cce-file-descriptor-v2`、`cce-symbol-descriptor-v2`，其正文直接读专用 artifact。
store 查询增加该列；v1 或缺失版本沿用旧的 address/source slicing 语义。
不能只按 representation 一刀切，否则旧 SymbolSummary 会从“源码片段”变成“整份文件”。
旧 snapshot 保持原语义；新 profile 不复用旧向量。未知版本报明确错误，不猜格式。

**Verify**: `cargo test -p cce-store --all-features`
→ 新增 round-trip fixture：带 source address 的 summary 读回 descriptor；raw 仍读源码；
旧 v1 summary 仍读原源码片段；role/history 读取不变；损坏 artifact 仍返回明确错误。

### 2. 按既有 artifact 模式写入描述

对两个 descriptor 跟随 RoleSummary 的 `put_bytes → records.artifacts → commit_snapshot`
模式，复用适当 artifact kind，必要时增加明确的 `RetrievalText` kind 并测试序列化。
`IndexedDocument.body`、artifact bytes 和恢复后的 `DocumentContent.text` 必须逐字相等。
FTS body、dense 输入与 generated_by/extractor 版本应描述同一 representation。

增加 profile options 的 descriptor materialization 版本。保留旧 snapshot 只读可用；
不要修改历史 artifact 或清空用户 `.cce`。dense reuse 的 profile 边界应使旧错误输入的
向量不能被新构建复用。

**Verify**: `cargo test -p cce-engine --test integration --all-features`
→ 测试 fixture 同符号的 raw 与 summary 文本按设计不同；两者 source address 相同；
重新打开 store 后仍成立；GC 保留被 descriptor 引用的 artifact。

### 3. 修正前置注释边界

`leading_doc_comment` 先把一基声明行换成零基索引，再从声明的上一行开始向上读取。
保留原来的四行上限和属性处理；检查 first-line、连续注释、空行、普通代码隔断、
Rust attributes、块注释和多字节文本。不要把函数体内 Python docstring 自动归为前置注释，
它需要另一个 AST 提取规则，不属于此处 off-by-one 修复。

**Verify**: `cargo test -p cce-engine leading_doc_comment --all-features`
→ 新增针对该 helper 的测试实际运行且全过；声明不是注释，前置注释进入 descriptor。

### 4. 证明 embedding 输入，再做配对实验

在 dense 单元测试使用本地确定性／记录输入的测试 embedder，断言真正传入的字符串是
descriptor，而非仅检查 `representation` 标签。不要依赖模型“必然产生不同向量”的假设。
测试不下载模型。

质量实验分别保存：旧实现、仅正文修复、正文+注释修复。固定查询、源码语料、gold、
模型和预算；只有 materialization profile 是有意改变。关注 vocab-gap、CJK、
`self-dense-profile-guard`、`self-e5-prefix`、`self-adv-lease-acquire`。
现有 dense 路由共享候选索引；不能把只切换 route flags 直接宣称成严格 raw-only/summary-only
实验，除非另证实候选资格在 top-K 前受到约束。本计划首先验收上述输入和修复消融。

**Verify**:

```bash
cargo test -p cce-engine dense --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-daemon-dense-jina-code.yaml . WORKTREE research/output/p015-candidate.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p015-candidate.jsonl --output research/output/p015-smoke-metrics.json
```

上述为 smoke；正式修复消融使用 [固定语料操作附录](kernel-paired-benchmark.md)，
三套实现二进制依次索引同一个 frozen corpus，索引各自独立，不能直接比较变化的源码根。
先重建 CLI/daemon 再串行跑 arm；模型固定 jina-code，
有缓存时离线使用，首次下载行为需披露。另用 `cce-search-sparse.yaml` 保存 sparse 控制臂。
预期测试与门禁退出 0；报告符号／事实指标与 CI，不设凭空的提升百分比。

## Done criteria

- [ ] descriptor 写入、重新打开后的读取、FTS body、embedding 输入逐字一致。
- [ ] 原始 source address/region/provenance 保持指向源码。
- [ ] 前置注释测试通过，profile 更新避免复用错误旧向量。
- [ ] artifact GC 和其他 representations 无回归；dense disabled 仍能索引与查询。
- [ ] 全套门禁、self smoke、dense/sparse 控制与修复消融完成，结果不覆盖旧文件。
- [ ] 修改在 Scope 内，README 状态更新。

## STOP conditions / maintenance

若某 representation 既需要片段读取又需要独立正文且现有字段不能无歧义表达，先提出
body-location 类型／迁移方案，不用 source address 偷渡正文位置。
若新索引仍产生与 raw 相同的 descriptor 文本，先修输入契约，不更换更大的模型掩盖问题。
后续新增 representation 必须补持久化往返和 embedder 输入测试。
