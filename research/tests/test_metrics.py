from cce_research.adapters import Adapter, normalize_payload, predicted_intent
from cce_research.metrics import (
    bootstrap,
    case_observations,
    compare,
    component_mrr,
    component_recall,
    evaluate,
    overlaps,
    path_component,
)
from cce_research.schema import (
    BenchmarkCase,
    CaseResult,
    GoldFact,
    LineRange,
    Provenance,
    RetrievedItem,
    RetrievedRange,
)


def _provenance() -> Provenance:
    return Provenance(
        source_url="https://example.com/repo",
        dataset_revision="test-v1",
        license_spdx="Apache-2.0",
        redistribution="allowed",
        construction_method="test fixture",
    )


def _case(**overrides: object) -> BenchmarkCase:
    fields: dict[str, object] = {
        "case_id": "c1",
        "repository": "example/repo",
        "revision": "WORKTREE",
        "query": "q",
        "intent": "impact",
        "gold_files": ["src/a.rs"],
        "provenance": _provenance(),
    }
    fields.update(overrides)
    return BenchmarkCase.model_validate(fields)


def _result(case_id: str = "c1", **overrides: object) -> CaseResult:
    fields: dict[str, object] = {
        "case_id": case_id,
        "system": "test",
        "system_revision": "WORKTREE",
        "dataset_revision": "test-v1",
        "retrieved": [
            RetrievedRange(
                path="src/a.rs",
                start_line=1,
                end_line=10,
                route="lexical",
                rank=1,
                score=1.0,
                estimated_tokens=10,
            )
        ],
        "query_ms": 1.0,
    }
    fields.update(overrides)
    return CaseResult.model_validate(fields)


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


def test_evaluate_groups_metrics_by_intent() -> None:
    cases = [_case(case_id="c1", intent="impact"), _case(case_id="c2", intent="trace")]
    results = [_result("c1"), _result("c2")]
    summary = evaluate(cases, results)
    assert "by_intent/impact/recall@20" in summary
    assert "by_intent/trace/recall@20" in summary
    assert summary["by_intent/impact/recall@20"].samples == 1


def test_intent_accuracy_only_scored_when_intent_withheld() -> None:
    supplied = _case(case_id="c1", supply_intent=True)
    withheld = _case(case_id="c2", supply_intent=False)
    results = [
        _result("c1", predicted_intent="trace"),
        _result("c2", predicted_intent="impact"),
    ]
    summary = evaluate([supplied, withheld], results)
    assert summary["intent_accuracy"].value == 1.0
    assert summary["intent_accuracy"].samples == 1


def test_intent_accuracy_counts_classifier_misses() -> None:
    withheld = _case(case_id="c1", supply_intent=False)
    result = _result("c1", predicted_intent="natural_language_behavior")
    summary = evaluate([withheld], [result])
    assert summary["intent_accuracy"].value == 0.0


def test_compare_reports_paired_deltas() -> None:
    case = _case()
    weak = _result(retrieved=[], abstained=True)
    strong = _result()
    deltas = compare([case], [weak], [strong])
    assert deltas["recall@20"].delta == 1.0
    assert deltas["mrr"].delta == 1.0


def test_bpref_ignores_unjudged_but_penalizes_judged_irrelevant() -> None:
    from cce_research.metrics import bpref

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    # Gold at rank 2, one unjudged item above it: unjudged costs nothing.
    case = _case(gold_files=["src/gold.rs"])
    assert bpref(case, [hit("src/unknown.rs", 1), hit("src/gold.rs", 2)]) == 1.0
    # Same shape but the item above is adjudicated irrelevant: penalty.
    judged = _case(gold_files=["src/gold.rs"], judged_files=["src/unknown.rs"])
    assert bpref(judged, [hit("src/unknown.rs", 1), hit("src/gold.rs", 2)]) == 0.0


def test_unjudged_rate_counts_only_unverdicted_paths() -> None:
    from cce_research.metrics import unjudged_rate

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    case = _case(gold_files=["src/gold.rs"], judged_files=["src/no.rs"])
    rate = unjudged_rate(case, [hit("src/gold.rs", 1), hit("src/no.rs", 2), hit("src/?.rs", 3)])
    assert rate == 1 / 3
    assert unjudged_rate(case, []) == 0.0


