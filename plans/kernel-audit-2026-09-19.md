# CCE 内核问题记录与执行依据

审查时间：2026-09-19。代码基线：`d7c2c2a` 加已有未提交修改。
[输入文件摘要](kernel-audit-2026-09-19.json) 记录实际读取的工作区内容；HEAD 本身不能代表本轮代码。
本记录只读审查了 Rust core/store/engine 与研究评测，没有改生产代码、重跑引擎矩阵或执行完整门禁。
UI、gateway 安全、依赖漏洞和部署没有全面审查。

规划过程中另有工作更新了 `retrieval.rs` 和 `metadata.rs`。最终复核时本文列出的 dense
全量读取、正文恢复、packing 与图身份问题仍存在；摘要保留最初审查时点，不冒充最新源码。
执行者须对实时 diff 重核，不能重置这些并行修改来套用计划。

## 问题清单

| ID | 问题与可观察后果 | 代码证据 | 处理 |
| --- | --- | --- | --- |
| E1 | 一个 item 的多个地址重复计 token，并占用多个排名位置 | `adapters.py::normalize_context_pack`；`metrics.py::case_observations` | [012](012-evaluation-evidence-accounting.md) |
| E2 | 错文件同名 symbol 可满足 fact；未标注 symbol 默认满分 | `metrics.py::claim_support/symbol_recall` | [012](012-evaluation-evidence-accounting.md) |
| G1 | SCIP local symbol 跨 Document 冲突，伪边被标为 confidence=1 | `scip.rs::ingest` 全局 definitions 键 | [013](013-graph-identity-and-call-completeness.md) |
| G2 | 同文件第二个 caller 调用同一 callee 的边丢失 | `relations.rs::call_relations` 的 callee-only emitted | [013](013-graph-identity-and-call-completeness.md) |
| S1 | A→B→A 复用后 current 仍为 B，查询入口使用不同快照 | `engine.rs::index` 复用分支；`metadata.rs::commit_snapshot` | [014](014-reactivate-complete-snapshot.md) |
| R1 | dense 的 FileDescriptor/SymbolSummary 恢复成原始源码 | `engine.rs` descriptor 写入；`metadata.rs::documents_for_snapshot` | [015](015-persist-retrieval-representations.md) |
| R2 | 前置注释读取从声明行开始，漏掉普通声明前的注释 | `parser.rs` 一基 start_line；`engine.rs::leading_doc_comment` | [015](015-persist-retrieval-representations.md) |
| P1 | 放不下的候选仍占去重和文件配额，阻止后续短证据 | `context.rs::render_item` 先 insert 后检查 budget | [016](016-commit-packing-state-on-admission.md) |
| P2 | SearchVerdict 未进入 context，预算裁剪后的证据缺口不可见 | `context.rs::pack`；`cce-core::ContextPack` | [017](017-preserve-verdict-through-context.md) |
| P3 | context latency 只回填 search 耗时，漏掉 backlinks/packing | `context.rs::CceEngine::context` | [017](017-preserve-verdict-through-context.md) |
| Q1 | dense 每次全量解码索引、全库正文物化和逐命中 entity 查询 | `retrieval.rs` dense 分支；`metadata.rs::source_text` | [018](018-dense-query-materialization.md) |

上述逻辑问题置信度高，来自代码路径核对；实际产品影响大小仍需要各计划中的回归测试与实验。
对性能项只确认重复工作模式，尚未测得其总延迟占比。各计划单独列出投入、风险、范围与停止条件。

## 评测观察与分母

使用当前研究评测器重新计算已有原始结果，而非直接引用历史 metrics JSON。
所有自测均按 `case_clusters` 折叠派生预算/变体族，再对族取均值。
v12 与当前自测数据 SHA256 一致：
`3ad10097501e46c89734dc56e8a812b63ddb6ea8f557572fca026443f923f37c`。

| 结果 | 样本 | 观察 |
| --- | --- | --- |
| [self-v12-search](../benchmarks/results/self-v12-search.jsonl) | 51 行，41 族 | 全体 Recall@20=.7364；file_success@20=.6585 |
| 同一 v12 的符号标注子集 | 26 族 | symbol_recall@20=.4647 |
| 同一 v12 的正例事实子集 | 13 族 | claim_support=.7308，仍采用旧宽松判据 |
| 同一 v12 的拒答 | 15 个负例，36 个正例行 | 7 个负例有返回项；0 个正例空结果 |
| 同一 v12 时延 | 51 个原始请求，不按族折叠 | median=1.536s；p95=2.956s |

负例有候选不直接等于引擎断言有答案：旧结果归一化没有保留 verdict，不能区分 WeakWitness。
fact 支持也是引用/符号代理，不等于回答正确率。这里不作显著提升、统计等价或优于外部系统的结论。

六个外部仓库使用各自 `sweb-lite-<repo>-sparse-v2.jsonl` 保存的结果，数据摘要与当前
对应数据集一致，共 253 行。不是与 self-v12 同一二进制的横向胜负比较。

