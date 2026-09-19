# Plan 014: 复用已完成快照时原子更新 current

## Status

- Priority: P0；Effort: S；Risk: MED；Category: correctness。
- Status: DONE `4e7a765`；Depends on: 无；评测沿用 012 的计量版本。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19；Confidence: HIGH。
- 输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

仓库先索引 A、再索引 B、恢复 A 后，当前 `index()` 会复用 A 并报告成功，但 current 仍为 B。
fresh search 使用返回的 A，status、atlas 和非 fresh search 使用 B，API 对当前快照的认识分裂。
本计划把“数据已存在”与“快照已激活”拆开，保持单写者和原子发布契约。

## Current state

[engine.rs](../crates/cce-engine/src/engine.rs)，`index()`，约 569、643 行：

```rust
if self.store.snapshot_is_complete(&scanned.snapshot.id)? {
    // repair views / zoekt（已有逻辑）
    return Ok(IndexReport {
        snapshot: scanned.snapshot,
        reused_snapshot: true,
        // ...
    });
}
```

[metadata.rs](../crates/cce-store/src/metadata.rs) 唯一 current 写入在 `commit_snapshot` 事务中：

```sql
INSERT INTO current_snapshots(repository_id, snapshot_id, updated_at)
VALUES (?1, ?2, ?3) ON CONFLICT(repository_id) DO UPDATE SET
snapshot_id=excluded.snapshot_id, updated_at=excluded.updated_at
```

`status()`、`atlas` 的快照解析和 `resolve_index(false)` 读取 current。
测试模式沿用 [integration.rs](../crates/cce-engine/tests/integration.rs) 的
`fixture_repo`、`engine`、`search_request` 与 `index_then_status_then_incremental_reuse`。
索引的不可变数据不应为切换 current 而重写。

## Scope / workflow

- In scope: `crates/cce-store/src/metadata.rs`、`crates/cce-engine/src/engine.rs`、
  `crates/cce-engine/tests/integration.rs`、`research/output/p014-*`、本计划与 README。
- Out of scope: retrieval 排序、watcher、mtime 缓存策略、snapshot retention 策略、公开 API 形状。
- Drift: `git diff --stat d7c2c2a -- crates/cce-store/src/metadata.rs crates/cce-engine/src/engine.rs crates/cce-engine/tests/integration.rs`
  并比对源码摘要；现有未提交内容不能重置。
- 提交风格：`index: reactivate reused snapshots atomically`；不 push 或合并。

## Steps

### 1. 增加受约束的 store 激活操作

增加 `activate_complete_snapshot(repository_id, snapshot_id)`，使用与 `commit_snapshot`
一致的事务／错误转换风格。在同一事务中检查：snapshot 存在、属于该 repository、complete=1；
然后更新 current。无效目标返回错误，current 保持不变。已是 current 时幂等。
方法文档明确调用方必须持有跨进程 IndexLease；store 事务保护数据库一致性，不能代替 lease。

**Verify**: `cargo test -p cce-store --all-features`
→ 新增成功、重复、跨仓库、未完成、缺失目标五类测试；失败路径指针不变。

### 2. 接入复用路径并处理竞态

`index()` 发现完整 snapshot 且 current 不同才申请 lease；同快照健康读路径不增加写锁。
取得 lease 后重新扫描／确认工作树身份，再检查目标与 current，避免旧扫描结果覆盖
另一个 writer 刚提交的快照。若身份改变，丢弃旧决定并用新扫描进入正常流程，
不要在持 lease 时递归调用 `index()`，也不要创建第二把 lease。

将 view repair、Zoekt repair 和 activation 的写路径共享同一 lease 生命周期；只有
成功激活目标，才能返回指向该目标的成功报告。lease 冲突返回既有 `IndexBusy`，
不能静默跳过必需的 activation 然后报成功。可选 provider 修复的降级行为保持原契约。

**Verify**: `cargo test -p cce-engine --test integration --all-features`
→ 已有 repair、增量复用、非 fresh serving 测试通过；新增 writer conflict 用例不会
错误报告已激活。不要使用 sleep 推测时序，用显式持有 lease 的 fixture。

### 3. 覆盖 A→B→A 的公开行为

在同一个临时仓库中建立 A、修改源码为 B 并索引，再精确恢复 A 内容。
关闭不相关外部 providers，dense disabled，防止 fixture 依赖网络或机器安装状态。
第三次 index 应 `reused_snapshot=true` 且 snapshot=A；随后断言 store current、status、
codebase map/atlas、非 fresh search 均使用 A。非 fresh hit 仍应 `verified_current=false`，
不能因为激活成功而把未扫描的后续查询标记为 verified。

**Verify**: `cargo test -p cce-engine --test integration reactivates_prior_snapshot --all-features`
→ 至少一个匹配测试运行并通过；不能接受 `0 tests`。

## Final gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-sparse.yaml . WORKTREE research/output/p014-smoke.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p014-smoke.jsonl --output research/output/p014-smoke-metrics.json
```

预期全部退出 0。先构建两个二进制，再跑 benchmark，不能与 cargo jobs 并发。
正式评测保存冻结源码与二进制摘要；本地 WORKTREE smoke 不构成性能比较。

## Done criteria

- [x] store 拒绝错误仓库／未完成／缺失目标，事务失败不动 current。
- [x] A→B→A 的 index、status、atlas、非 fresh search 指向同一 A。
- [x] 已是 current 的路径不新增 lease 竞争；需切换而 lease busy 时明确失败。
- [x] 修复不改动 source address、freshness 标志和旧快照内容。
- [x] Rust 门禁与 smoke 完成，修改范围正确，README 状态更新。

## STOP conditions / maintenance

若需修改 GC 保留策略或跨仓库 activation，超出本计划，先报告。
若运行中工作树持续变化，沿用现有明确错误机制，不能无限重试或承诺 verified。
此修复不解决“同 size + 同 mtime 的改动漏检”；后者是扫描验证边界的独立问题。
将来增加 snapshot checkout/restore API 时复用同一个 activation 操作。

## Implementation notes (2026-09-19)

- `MetadataStore::activate_complete_snapshot`：单事务校验 exists + repository + complete=1
  后 upsert current；missing/foreign/incomplete 报错且不动 current；幂等。
- `index()` 激活门：scan→complete 且 ≠current 才取 lease；lease 内重扫并复查
  current/target；rescan 落到未完成快照时携同一 lease 走正常写路径（不重入、不递归）。
- 视图修复与 Zoekt 修复复用同一 lease（`lease: Option` 贯穿）；无需激活时不取写锁；
  busy → IndexBusy 显式失败，不跳过激活报成功。
- 测试：store 五类（成功/幂等/跨仓库/未完成/缺失）+ integration
  `reactivates_prior_snapshot`（A→B→A 后 current/status/atlas/非 fresh search 均 A，
  非 fresh hit 仍 unverified）与 `activation_conflict_reports_busy_not_success`
  （持锁 → IndexBusy + current 不变）。
- smoke：research/output/p014-smoke.jsonl + p014-smoke-metrics.json
  （recall@20 .7321、no_context_precision 1.0，与 p013 持平）。