def test_decoy_hit_rate_counts_only_adjudicated_irrelevant() -> None:
    from cce_research.metrics import decoy_hit_rate

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    case = _case(gold_files=["src/gold.rs"], judged_files=["src/decoy.rs"])
    retrieved = [hit("src/gold.rs", 1), hit("src/decoy.rs", 2), hit("src/?.rs", 3)]
    # gold + unjudged don't count; only the adjudicated-irrelevant decoy.
    assert decoy_hit_rate(case, retrieved) == 1 / 3
    assert decoy_hit_rate(case, []) == 0.0
    # Decoys count wherever they rank — unlike bpref, after gold too.
    assert decoy_hit_rate(case, [hit("src/gold.rs", 1), hit("src/decoy.rs", 2)]) == 0.5
    # A path in both gold and judged is data noise, not a decoy.
    weird = _case(gold_files=["src/gold.rs"], judged_files=["src/gold.rs"])
    assert decoy_hit_rate(weird, [hit("src/gold.rs", 1)]) == 0.0
    # no_context: every surfaced path is a convicted false positive —
    # adjudication is not needed to know nothing should have been returned.
    nc = _case(gold_files=[], no_context=True, judged_files=["src/decoy.rs"])
    assert decoy_hit_rate(nc, [hit("src/decoy.rs", 1), hit("src/?.rs", 2)]) == 1.0
    assert decoy_hit_rate(nc, []) == 0.0


def test_unjudged_rate_reports_no_debt_on_no_context() -> None:
    from cce_research.metrics import unjudged_rate

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    # On a no_context case every hit is known-irrelevant by definition —
    # nothing awaits adjudication, so there is no pooling debt to report.
    nc = _case(gold_files=[], no_context=True)
    assert unjudged_rate(nc, [hit("src/a.rs", 1), hit("src/b.rs", 2)]) == 0.0


def test_file_hit_is_any_gold_at_skim_depth() -> None:
    from cce_research.metrics import file_hit

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    case = _case(gold_files=["src/a.rs", "src/b.rs"])
    # "Any gold" semantics — unlike file_success, one of two suffices.
    assert file_hit(case, [hit("src/a.rs", 1)]) == 1.0
    assert file_hit(case, [hit("src/no.rs", 1)]) == 0.0
    # no_context: abstention is the success state.
    nc = _case(gold_files=[], no_context=True)
    assert file_hit(nc, []) == 1.0
    assert file_hit(nc, [hit("src/x.rs", 1)]) == 0.0


def test_redundancy_counts_repeat_paths_in_topk() -> None:
    from cce_research.metrics import redundancy

    def hit(path: str, rank: int) -> RetrievedRange:
        return RetrievedRange(
            path=path, start_line=1, end_line=5, route="lexical",
            rank=rank, score=1.0, estimated_tokens=5,
        )

    case = _case(gold_files=["src/a.rs"])
    retrieved = [hit("src/a.rs", 1), hit("src/a.rs", 2), hit("src/b.rs", 3), hit("src/a.rs", 4)]
    assert redundancy(case, retrieved) == 0.5
    assert redundancy(case, []) == 0.0
    # no_context: hits are already convicted decoys — no double charge.
    nc = _case(gold_files=[], no_context=True)
    assert redundancy(nc, [hit("src/a.rs", 1), hit("src/a.rs", 2)]) == 0.0


def _context_item(
    item_id: str,
    rank: int,
    tokens: int,
    path: str | None = "src/a.rs",
    start: int = 1,
    end: int = 5,
    symbol: str | None = None,
    kind: str = "source",
) -> RetrievedItem:
    primary = (
        LineRange(path=path, start_line=start, end_line=end, symbol=symbol)
        if path
        else None
    )
    return RetrievedItem(
        item_id=item_id,
        rank=rank,
        score=1.0,
        route="lexical",
        symbol=symbol,
        estimated_tokens=tokens,
        kind=kind,
        primary=primary,
    )


