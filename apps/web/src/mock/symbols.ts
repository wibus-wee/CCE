/**
 * MOCK — fixture symbol index + cross-reference sites for the Symbols
 * surface (Sourcegraph's symbol sidebar / "find references" panel).
 * `/v1/symbols` doesn't exist yet and no SCIP cross-reference index is
 * wired up, so the screen carries a `preview` badge — fixtures are never
 * live index truth.
 *
 * `refCount` is the headline occurrence count a real index would report
 * (kept from the original fixture); `references` lists the authored
 * fixture sites — a sample that can be smaller than `refCount`, the same
 * relationship a truncated SG result list has to its count badge. When a
 * real endpoint lands, delete this file and switch the screen to the API.
 */

export type MockSymbolKind =
    | 'function'
    | 'struct'
    | 'trait'
    | 'module'
    | 'constant'
    | 'component'
    | 'type'

/** One cross-reference site — file, line, and the source text around the hit. */
export interface MockSymbolReference {
    path: string
    line: number
    /** Faint mono snippet shown beside the site — the source line as indexed. */
    snippet: string
}

export interface MockSymbolEntry {
    name: string
    kind: MockSymbolKind
    qualifiedName: string
    path: string
    line: number
    language: 'rust' | 'typescript' | 'python'
    /** Headline reference count (what the Refs column reports). */
    refCount: number
    /** Fixture reference sites — a sample, not a real cross-reference index. */
    references: MockSymbolReference[]
    route: string
    confidence: number
}

