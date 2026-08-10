from cce_research.metrics import bootstrap, ndcg, overlaps
from cce_research.schema import BenchmarkCase, LineRange, Provenance, RetrievedRange


def test_overlap_is_path_and_line_aware() -> None:
    assert overlaps(
        LineRange(path="a.rs", start_line=10, end_line=20),
        LineRange(path="a.rs", start_line=20, end_line=30),
    )
    assert not overlaps(
        LineRange(path="a.rs", start_line=10, end_line=20),
        LineRange(path="b.rs", start_line=10, end_line=20),
    )


def test_bootstrap_is_deterministic() -> None:
    assert bootstrap([0.0, 1.0], samples=100) == bootstrap([0.0, 1.0], samples=100)


def test_ndcg_counts_each_gold_file_at_most_once() -> None:
    case = BenchmarkCase(
        case_id="case",
        repository="owner/repository",
        revision="revision",
        query="query",
        intent="architecture",
        gold_files=["src/target.ts"],
        provenance=Provenance(
            source_url="https://example.com/repository",
            dataset_revision="dataset-v1",
            license_spdx="MIT",
            redistribution="metadata_only",
            construction_method="test",
        ),
    )
    retrieved = [
        RetrievedRange(
            path="src/target.ts",
            start_line=index,
            end_line=index,
            route="lexical",
            rank=index,
            score=1.0,
            estimated_tokens=1,
        )
        for index in range(1, 4)
    ]

    assert ndcg(case, retrieved) == 1.0
