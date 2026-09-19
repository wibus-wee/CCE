# Plan 012: 分开计量检索项、来源地址和证据覆盖

## Status

- Priority: P0；Effort: M；Risk: MED；Category: tests / correctness。
- Status: TODO；Depends on: 无。013–018 可以先写回归测试，但质量比较应使用本计划的新计量。
- Planned at: `d7c2c2a` + 未提交工作区，2026-09-19。
- 审查输入摘要：[kernel-audit-2026-09-19.json](kernel-audit-2026-09-19.json)。

## Why this matters

当前适配器把一个检索项展开成多个地址，丢失 item 身份，并对每个地址重复计算完整 token。
另一方面，事实支持只比较裸符号名，错误文件中的同名函数也能算作支持。
这两处问题使 packing 优化和事实召回的比较失真。本计划只修评测契约，不修改 Rust 排序。

## Current state

- [adapters.py](../research/cce_research/adapters.py)，`normalize_context_pack`，约 529 行：

  ```python
  for address in addresses:
      output.append(RetrievedRange(
          # 省略地址字段
          estimated_tokens=int(item.get("estimatedTokens", 0)),
      ))
  ```

- [metrics.py](../research/cce_research/metrics.py)，`claim_support`，约 450 行：

  ```python
  found_symbols = {candidate.symbol for candidate in retrieved if candidate.symbol}
  # 任意 evidence overlap 或任意裸 symbol 命中就满足一个 fact。
  ```

- `CaseResult` 只有扁平 `retrieved`；`RetrievedRange` 没有 item、snapshot、region 身份。
- `_result` 使用 `abstained=not retrieved`，没有保留引擎 verdict。
- `symbol_recall` 在未标注时返回 1；当前全局均值不代表有标注子集能力。
- 测试沿用 [test_metrics.py](../research/tests/test_metrics.py) 的 `_case`、`_result`，
  以及 `test_normalize_payload_handles_context_packs`；schema 使用 Pydantic `extra="forbid"`。

内存复核：100-token item + 两个地址变成 200 tokens；`unrelated.rs::pack` 可满足
本应由 `a.rs` 支撑、附带 symbol=`pack` 的 fact。两个例子应成为回归测试。

## Scope

In scope:

- `research/cce_research/{schema,adapters,metrics,cli}.py`
- `research/tests/{test_adapters,test_daemon_adapters,test_metrics,test_cli}.py`
- `benchmarks/datasets/cce-self.jsonl`：只允许经过源代码核实的限定符号标注与 revision 更新。
- `docs/benchmarking.md`：更新指标定义的现有章节。
- `research/output/p012-*`：新结果、原始响应和复算报告。
- 本计划与 `plans/README.md` 状态。

Out of scope: Rust 实现、排序参数、悄悄修改 gold 以提高成绩、覆盖已有结果文件。
Python 仅限 `research/`；原始响应可能很大，只存研究产物目录，不塞进 SQLite。

## Git workflow / drift check

执行 `git diff --stat d7c2c2a -- research benchmarks/datasets/cce-self.jsonl`，
结合输入摘要和上述 excerpts 检查漂移；现有未提交改动是审查基线，不能重置。
在独立分支或经确认的现有工作分支实施，提交风格 `research: preserve result item identity`；
本计划不授权 push、合并或修改其他人的工作。

## Steps

### 1. 建立双层结果模型

在 schema 中增加可选的 item 层：稳定 item/document ID、原始 rank、token cost、主地址、
支持地址，以及可用的 repository/snapshot/region 身份。`CaseResult` 增加结果类型
（search/context/legacy）、可选 `used_tokens`、可选 `verdict_state` 和计量版本。
旧 `retrieved` 保留兼容读取。新适配器同时填充两层，但两层只能由一个 normalization
函数派生，不能各写一套路径转换。

新结果按原始 item/hit 顺序定义 @K；地址是该项的属性，不再占用独立排名槽位。
search 的多个表示如果产品已合并为一个 hit，评测也只计一个 hit。

**Verify**: `uv run --project research pytest research/tests/test_adapters.py research/tests/test_metrics.py -q`
→ 新旧 JSON 都能读取；两个来源地址不改变 item 数或原始 rank。

### 2. 修正预算和 abstention 计量

context 预算使用顶层 `usedTokens`，同时核对全部 items 的 token 总和；没有地址的
orientation 也计入。不要再次按地址累计或用重复 token 重新截断一个已经装好的 pack。
search 没有 packing 预算，跳过该指标。