export const MOCK_SYMBOLS: MockSymbolEntry[] = [
    {
        name: 'snapshot_freshness', kind: 'function', qualifiedName: 'cce_engine::snapshot::freshness',
        path: 'crates/cce-engine/src/snapshot.rs', line: 84, language: 'rust',
        refCount: 17, route: 'exact_symbol', confidence: 0.98,
        references: [
            { path: 'crates/cce-engine/src/snapshot.rs', line: 151, snippet: 'debug_assert!(snapshot_freshness(&m, s) >= 0.0);' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 112, snippet: 'let fresh = snapshot_freshness(&view.manifest, snap)?;' },
            { path: 'crates/cce-engine/src/retrieval.rs', line: 204, snippet: 'if snapshot_freshness(&v, snap) < MIN_FRESH {' },
            { path: 'apps/daemon/src/service.rs', line: 88, snippet: 'report.freshness = snapshot_freshness(&m, snap);' },
            { path: 'crates/cce-engine/src/lib.rs', line: 42, snippet: 'pub use snapshot::snapshot_freshness;' },
        ],
    },
    {
        name: 'IndexLease', kind: 'struct', qualifiedName: 'cce_engine::lease::IndexLease',
        path: 'crates/cce-engine/src/lease.rs', line: 41, language: 'rust',
        refCount: 9, route: 'structural', confidence: 0.96,
        references: [
            { path: 'crates/cce-engine/src/lease.rs', line: 96, snippet: 'impl Drop for IndexLease {' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 58, snippet: 'let lease = IndexLease::acquire(&views)?;' },
            { path: 'apps/daemon/src/writer.rs', line: 133, snippet: 'lease: IndexLease,' },
            { path: 'crates/cce-engine/src/lease.rs', line: 71, snippet: 'pub fn acquire(views: &Views) -> Result<IndexLease> {' },
        ],
    },
    {
        name: 'rank_fused', kind: 'function', qualifiedName: 'cce_engine::retrieval::rank_fused',
        path: 'crates/cce-engine/src/retrieval.rs', line: 318, language: 'rust',
        refCount: 23, route: 'lexical', confidence: 0.91,
        references: [
            { path: 'crates/cce-engine/src/retrieval.rs', line: 402, snippet: 'let fused = rank_fused(&lex, &dense, w)?;' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 187, snippet: 'hits = rank_fused(hits, &plan.weights)?;' },
            { path: 'crates/cce-engine/src/fusion.rs', line: 33, snippet: 'pub(crate) use crate::retrieval::rank_fused;' },
            { path: 'apps/cli/src/query.rs', line: 74, snippet: 'let ranked = rank_fused(&hits, &w)?;' },
            { path: 'crates/cce-engine/src/retrieval.rs', line: 355, snippet: 'debug_assert!(rank_fused(&a, &b, w).is_ok());' },
        ],
    },
    {
        name: 'pack_context', kind: 'function', qualifiedName: 'cce_engine::packing::pack_context',
        path: 'crates/cce-engine/src/packing.rs', line: 156, language: 'rust',
        refCount: 11, route: 'hybrid', confidence: 0.94,
        references: [
            { path: 'crates/cce-engine/src/packing.rs', line: 201, snippet: 'let pack = pack_context(&hits, budget)?;' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 244, snippet: 'let ctx = pack_context(&evid, tokens)?;' },
            { path: 'apps/cli/src/pack.rs', line: 51, snippet: 'pack_context(&sel, cli.budget)' },
            { path: 'crates/cce-engine/src/manifest.rs', line: 143, snippet: '// pack_context consumes this ordering' },
        ],
    },
    {
        name: 'ViewManifest', kind: 'struct', qualifiedName: 'cce_engine::manifest::ViewManifest',
        path: 'crates/cce-engine/src/manifest.rs', line: 29, language: 'rust',
        refCount: 31, route: 'structural', confidence: 0.97,
        references: [
            { path: 'crates/cce-engine/src/manifest.rs', line: 87, snippet: 'impl ViewManifest {' },
            { path: 'crates/cce-engine/src/snapshot.rs', line: 66, snippet: 'manifest: ViewManifest,' },
            { path: 'apps/daemon/src/service.rs', line: 52, snippet: 'fn report(m: &ViewManifest) -> Status {' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 34, snippet: 'pub struct Pipeline { m: ViewManifest }' },
            { path: 'crates/cce-engine/src/lease.rs', line: 58, snippet: 'views: &ViewManifest,' },
        ],
    },
    {
        name: 'SearchRoute', kind: 'type', qualifiedName: 'cce_engine::plan::SearchRoute',
        path: 'crates/cce-engine/src/plan.rs', line: 66, language: 'rust',
        refCount: 44, route: 'structural', confidence: 0.99,
        references: [
            { path: 'crates/cce-engine/src/plan.rs', line: 104, snippet: 'match route { SearchRoute::Lexical => ..' },
            { path: 'crates/cce-engine/src/retrieval.rs', line: 140, snippet: 'route: SearchRoute,' },
            { path: 'crates/cce-engine/src/plan.rs', line: 88, snippet: 'pub enum SearchRoute {' },
            { path: 'apps/cli/src/query.rs', line: 39, snippet: 'let r = SearchRoute::from(q.flags);' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 96, snippet: 'plan.route = SearchRoute::Hybrid;' },
        ],
    },
    {
        name: 'RetrievalProvider', kind: 'trait', qualifiedName: 'cce_engine::providers::RetrievalProvider',
        path: 'crates/cce-engine/src/providers.rs', line: 12, language: 'rust',
        refCount: 14, route: 'structural', confidence: 0.95,
        references: [
            { path: 'crates/cce-engine/src/providers.rs', line: 47, snippet: 'impl RetrievalProvider for Lexical {' },
            { path: 'crates/cce-engine/src/providers.rs', line: 73, snippet: 'impl RetrievalProvider for Dense {' },
            { path: 'crates/cce-engine/src/pipeline.rs', line: 129, snippet: 'providers: Vec<Box<dyn RetrievalProvider>>,' },
            { path: 'apps/daemon/src/service.rs', line: 112, snippet: 'fn probe(p: &dyn RetrievalProvider) {' },
        ],
    },
    {
        name: 'ARTIFACT_STORE_SCHEMA', kind: 'constant', qualifiedName: 'cce_engine::store::ARTIFACT_STORE_SCHEMA',
        path: 'crates/cce-engine/src/store.rs', line: 8, language: 'rust',
        refCount: 6, route: 'exact_symbol', confidence: 0.93,
        references: [
            { path: 'crates/cce-engine/src/store.rs', line: 62, snippet: 'debug_assert_eq!(v, ARTIFACT_STORE_SCHEMA);' },
            { path: 'crates/cce-engine/src/migrate.rs', line: 27, snippet: 'if meta.schema != ARTIFACT_STORE_SCHEMA {' },
            { path: 'apps/cli/src/doctor.rs', line: 81, snippet: 'println!("schema {}", ARTIFACT_STORE_SCHEMA);' },
        ],
    },
    {
        name: 'QueryScreen', kind: 'component', qualifiedName: 'web::screens::QueryScreen',
        path: 'apps/web/src/screens/QueryScreen.tsx', line: 33, language: 'typescript',
        refCount: 2, route: 'lexical', confidence: 0.88,
        references: [
            { path: 'apps/web/src/App.tsx', line: 24, snippet: 'import { QueryScreen } from \'./screens/QueryScreen\'' },
            { path: 'apps/web/src/App.tsx', line: 530, snippet: 'element={<QueryScreen onOpenFile={openFile} />}' },
        ],
    },
    {
        name: 'DisplayBadge', kind: 'component', qualifiedName: 'web::ui::DisplayBadge',
        path: 'apps/web/src/ui/DisplayBadge.tsx', line: 14, language: 'typescript',
        refCount: 19, route: 'lexical', confidence: 0.9,
        references: [
            { path: 'apps/web/src/screens/SymbolsScreen.tsx', line: 6, snippet: 'import { DisplayBadge } from \'../ui/DisplayBadge\'' },
            { path: 'apps/web/src/screens/CommitsScreen.tsx', line: 15, snippet: 'import { DisplayBadge } from \'../ui/DisplayBadge\'' },
            { path: 'apps/web/src/screens/BranchesScreen.tsx', line: 10, snippet: 'import { DisplayBadge } from \'../ui/DisplayBadge\'' },
            { path: 'apps/web/src/ui/DisplayKeyValue.tsx', line: 3, snippet: 'import { DisplayBadge } from \'./DisplayBadge\'' },
            { path: 'apps/web/src/screens/IndexScreen.tsx', line: 12, snippet: 'import { DisplayBadge } from \'../ui/DisplayBadge\'' },
        ],
    },
    {
        name: 'embedding', kind: 'module', qualifiedName: 'cce_engine::embedding',
        path: 'crates/cce-engine/src/embedding.rs', line: 1, language: 'rust',
        refCount: 8, route: 'structural', confidence: 0.85,
        references: [
            { path: 'crates/cce-engine/src/lib.rs', line: 19, snippet: 'pub mod embedding;' },
            { path: 'crates/cce-engine/src/dense.rs', line: 7, snippet: 'use crate::embedding::{self, Backend};' },
            { path: 'crates/cce-engine/src/providers.rs', line: 61, snippet: 'embedding::encode(&q, &self.model)?' },
        ],
    },
    {
        name: 'score_late_fusion', kind: 'function', qualifiedName: 'cce_engine::fusion::score_late_fusion',
        path: 'crates/cce-engine/src/fusion.rs', line: 77, language: 'rust',
        refCount: 5, route: 'dense_summary', confidence: 0.72,
        references: [
            { path: 'crates/cce-engine/src/fusion.rs', line: 119, snippet: 'let s = score_late_fusion(&lex, &dense)?;' },
            { path: 'crates/cce-engine/src/retrieval.rs', line: 371, snippet: 'rank = score_late_fusion(rank, &d)?;' },
        ],
    },
    {
        name: 'taint_pass', kind: 'function', qualifiedName: 'cce_engine::dataflow::taint_pass',
        path: 'crates/cce-engine/src/dataflow.rs', line: 203, language: 'rust',
        refCount: 0, route: 'structural', confidence: 0.61,
        references: [],
    },
]
