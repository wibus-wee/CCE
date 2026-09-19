# Plan 019: `pat:` 结构模式查询算子（两级漏斗 + tree-sitter CST 匹配）

## Status

- Priority: P1；Effort: M；Risk: MED；Category: feature / retrieval。
- Status: TODO；Depends on: 无硬依赖；与 015/018 共享 `retrieval.rs`/文档读取路径，
  执行前须做 drift 核对。对标 Sourcegraph-alignment F2（差距 100%）。
- Planned at: `7aadd4e` + 未提交工作区，2026-09-19。
- Confidence: 管线架构 HIGH（Sourcegraph 已验证的两级漏斗）；引擎选型
  MED——依赖 step 0 的版本兼容 spike。
- 输入摘要：[sourcegraph-alignment-gap-analysis.md](../docs/sourcegraph-alignment-gap-analysis.md)
  F2/F3；Sourcegraph 参考实现 `cmd/searcher/internal/search/{search_structural,zoekt_search}.go`。

## Why this matters

structural search 是差距分析中投入产出比最高的缺失项：完全确定性、无 ML 依赖、
agent 高频需求（找调用形状 `foo(:[x], ..)`、错误处理模式 `if (:[c]) { ... }`）。
当前查询表达力只有 `lang:`/`path:`/`type:` 三个算子。

**不采用 comby**：Sourcegraph 的实现是 OCaml 二进制子进程（fork 上限 4、tar/stdin
IPC、二进制分发），5.3 起官方默认禁用并标注 "performance limitations, not actively
developed"。CCE 全 Rust、已 vendor 8 个 tree-sitter grammar，进程内 CST 匹配
直接消掉子进程开销，且真 CST 比 comby 的近似 concrete-syntax 更精确。

## Current state

- [retrieval.rs](../crates/cce-core/src/retrieval.rs) `parse_query_filters`（L217）：
  按 whitespace 切 token，只认 `lang:`/`path:`/`type:{diff,commit,file}`；
  未知 `key:` token 原样留在查询文本——**此契约必须保留**。
  `QueryFilters` 只有 `path_prefix`/`language`/`hit_type` 三个字段。
- [retrieval.rs](../crates/cce-engine/src/retrieval.rs) L240 消费 filters，
  L263 `type:` dispatch 是算子改路由的既有模式（`diff`→Diff route）。
- `SearchRoute::Structural` **已被图扩张占用**（`cce-core/src/retrieval.rs` L69），
  新算子必须另起 route 名，不能复用。
- [parser.rs](../crates/cce-engine/src/parser.rs) `language()`（L282）：
  rust/typescript/tsx/javascript/python/go/java/csharp 8 个 grammar 的单一映射表，
  `SourceParser::parse` 已产出符号区间；匹配器复用此表。
- 候选供给：SQLite FTS5（lexical route）+ zoekt provider
  [zoekt.rs](../crates/cce-engine/src/zoekt.rs)（外部 sidecar，非必需）+
  snapshot 源 artifact（content-addressed，本地必达）。
- Sourcegraph 管线（已读源码核实）：`StructuralPatToRegexpQuery` 把模板降阶成
  过近似 regex → zoekt 三元组索引/zip regex 预筛候选文件 → comby 只在候选上精确匹配
  → NDJSON 逐行回流。要抄的是这个漏斗，不是引擎形态。

## Scope / workflow

- In scope: `crates/cce-core/src/{retrieval,text}.rs`、`crates/cce-engine/src/`
  （新增 `pattern.rs`，改 `retrieval.rs`、`parser.rs`、`Cargo.toml`）、
  `crates/cce-engine/tests/integration.rs`、benchmarks 数据集按需、本计划与 README。
- Out of scope: `type:symbol` 符号检索、comby `rule:` 约束语言、改写（rewrite）
  能力、未知语言的 `.generic` 兜底、多语言同一查询、web UI。
- Drift: `git diff --stat 7aadd4e -- crates/cce-core/src crates/cce-engine/src`，
  若 015/018 已落地须复读其正文读取契约；`parse_query_filters` 的未知-token 保留
  语义不得变。
- 提交风格 `retrieval: add pat: structural pattern operator`、`pattern: tree-sitter
  structural matcher`；不 push/合并。

## Steps

### 0. 引擎 spike：ast-grep-core 版本对齐与 Language 适配

ast-grep-core 把 `tree_sitter::Language` 重导出为 `TSLanguage`，其 `Language` trait
只要三个方法（`kind_to_id`/`field_to_id`/`build_pattern`），`LanguageExt` 只要
`get_ts_language()`——`ast-grep-language` 的 `impl_lang!` 宏就是参照（约 15 行/语言）。

选一个 tree-sitter 依赖与 workspace `tree-sitter = "0.26"` 兼容的 ast-grep-core 版本
（版本错位会产生两个不同的 `tree_sitter::Language` 类型，无法适配——这是本计划
唯一的硬阻塞点）。为 `language()` 表写 `CceLang` 薄 wrapper；fixture：`fn $F($$$ARGS)`
匹配 Rust 函数、`fmt.Println($$$X)` 匹配 Go 调用。

若无可对齐版本：升级 workspace tree-sitter（连带 grammar  crates）优先；
仍不可行则降级为直接用 `tree_sitter::Query` S-expression 表面（无 ast-grep 依赖，
但模板语法变成 `(call_expression function:(identifier) @n)`，放弃 `:[hole]` 兼容）。

**Verify**: `cargo test -p cce-engine pattern --all-features` 中 spike fixture 通过；
`cargo tree -p cce-engine | rg tree-sitter` 确认全 workspace 只有一份 tree-sitter。