def _context_result(*items: RetrievedItem, used_tokens: int | None = None) -> CaseResult:
    return _result(
        result_kind="context",
        items=list(items),
        retrieved=[row for item in items for row in item.to_ranges()],
        used_tokens=used_tokens,
        metrics_version=2,
    )


def test_pack_sufficiency_scores_what_the_pack_delivered() -> None:
    from cce_research.metrics import pack_sufficiency

    case = _case(
        gold_facts=[
            GoldFact(
                claim="a",
                evidence=[LineRange(path="src/a.rs", start_line=1, end_line=5)],
            ),
            GoldFact(
                claim="b",
                evidence=[LineRange(path="src/b.rs", start_line=1, end_line=5)],
            ),
        ],
        budget_tokens=256,
    )
    # The engine already applied the budget: items are the admitted set.
    # Fact b's evidence never made the pack, so sufficiency is 0.5 — the
    # metric reads the delivered items, not a re-packed guess.
    delivered = _context_result(
        _context_item("i1", 1, 256, "src/a.rs"),
        _context_item("i2", 2, 256, "src/b.rs"),
        used_tokens=512,
    )
    assert pack_sufficiency(case, delivered) == 1.0
    cut = _context_result(
        _context_item("i1", 1, 256, "src/a.rs"), used_tokens=256
    )
    assert pack_sufficiency(case, cut) == 0.5
    # Search results carry no packing stage — uncomputable, not free points.
    assert pack_sufficiency(case, _result()) is None
    # Legacy flat results cannot reconstruct what was packed.
    assert pack_sufficiency(case, _result(retrieved=[])) is None


def test_case_observations_emits_decoy_hit_rate() -> None:
    case = _case(judged_files=["src/decoy.rs"])
    observations = case_observations(case, _result())
    for cutoff in (5, 10, 20, 50):
        assert f"decoy_hit_rate@{cutoff}" in observations


def test_case_clusters_groups_derived_families() -> None:
    from cce_research.metrics import case_clusters

    parent = _case(case_id="a")
    variant = _case(case_id="a::var-verbose", derived_from="a", derivation="variant:verbose")
    pin = _case(case_id="a::pin", derived_from="a::var-verbose", derivation="pin:lexical")
    other = _case(case_id="b")
    orphan = _case(case_id="c::x", derived_from="missing-parent", derivation="x")
    clusters = case_clusters([parent, variant, pin, other, orphan])
    assert clusters["a"] == clusters["a::var-verbose"] == clusters["a::pin"] == "a"
    assert clusters["b"] == "b"
    # A missing parent still groups siblings under its id.
    assert clusters["c::x"] == "missing-parent"


def test_evaluate_collapses_families_to_one_observation() -> None:
    # Three derived rows all succeeding vs one singleton failing: flat
    # weighting says 0.75, cluster weighting says 0.5 — each *real* case
    # counts once.
    parent = _case(case_id="a")
    derived = [
        _case(case_id=f"a::d{i}", derived_from="a", derivation=f"d{i}") for i in range(2)
    ]
    other = _case(case_id="b")
    cases = [parent, *derived, other]
    results = [_result("a"), _result("a::d0"), _result("a::d1"), _result("b", retrieved=[])]
    summary = evaluate(cases, results)
    assert summary["file_recall@20"].value == 0.5
    assert summary["file_recall@20"].samples == 2


def test_evaluate_emits_by_tag_groups() -> None:
    case = _case(tags=["adversarial", "near-miss"])
    summary = evaluate([case], [_result()])
    assert "by_tag/adversarial/file_recall@20" in summary
    assert "by_tag/near-miss/file_recall@20" in summary


def test_budget_compliant_scored_only_when_tokens_present() -> None:
    # Context results are audited against the pack's own accounting:
    # top-level usedTokens first, else the item sum (orientation items
    # with no address still consume budget).
    over = _context_result(
        _context_item("i1", 1, 9000), used_tokens=9000
    )
    assert case_observations(_case(), over)["budget_compliant"] == 0.0
    packed = _context_result(
        _context_item("i1", 1, 100),
        # No usedTokens: the item sum still audits the budget.
    )
    assert case_observations(_case(), packed)["budget_compliant"] == 1.0
    # Search results carry no packing stage to audit.
    assert "budget_compliant" not in case_observations(_case(), _result())
    # Legacy flat rows double-counted per address; they must not guess a
    # budget verdict from unrecoverable accounting.
    legacy = _result(
        retrieved=[
            RetrievedRange(
                path="src/a.rs", start_line=1, end_line=5, route="lexical",
                rank=1, score=1.0, estimated_tokens=9000,
            )
        ]
    )
    assert "budget_compliant" not in case_observations(_case(), legacy)


