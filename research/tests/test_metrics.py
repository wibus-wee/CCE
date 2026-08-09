from cce_research.metrics import bootstrap, overlaps
from cce_research.schema import LineRange


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