旧结果缺 item 身份或顶层 token 时，将预算指标标记不可计算；不能用重复地址猜测恢复。
保存新 run 的原始响应，供后续重算。历史扁平文件继续允许计算明确标成 legacy 的指标。
保留“无返回证据”的统计，同时单独记录 `answered/weak_witness/abstained`；弱见证有
候选与系统断言有答案是不同事件，不能互相替换。

**Verify**: 同上命令 → 100-token 双地址样例总量为 100；无地址 item 计量；旧文件不会
伪造预算结论；`weak_witness` 能保留下来但不被改写成空结果。

### 3. 限定符号证据的身份

增加 scoped symbol 标注（至少 path + symbol，可选 canonical region/qualified name）。
fact 存在 evidence 路径时，symbol 命中必须同时落在该路径集合；不允许同名符号绕过路径。
支持既有行范围 overlap，但明确它仍是来源覆盖代理，不能声称完整逻辑事实已被证明。
只有裸 symbol 的旧 gold 保留 legacy 指标；新 strict 指标在身份不明时跳过并报告覆盖数。
不要把多个 gold 文件与裸符号做笛卡尔积来“生成”限定标注。

有符号标注的子集单独报告 `symbol_recall@20` 的有效样本数；无标注为 skipped。
正例 fact 与能力拒答负例分开汇总，派生样本继续按根问题族折叠。

**Verify**: `uv run --project research pytest research/tests/test_metrics.py -q`
→ 错误文件同名 symbol 得 0；正确限定符号得 1；缺标注不抬高均值；已有 cluster 测试通过。

### 4. 切换评测版本并建立新基线

manifest 记录 metric/schema 版本；`compare` 不接受两侧混用 item/legacy 排名语义。
对旧产物只做有明确定义的重算，不能宣称恢复了已丢失的 item、snapshot、verdict。
使用同一冻结语料、gold、预算和模型重新采集 baseline/candidate。原始结果不得覆盖。
把本次 metric definition 变化与引擎质量变化分开出表。
后续引擎 A/B 按 [固定语料操作附录](kernel-paired-benchmark.md) 执行，包含未提交内容的
冻结方法、两套绝对 binary 路径和独立索引目录；下文 `. WORKTREE` 命令只作 smoke。

**Verify**:

```bash
uv run --project research pytest research/tests -q
uv run --project research cce-research validate-dataset benchmarks/datasets/cce-self.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl benchmarks/results/self-v12-search.jsonl --output research/output/p012-v12-legacy.json
```

→ 测试通过；数据集合法；历史结果被明确标为 legacy，预算未知不填成满分。

## Commands and final gates

后续新基线的 smoke 命令如下；先完成构建，再跑评测，禁止 cargo 与 provider 基准并行。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-sparse.yaml . WORKTREE research/output/p012-sparse.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p012-sparse.jsonl --output research/output/p012-sparse-metrics.json
```

所有命令预期退出 0；若基线已有不相关失败，保留日志并报告，不能顺手扩大修改范围。
`WORKTREE` 仅用于本地 smoke；正式比较必须冻结源码、记录二进制/源码/数据/adapter 摘要。
dense 对照使用 `cce-search-daemon-dense-jina-code.yaml`；本地模型首次下载需披露，
不能让无模型环境的离线 sparse 验证依赖网络。

## Done criteria

- [ ] 新结果中 item 数、排名、token、地址分别保留，归一化回归测试通过。
- [ ] 错路径同名 symbol 不再得到 strict fact 支持。
- [ ] 旧结果继续可读，无法恢复的信息明确缺失；不混用新旧计量比较。
- [ ] 报告记录有效标注样本数、根问题族、verdict 与空结果分别统计。
- [ ] 完整研究测试和上述质量门禁通过；有新 smoke 产物。既有门禁失败要记录并阻止 DONE。
- [ ] `git diff --name-only` 中本轮修改均在 Scope 内；README 状态更新。

## STOP conditions / maintenance

若没有足够信息恢复原始 item，停止恢复该指标并重新采集，不猜 rank 或 token。
若 raw response 声称的地址覆盖范围大于实际 snippet，不能据此升级成“正文已经交付”证明；
交付覆盖由 017 处理。若要修订 gold 的语义相关性，另做裁决轮，不能混入计量 PR。
每次增加指标都要注明单位（item/file/region）、样本子集与缺失规则。