def test_dual_address_item_counts_tokens_once() -> None:
    """The 012 acceptance case: a 100-token item citing two addresses is
    a 100-token pack entry and one rank slot — not 200 tokens and two
    slots."""
    item = RetrievedItem(
        item_id="i1",
        rank=1,
        score=1.0,
        route="lexical",
        estimated_tokens=100,
        primary=LineRange(path="src/a.rs", start_line=1, end_line=9),
        supporting=[LineRange(path="src/b.rs", start_line=20, end_line=30)],
    )
    result = _context_result(item, used_tokens=100)
    # Two flat compat rows, one item — rank positions count items.
    assert len(result.retrieved) == 2
    assert len(result.items) == 1
    assert all(row.rank == 1 for row in result.retrieved)
    observations = case_observations(_case(), result)
    assert observations["used_tokens"] == 100.0
    assert observations["budget_compliant"] == 1.0
    # @1 cutoff keeps the whole item — both addresses are observable.
    from cce_research.metrics import observed_ranges

    assert {row.path for row in observed_ranges(result, 1)} == {"src/a.rs", "src/b.rs"}


def test_orientation_item_consumes_budget_without_rank_slot() -> None:
    orientation = _context_item(
        "orient", 1, 800, path=None, kind="orientation"
    )
    source = _context_item("i2", 2, 100, "src/a.rs")
    result = _context_result(orientation, source, used_tokens=900)
    # Orientation produced no flat row but still spent tokens.
    assert len(result.retrieved) == 1
    assert case_observations(_case(), result)["used_tokens"] == 900.0


def test_verdict_state_recorded_separately_from_empty_results() -> None:
    weak = _result(verdict_state="weak_witness", metrics_version=2)
    observations = case_observations(_case(no_context=True), weak)
    # Weak witness keeps candidates: abstention_accuracy (empty-rows
    # semantics) is 0, but the verdict layer records the flag.
    assert observations["abstention_accuracy"] == 0.0
    assert observations["verdict_weak_witness"] == 1.0
    assert observations["verdict_flagged"] == 1.0
    assert observations["verdict_answered"] == 0.0
    abstained = _result(retrieved=[], abstained=True, verdict_state="abstained")
    observations = case_observations(_case(no_context=True), abstained)
    assert observations["abstention_accuracy"] == 1.0
    assert observations["verdict_abstained"] == 1.0


def test_compare_rejects_mixed_metrics_versions() -> None:
    cases = [_case(case_id="c1")]
    import pytest

    with pytest.raises(ValueError, match="metrics_version"):
        compare(
            cases,
            [_result("c1")],  # legacy v1
            [_result("c1", metrics_version=2, result_kind="search")],
        )


def test_compare_reports_cluster_sample_size() -> None:
    parent = _case(case_id="a")
    derived = _case(case_id="a::v", derived_from="a", derivation="variant:verbose")
    other = _case(case_id="b")
    cases = [parent, derived, other]
    weak = [_result("a", retrieved=[]), _result("a::v", retrieved=[]), _result("b", retrieved=[])]
    strong = [_result("a"), _result("a::v"), _result("b")]
    deltas = compare(cases, weak, strong)
    assert deltas["file_recall@20"].samples == 2  # two families


def test_lower_is_better_covers_adversarial_metrics_only() -> None:
    from cce_research.metrics import LOWER_IS_BETTER

    assert "decoy_hit_rate@20" in LOWER_IS_BETTER
    assert "false_positive_rate" in LOWER_IS_BETTER
    assert "query_ms" in LOWER_IS_BETTER
    # Quality metrics stay higher-is-better; unjudged_rate is a gold-
    # completeness diagnostic and must never gate.
    for name in ("file_success@20", "unjudged_rate@20", "abstention_accuracy", "ndcg@10"):
        assert name not in LOWER_IS_BETTER


