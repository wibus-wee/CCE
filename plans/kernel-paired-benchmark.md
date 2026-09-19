# 固定语料的内核配对评测操作

此文档是 013、015、016、017、018 的正式 A/B 操作附录。计划里的 `. WORKTREE`
命令只用于当前实现的 self-index smoke，不能直接用作修改前后的严格配对。
执行本附录时，两侧使用同一个冻结仓库路径、同一数据集与研究评测器，只切换引擎二进制。
012 的计量变更应先完成；其自身使用原始响应 fixture 验证计量，不把计量变化称为质量增益。

## 前置条件

- 从仓库根运行；`uv`、Rust 工具链及 research 环境可用。
- 暂停修改被评测输入，完成本轮前置计划；不要求清空或提交当前工作区。
- 选择未使用过的输出目录。下面用 `p013-paired`，其他计划替换这个唯一名字。
- baseline 在实现目标修复之前构建；candidate 在之后构建。两次都重建 CLI 与 daemon。
- 模型固定；首次下载需披露。baseline/candidate 的模型来源和版本必须一致。
- 不同时运行 cargo jobs、provider 索引和 benchmark arms。

## 1. 冻结当前语料，包括未提交文件与删除

```bash
export CCE_PAIR_ROOT="$PWD/research/output/p013-paired"
uv run --project research python - <<'PY'
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

root = Path.cwd().resolve()
out = Path(os.environ["CCE_PAIR_ROOT"]).resolve()
if out.exists():
    raise SystemExit("Choose a new output directory; existing results must not be overwritten")
out.mkdir(parents=True)
corpus = out / "corpus"
def input_names():
    live = subprocess.check_output([
        "git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"
    ]).decode().split("\0")
    head = subprocess.check_output([
        "git", "ls-tree", "-r", "--name-only", "-z", "HEAD"
    ]).decode().split("\0")
    return sorted(set(name for name in live + head if name))

names = input_names()

def digest(path):
    if path.is_symlink():
        raise SystemExit(f"Review symlink handling before freezing: {path}")
    if not path.exists():
        return None
    if not path.is_file():
        raise SystemExit(f"Unsupported input (for example submodule): {path}")
    return hashlib.sha256(path.read_bytes()).hexdigest()

before = {name: digest(root / name) for name in names}
subprocess.run([
    "git", "clone", "--no-hardlinks", "--no-checkout", "--", str(root), str(corpus)
], check=True)
subprocess.run(["git", "-C", str(corpus), "checkout", "--detach", "HEAD"], check=True)
for name, expected in before.items():
    src, dst = root / name, corpus / name
    if expected is None:
        if dst.is_file() or dst.is_symlink():
            dst.unlink()
    else:
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
after = {name: digest(root / name) for name in names}
copied = {name: digest(corpus / name) for name in names}
if before != after or before != copied or names != input_names():
    raise SystemExit("Source changed while freezing; keep diagnostics and retry in a new directory")
shutil.copy2(root / "benchmarks/datasets/cce-self.jsonl", out / "dataset.jsonl")
manifest = {
    "head": subprocess.check_output(["git", "rev-parse", "HEAD"]).decode().strip(),
    "corpus_root": str(corpus),
    "files": before,
    "dataset_sha256": digest(out / "dataset.jsonl"),
    "includes_uncommitted_files": True,
}
(out / "corpus-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print("Frozen corpus manifest written; no remote repository was fetched")
PY
```

该 local clone 保留 Git 历史，overlay 保留当前未提交内容与删除；gitignored 模型、索引和
外部克隆不进入语料。出现 symlink/submodule 时明确停止，先定义其内容边界，不跟随到仓库外。
需要额外 provider artifact 时另记录其摘要及源版本，不能无证明地复制旧 index.scip。

## 2. 保存 before 的两个二进制与 adapter

```bash
cargo build --release -p cce-cli -p cce-daemon
mkdir -p "$CCE_PAIR_ROOT/before-bin"
cp target/release/cce "$CCE_PAIR_ROOT/before-bin/cce"
cp target/release/cce-daemon "$CCE_PAIR_ROOT/before-bin/cce-daemon"
```