### 1. 查询表面：`pat:'<template>'` 与引号 tokenize

模板含空格/括号，`parse_query_filters` 的 whitespace 切分必须先升级为支持
`key:'quoted value'` / `key:"quoted value"`；`pat:` 值进 `QueryFilters.pattern`
（新字段，`serde camelCase`、`Option<String>`、`skip_serializing_if` 与现有字段一致）。
`patterntype:structural` 作为可选别名 token（值为 `structural` 时把剩余裸文本
整体视为模板）。`lang:`/`path:` 照旧AND收窄。未知 token、裸冒号、空值的
原样保留契约不变，补测试覆盖引号、模板内冒号（`pat:foo(:[x:1])`）、未闭合引号。

**Verify**: `cargo test -p cce-core --all-features` → 新旧 tokenize 用例全过；
`a:b lang: path:` 用例（text.rs L103）不回归。

### 2. 模板编译器：语法翻译 + 过近似 regex 预筛

`pattern.rs` 两个纯函数：

- `compile_template(template, lang) -> Pattern`：`:[name]`→`$NAME`、`...`→`$$$`，
  `$X` 直通；交给 ast-grep `Pattern::new` 编译（失败要报"模板非法"，不静默降级）。
- `literal_anchors(template) -> RegexPrefilter`：模板按洞切分，字面片段转义后按序
  懒惰通配连接（comby `StructuralPatToRegexpQuery` 同型）；**超集性质必须性质测试**：
  凡 ast-grep 命中的文本必命中该 regex。洞无任何字面锚点（`:[x]` 单洞模板）时
  prefilter 退化为全文件枚举，如实报告为慢路径而非拒绝。

候选供给顺序：`lang:`/`path:` 先收窄文件集 → prefilter regex 在
snapshot 源 artifact 上扫候选（zoekt provider 在时优先加速，但**离线必须不依赖它**）。

**Verify**: `cargo test -p cce-engine pattern --all-features` → 锚点提取、转义、
无锚点退化、超集性质（对每个 matcher fixture 断言 regex 命中其全部命中区间所在文件）。

### 3. 检索路径：新 route + 证据契约

- `SearchRoute` 新增 `Pattern` variant（snake_case `pattern`）；**不得复用**
  `Structural`（图扩张）。`pat:` 在 retrieval.rs `type:` dispatch 同处把
  `plan.routes` pin 为 `[Pattern]`，理由进 `plan.reasons`。
- Pattern route：候选文件 → `SourceParser` 同 grammar 解析 → ast-grep 匹配 →
  命中字节区间映射 canonical `SourceAddress`/`Region`（复用文档 addressing），
  产出 `SearchHit{route: Pattern, representation: RawCode}`，`generated_by`
  记 `cce-pattern-v1` 之类的确定性派生标记——确定性事实与模型推断保持可分。
- 语言不支持（不在 `language()` 表）→ `missing_capabilities` 明示
  "structural matching unavailable for `<lang>`"，**不静默回退 regex**；
  无 `lang:` 时按候选文件扩展名逐文件选 grammar（Sourcegraph 同款语义）。
- 与既有证据门协作：pattern 命中计为确定性锚点证据，参与 verdict/witness 计算。

**Verify**: `cargo test -p cce-engine --all-features` → 集成测试覆盖：
命中映射到正确 region/address、不支持语言出 missing_capability、
`pat:` + `path:`/`lang:`/自由文本 AND 语义、确定性 replay 结果一致。

### 4. 门禁与 smoke

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p cce-cli -p cce-daemon
uv run --project research cce-research run benchmarks/datasets/cce-self.jsonl benchmarks/adapters/cce-search-daemon-dense-jina-code.yaml . WORKTREE research/output/p019-candidate.jsonl
uv run --project research cce-research evaluate benchmarks/datasets/cce-self.jsonl research/output/p019-candidate.jsonl --output research/output/p019-smoke-metrics.json
```

structural 命中应能提升 symbol/code 类 query 的 recall 或至少不回退；
新增 pat 专用 fixture（如 `pat:'$S.lock().unwrap()'`）进 smoke 数据集记录命中正确性。

## Done criteria

- [ ] `pat:'<template>'`（含 `:[x]`/`...`/`$X` 三种元变量写法）在 8 个支持语言上
  正确命中，结果带 canonical address/route=pattern/deterministic provenance。
- [ ] prefilter regex 超集性质经测试；无锚点模板如实降级为全枚举并标注。
- [ ] 不支持语言/非法模板/未闭合引号全部显式报错或 missing_capability，零静默降级。
- [ ] 离线路径不依赖 zoekt 与网络；引擎缺失（若保留可插拔）走 Unavailable 契约。
- [ ] 未知 `key:` token 原样保留等 `parse_query_filters` 既有契约不回归。
- [ ] 全套 Rust 门禁 + self-index smoke 完成；README 状态更新。

## STOP conditions / maintenance

- 若 step 0 证明 ast-grep-core 与 tree-sitter 0.26 无法对齐且升级牵连过大，
  退回 tree-sitter `Query` 原生方案并记录，不引入第二份 tree-sitter。
- 若 `pat:` 与自由文本的 AND 语义与 planner 冲突（planner 对空 query 的
  NoRetrieval 判定等），pattern 优先短路 planner，不把模板文本喂给 FTS。
- 不做 rewrite/`rule:`/多语言同查询；这些是独立计划。
- 若发现 prefilter 在大仓库成瓶颈（无锚点模板全枚举），记录为后续
  region 级漏斗（用已解析的 ParsedUnit 区间做第二级收窄）候选优化，不在本计划实现。