def test_claim_support_scores_facts_not_files() -> None:
    from cce_research.metrics import claim_support, claim_support_legacy
    from cce_research.schema import GoldFact

    case = _case(
        gold_facts=[
            GoldFact(
                claim="status is written via set_view_status",
                evidence=[LineRange(path="src/store.rs", start_line=10, end_line=30)],
                symbols=["set_view_status"],
            ),
            GoldFact(claim="unreachable claim", evidence=[], symbols=["missing_fn"]),
        ]
    )
    hit = RetrievedRange(
        path="src/other.rs", start_line=1, end_line=5, symbol="set_view_status",
        route="lexical", rank=1, score=1.0, estimated_tokens=5,
    )
    # Strict: a same-named symbol in the wrong file does NOT support the
    # fact — the evidence localizes it to src/store.rs. The bare-symbol
    # fact (no evidence paths) is undecidable: excluded from the
    # denominator and reported, never silently passed or failed.
    assert claim_support(case, [hit]) == (0.0, 1)
    assert claim_support(case, []) == (0.0, 1)
    # The scoped symbol on the right file does support it.
    right = RetrievedRange(
        path="src/store.rs", start_line=40, end_line=60, symbol="set_view_status",
        route="lexical", rank=1, score=1.0, estimated_tokens=5,
    )
    assert claim_support(case, [right]) == (1.0, 1)
    # The legacy loose criterion kept the old answer for continuity —
    # the two definitions must never be mixed in one comparison.
    assert claim_support_legacy(case, [hit]) == 0.5


def test_evaluate_emits_intent_precision_and_recall() -> None:
    withheld_a = _case(case_id="a", supply_intent=False, intent="impact")
    withheld_b = _case(case_id="b", supply_intent=False, intent="impact")
    withheld_c = _case(case_id="c", supply_intent=False, intent="trace")
    results = [
        _result("a", predicted_intent="impact"),
        _result("b", predicted_intent="natural_language_behavior"),
        _result("c", predicted_intent="trace"),
    ]
    summary = evaluate([withheld_a, withheld_b, withheld_c], results)
    assert summary["intent_recall/impact"].value == 0.5
    assert summary["intent_precision/natural_language_behavior"].value == 0.0
    assert summary["intent_precision/impact"].value == 1.0


def test_ndcg_never_exceeds_one_with_duplicate_gold_paths() -> None:
    case = _case()
    duplicated = [
        RetrievedRange(
            path="src/a.rs",
            start_line=offset,
            end_line=offset + 5,
            route="lexical",
            rank=index + 1,
            score=1.0,
            estimated_tokens=10,
        )
        for index, offset in enumerate(range(1, 60, 6))
    ]
    summary = evaluate([case], [_result(retrieved=duplicated)])
    assert summary["ndcg@10"].value <= 1.0


def test_compare_rejects_mismatched_coverage() -> None:
    cases = [_case(case_id="c1"), _case(case_id="c2")]
    try:
        compare(cases, [_result("c1")], [_result("c1"), _result("c2")])
    except ValueError as error:
        assert "coverage" in str(error)
    else:
        raise AssertionError("compare must reject missing case coverage")


def test_build_command_omits_intent_when_withheld() -> None:
    adapter = Adapter(
        name="t",
        command=["cce", "context", "{repository}", "{query}", "{intent_args}", "{route_args}"],
        timeout_seconds=10,
        environment={},
        model_identity="none",
        model_revision="none",
    )
    supplied = adapter.build_command(_case(), __import__("pathlib").Path("/repo"))
    assert supplied[-2:] == ["--intent", "impact"]
    withheld_cmd = adapter.build_command(
        _case(supply_intent=False), __import__("pathlib").Path("/repo")
    )
    assert "--intent" not in withheld_cmd


def test_build_command_expands_route_overrides() -> None:
    adapter = Adapter(
        name="t",
        command=["cce", "{route_args}"],
        timeout_seconds=10,
        environment={},
        model_identity="none",
        model_revision="none",
    )
    command = adapter.build_command(
        _case(routes=["lexical", "exact_symbol"]), __import__("pathlib").Path("/repo")
    )
    assert command == ["cce", "--route", "lexical", "--route", "exact_symbol"]