| 仓库 | 行数 | 有标注 symbol recall@20 | 正例 claim support | median / p95 秒 |
| --- | ---: | ---: | ---: | ---: |
| pytest | 16 | .267 | .467 | 3.13 / 4.94 |
| scikit-learn | 10 | .333 | .650 | 4.48 / 5.77 |
| sphinx | 16 | .067 | .467 | 4.74 / 7.03 |
| matplotlib | 22 | .148 | .472 | 7.97 / 10.48 |
| sympy | 75 | .204 | .472 | 22.84 / 32.14 |
| django | 114 | .249 | .582 | 23.74 / 32.61 |

p95 使用排序后的 `floor(.95*n)` 零基位置，截到末元素；复算时保持这一方法，或明确声明更换。
原始结果位于 gitignored `research/output/`，本地存在不保证其他 checkout 可取得。
外部 builder 使用固定 checkout 与 patch-derived gold，存在 issue base revision 偏差；
符号/事实也不是完整人工裁决集合，不能把这些数值外推为全体真实任务的能力。

旧 [完整评测报告](../research/output/full-eval-20260918/REPORT.md) 的数百秒长查询已有
[pair-floor 后续修复记录](../research/sweb-pytest-arm-report.md)。它有历史诊断价值，
不能代表当前执行路径。最新源码又有工作区改动，已有结果也不等同于当前源码重新测试通过。

## 只读复核

- 100-token item、两个来源地址，在当前 normalizer 中得到两个各 100-token 的 range。
- 有 evidence=`a.rs`、symbol=`pack` 的 fact，被 `unrelated.rs::pack` 得到 claim_support=1。
- 当前 `.cce-benchmark` 的一个 current snapshot 中，1,850 对同实体 RawCode/SymbolSummary
  共用相同 source digest 和 address。结合 materialization 代码可确定两者恢复出相同正文；
  未直接检验所有向量是否逐位相等，也未把该 snapshot 冒充 v12 的 snapshot。

这三项是在内存／SQLite 只读模式下完成的诊断，尚未替代各计划要求的自动化回归测试。

## 执行顺序与实验边界

1. 012 固定计量，再重新采集可比较基线。013/014 的精确正确性测试可以先编写。
2. 013 修图身份与漏边，014 修当前快照激活；二者正确性不依赖检索得分是否上升。
3. 015 修正文与注释，分别测试输入契约和质量消融。
4. 016 修 packing admission，017 传递 search verdict 与实际交付缺口。
5. 018 在 015 的正文契约上减少 I/O，要求质量输出保持一致。

013/015 共享 profile 配置，014/015/018 共享 engine/store，016/017 共享 packer；
即使任务逻辑独立，也不要在同一工作区让多个 executor 同时覆盖这些文件。
每份计划包含完整命令，benchmark 前必须一起重建 cce-cli 和 cce-daemon。

正式 A/B 必须保持**被检索语料**固定。CCE 自索引尤其要注意：修改计划、代码或测试
本身就改变 corpus；应使用冻结的仓库副本与相同 gold，只更换构建后的 engine 二进制。
记录二进制、adapter、源码语料、数据集、模型和 profile 摘要，索引产物按实现版本隔离。
复用产物可以用于 smoke，不能掩盖 materialization 版本变化。

## 需要后续专项验证的边界

| 问题 | 当前证据与下一步 | 本轮处置 |
| --- | --- | --- |
| SCIP 与源码版本未绑定 | provider 导入按路径/坐标匹配；需构造旧 index.scip + 新同路径源码，验证降级/拒绝，再设计内容摘要清单 | 单独设计，013 不宣称已解决 |
| size+mtime 缓存的验证强度 | scanner 使用毫秒级 mtime；同长度且保留 mtime 的编辑可能复用旧 hash；需 racy-write fixture | 单独验证 strict freshness 策略，014 不扩大范围 |
| qualified symbol witness | `Type::method` 的各段存在不自动证明所属关系；需同名成员跨类型 fixture | 先修图身份，再做限定符号契约 |
| 外部 sparse 大仓库延迟 | Django/SymPy 的约 23 秒不是 dense 路径测量 | 对 FTS/flow/witness/index 阶段计时，再立性能方案 |
| 外部 gold 与原 issue revision | builder 复用固定 checkout；patch touched-files 与完整相关证据不同 | 后续冻结 base revision、裁决与 held-out 评测 |

## 已考虑但本轮不采用

- 直接打开 `if false` 下的 corroborated-path gate：它可能误拒绝真实词汇鸿沟查询，
  需单独实验；不能把当前实验开关当一个无风险修复。
- 用 RRF 分数阈值判断答案存在：分数跨查询不可比较。
- 先换更大 embedding、默认开启 reranker：先修输入与证据计量，当前结果不足以证明收益。
- 先引入 ANN：当前已确定有全量正文 I/O；修完并 profile 后再判断 dot product 占比。
- 把 Calls/References 可达性称为精确 source→sink taint：缺值流／参数／返回／sanitizer
  语义，不能因此把 PreciseDataflow 标 Ready。旧 009 对此必须重写。
- 重新实现 SCIP importer、SearchVerdict 或 federation：代码已存在相关能力；
  旧计划的 TODO 状态不能作为未实现证据。
