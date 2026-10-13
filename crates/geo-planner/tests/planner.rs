//! Planner tests with a MockEngine: proves a second engine plugs in without
//! touching the IR or the planner (RFC-002 "engine #2" check).

use geo_executor::*;
use geo_ir::*;
use geo_planner::*;
use geo_source_geoparquet::GeoParquetSource;
use geo_types::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

struct Mock {
    id: &'static str,
    ops: Vec<OpKind>,
}

impl ExecutionEngine for Mock {
    fn id(&self) -> EngineId {
        EngineId::new(self.id)
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            ops: self.ops.iter().copied().collect::<BTreeSet<_>>(),
            sources: [SourceFormat::GeoParquet].into_iter().collect(),
            location: "mock".into(),
            notes: vec![],
        }
    }
    fn estimate(&self, _: &Plan) -> Result<Estimate, EngineError> {
        Ok(Estimate { rows_upper_bound: Some(1), bytes_to_read: Some(1), confidence: Confidence::Heuristic })
    }
    fn explain(&self, _: &Plan, _: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
        Ok(vec![ExplainStep { label: "mock".into(), detail: self.id.into() }])
    }
    fn execute(&self, _: &Plan, _: &ExecutionContext) -> Result<ExecutionResult, EngineError> {
        Ok(ExecutionResult { batches: vec![], metrics: ExecutionMetrics::default() })
    }
}

fn data() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/data/clustered_10mb_spatial.parquet");
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIP: benchmark data missing");
        None
    }
}

fn plan_for(p: &PathBuf, with_intersects: bool) -> Plan {
    let src = GeoParquetSource::open(p).unwrap().to_ir_source();
    let bb = Node::BboxFilter {
        input: Box::new(src),
        geometry_column: "geometry".into(),
        bbox: BoundingBox::new(104.0, 12.0, 105.0, 13.0, Crs::Epsg(4326)).unwrap(),
    };
    if with_intersects {
        Plan::new(Node::Intersects {
            input: Box::new(bb),
            geometry_column: "geometry".into(),
            with: GeometryLiteral { wkt: "POINT(104.5 12.5)".into(), crs: Crs::Epsg(4326) },
        })
    } else {
        Plan::new(bb)
    }
}

fn ctx() -> ExecutionContext {
    ExecutionContext::new(vec![])
}

#[test]
fn picks_only_capable_engine_and_logs_rejections() {
    let Some(p) = data() else { return };
    let weak = Mock { id: "weak", ops: vec![OpKind::Source, OpKind::BboxFilter] };
    let strong = Mock { id: "strong", ops: vec![OpKind::Source, OpKind::BboxFilter, OpKind::Intersects] };
    let engines: Vec<&dyn ExecutionEngine> = vec![&weak, &strong];
    let phys = Planner::default().plan(&plan_for(&p, true), &engines, &ctx()).unwrap();
    assert_eq!(phys.engine.0, "strong");
    assert!(phys.decisions.iter().any(|d| d.subject == "weak" && d.outcome == Outcome::Rejected));
    assert!(phys.decisions.iter().any(|d| d.outcome == Outcome::Chosen && d.subject == "strong"));
    assert!(phys.decisions.iter().any(|d| d.rule == "spatial_prune"));
    assert!(!phys.steps.is_empty());
}

#[test]
fn first_registered_wins_tie_and_hint_overrides() {
    let Some(p) = data() else { return };
    let a = Mock { id: "a", ops: vec![OpKind::Source, OpKind::BboxFilter] };
    let b = Mock { id: "b", ops: vec![OpKind::Source, OpKind::BboxFilter] };
    let engines: Vec<&dyn ExecutionEngine> = vec![&a, &b];
    let mut plan = plan_for(&p, false);
    let phys = Planner::default().plan(&plan, &engines, &ctx()).unwrap();
    assert_eq!(phys.engine.0, "a");
    assert!(phys.decisions.iter().any(|d| d.rule == "tie_break"));
    plan.hints.preferred_engine = Some("b".into());
    assert_eq!(Planner::default().plan(&plan, &engines, &ctx()).unwrap().engine.0, "b");
    plan.hints.preferred_engine = Some("zzz".into());
    let phys = Planner::default().plan(&plan, &engines, &ctx()).unwrap();
    assert!(phys.decisions.iter().any(|d| d.rule == "hint" && d.outcome == Outcome::Warning));
}

#[test]
fn no_capable_engine_is_an_error_with_reasons() {
    let Some(p) = data() else { return };
    let weak = Mock { id: "weak", ops: vec![OpKind::Source] };
    let engines: Vec<&dyn ExecutionEngine> = vec![&weak];
    match Planner::default().plan(&plan_for(&p, false), &engines, &ctx()) {
        Err(PlanError::NoEngine(msg)) => assert!(msg.contains("bbox_filter")),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn cost_guard_refuses_expensive_scans() {
    let Some(p) = data() else { return };
    let mut m = Mock { id: "m", ops: vec![OpKind::Source, OpKind::BboxFilter] };
    m.ops.push(OpKind::Intersects);
    struct Big(Mock);
    impl ExecutionEngine for Big {
        fn id(&self) -> EngineId {
            self.0.id()
        }
        fn capabilities(&self) -> Capabilities {
            self.0.capabilities()
        }
        fn estimate(&self, _: &Plan) -> Result<Estimate, EngineError> {
            Ok(Estimate { rows_upper_bound: None, bytes_to_read: Some(u64::MAX), confidence: Confidence::Heuristic })
        }
        fn explain(&self, p: &Plan, c: &ExecutionContext) -> Result<Vec<ExplainStep>, EngineError> {
            self.0.explain(p, c)
        }
        fn execute(&self, p: &Plan, c: &ExecutionContext) -> Result<ExecutionResult, EngineError> {
            self.0.execute(p, c)
        }
    }
    let big = Big(m);
    let engines: Vec<&dyn ExecutionEngine> = vec![&big];
    assert!(matches!(
        Planner::default().plan(&plan_for(&p, false), &engines, &ctx()),
        Err(PlanError::TooExpensive(_))
    ));
}

#[test]
fn invalid_plan_is_rejected_before_engines_are_consulted() {
    let Some(p) = data() else { return };
    let mut src = GeoParquetSource::open(&p).unwrap().to_ir_source();
    if let Node::Source { schema, .. } = &mut src {
        schema.geometry_columns[0].crs = Crs::Unknown("x".into());
    }
    let plan = Plan::new(Node::BboxFilter {
        input: Box::new(src),
        geometry_column: "geometry".into(),
        bbox: BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).unwrap(),
    });
    let m = Mock { id: "m", ops: vec![OpKind::Source, OpKind::BboxFilter] };
    let engines: Vec<&dyn ExecutionEngine> = vec![&m];
    assert!(matches!(Planner::default().plan(&plan, &engines, &ctx()), Err(PlanError::Invalid(_))));
}