在实现修复前执行下面的 adapter 生成器。`arm="before"`，search dense 使用所示模板；
sparse 换为 `cce-search-sparse.yaml`，context 换为 `cce.yaml`。两侧模板必须同一个。
为并发成本可比，生成器统一为 daemon session；route-pinned search 的 subprocess fallback
会被 harness 标记，时延统计应分开。

```bash
uv run --project research python - <<'PY'
import hashlib
import json
import os
from pathlib import Path
import subprocess
import yaml

arm = "before"  # candidate 阶段只改成 after
out = Path(os.environ["CCE_PAIR_ROOT"]).resolve()
template = Path("benchmarks/adapters/cce-search-daemon-dense-jina-code.yaml")
adapter = yaml.safe_load(template.read_text())
adapter["name"] = "kernel-paired-" + arm
adapter["session"] = "daemon"
adapter["command"][0] = str(out / (arm + "-bin") / "cce")
adapter.setdefault("environment", {})["CCE_DATA_DIR"] = str(out / (arm + "-index"))
adapter.setdefault("dense", "local")
(out / (arm + ".yaml")).write_text(yaml.safe_dump(adapter, sort_keys=False))
paths = [out / (arm + "-bin") / n for n in ["cce", "cce-daemon"]]
paths += [Path("Cargo.lock"), Path("research/uv.lock"), out / (arm + ".yaml")]
manifest = {
    "arm": arm,
    "head": subprocess.check_output(["git", "rev-parse", "HEAD"]).decode().strip(),
    "tracked_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "HEAD"])).hexdigest(),
    "sha256": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},
    "template": str(template),
}
(out / (arm + "-build.json")).write_text(json.dumps(manifest, indent=2) + "\n")
PY
uv run --project research cce-research run "$CCE_PAIR_ROOT/dataset.jsonl" "$CCE_PAIR_ROOT/before.yaml" "$CCE_PAIR_ROOT/corpus" before "$CCE_PAIR_ROOT/before.jsonl"
```

索引 data dir 在 frozen corpus 外，各 arm 单独构建，防止同 profile 意外共享旧向量或旧图。
二进制摘要才是可执行内容的身份；HEAD/diff 只是辅助归因，不能把未追踪源码排除在记录之外。
正式发布另保存完整构建输入摘要；例如沿用本轮 `kernel-audit-2026-09-19.json` 的逐文件模式。

## 3. 保存 after 并在同一语料运行

修复和测试完成后：

```bash
cargo build --release -p cce-cli -p cce-daemon
mkdir -p "$CCE_PAIR_ROOT/after-bin"
cp target/release/cce "$CCE_PAIR_ROOT/after-bin/cce"
cp target/release/cce-daemon "$CCE_PAIR_ROOT/after-bin/cce-daemon"
```

重跑第 2 步的 adapter 生成器，只把 `arm` 改成 `after`；模板、模型、预算、环境开关保持相同。
不得重建或覆盖 corpus/dataset。随后：

```bash
uv run --project research cce-research run "$CCE_PAIR_ROOT/dataset.jsonl" "$CCE_PAIR_ROOT/after.yaml" "$CCE_PAIR_ROOT/corpus" after "$CCE_PAIR_ROOT/after.jsonl"
uv run --project research cce-research compare "$CCE_PAIR_ROOT/dataset.jsonl" "$CCE_PAIR_ROOT/before.jsonl" "$CCE_PAIR_ROOT/after.jsonl" --gate --output "$CCE_PAIR_ROOT/compare.json"
```

两侧结束后重新计算 corpus-manifest 中列出的文件摘要，确认未变化；provider 生成的
ignored 文件需单独归因。检查两侧 view/provider 可用性与模型 digest 一致，否则不是单变量实验。
如果模型缓存分别初始化，性能报告分开冷启动与 warm 请求，不把下载或模型构建算成稳定查询成本。

## 输出与判定

- corpus/dataset 摘要不变，binary、adapter、模型、profile、provider 状态可追溯。
- 统计继续按独立问题族处理；gate 通过只表示未发现显著 guardrail 退化。
- 013/015 等改变图或表示的修复允许排名变化，但需列出变化案例；正确性 fixture 必须通过。
- 018 的 exact parity 使用固定时钟的确定性测试。真实 run 的 history/issue recency 读取
  当前时间，跨秒 score 自然变化不能被误判为缓存导致的质量变化。
- 该附录是执行配方，本次规划没有运行 clone、构建、模型下载或评测。