def test_normalize_payload_handles_search_results() -> None:
    payload = {
        "plan": {"intent": "impact", "routes": ["lexical"], "graphPolicy": "incoming_impact"},
        "verdict": {"state": "weak_witness", "reasons": ["gap"]},
        "request": {"snapshotId": "snap-1"},
        "hits": [
            {
                "documentId": "d1",
                "entityId": "e1",
                "symbolName": "SourceAddress",
                "route": "exact_symbol",
                "rank": 1,
                "score": 0.5,
                "address": {"path": "src/a.rs", "startLine": 3, "endLine": 9},
                "verifiedCurrent": True,
            }
        ],
    }
    normalized = normalize_payload(payload)
    assert normalized.result_kind == "search"
    assert normalized.verdict_state == "weak_witness"
    retrieved = normalized.retrieved
    assert retrieved[0].path == "src/a.rs"
    assert retrieved[0].symbol == "SourceAddress"
    assert retrieved[0].item_id == "d1"
    assert normalized.items[0].item_id == "d1"
    assert normalized.items[0].snapshot_id == "snap-1"
    assert predicted_intent(payload) == "impact"


def test_normalize_payload_handles_context_packs() -> None:
    payload = {
        "intent": "natural_language_behavior",
        "usedTokens": 42,
        "items": [
            {
                "id": "item-1",
                "kind": "source",
                "estimatedTokens": 42,
                "provenance": {
                    "route": "lexical",
                    "rank": 2,
                    "score": 0.25,
                    "snapshotId": "snap-9",
                    "sourceAddress": {"path": "src/b.rs", "startLine": 1, "endLine": 5},
                    "verifiedCurrent": True,
                },
            }
        ],
    }
    normalized = normalize_payload(payload)
    assert normalized.result_kind == "context"
    assert normalized.used_tokens == 42
    retrieved = normalized.retrieved
    assert retrieved[0].path == "src/b.rs"
    assert retrieved[0].item_id == "item-1"
    assert normalized.items[0].snapshot_id == "snap-9"
    assert predicted_intent(payload) == "natural_language_behavior"


def _component_map() -> dict[str, str]:
    return {"cce-store": "crates/cce-store", "cce-engine": "crates/cce-engine", "root": "."}


def test_path_component_longest_prefix_wins() -> None:
    component_map = {
        "cce-store": "crates/cce-store",
        "nested": "crates/cce-store/nested",
        "root": ".",
    }
    assert path_component("crates/cce-store/nested/x.rs", component_map) == "nested"
    assert path_component("crates/cce-store/src/a.rs", component_map) == "cce-store"
    assert path_component("README.md", component_map) == "root"
    without_root = {k: v for k, v in component_map.items() if k != "root"}
    assert path_component("other/a.rs", without_root) is None


def test_component_metrics_use_map() -> None:
    case = _case(gold_components=["cce-store"])
    result = _result(
        retrieved=[
            RetrievedRange(
                path="crates/cce-engine/src/a.rs",
                start_line=1,
                end_line=5,
                route="lexical",
                rank=1,
                score=1.0,
                estimated_tokens=5,
            ),
            RetrievedRange(
                path="crates/cce-store/src/b.rs",
                start_line=1,
                end_line=5,
                route="lexical",
                rank=2,
                score=0.9,
                estimated_tokens=5,
            ),
        ],
        component_map=_component_map(),
    )
    assert component_recall(case, result.retrieved[:1], result.component_map) == 0.0
    assert component_recall(case, result.retrieved[:5], result.component_map) == 1.0
    assert component_mrr(case, result.retrieved, result.component_map) == 0.5


def test_component_metrics_skip_when_unannotated_or_unmapped() -> None:
    case = _case()
    result = _result(component_map=_component_map())
    observations = case_observations(case, result)
    assert "component_recall_at_5" not in observations
    case = _case(gold_components=["cce-store"])
    result = _result(component_map={})
    observations = case_observations(case, result)
    assert "component_recall_at_5" not in observations
