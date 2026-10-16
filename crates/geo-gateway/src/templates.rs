use crate::{apply_filter, err, ApiError, QueryReq};
use geo_executor::ExecutionContext;
use geo_ir::*;
use geo_runtime::Runtime;
use geo_types::*;
use axum::http::StatusCode;

const METERS_PER_DEGREE: f64 = 111_320.0;

fn bad(message: impl Into<String>) -> ApiError {
    err(StatusCode::BAD_REQUEST, "bad_request", message)
}

fn source_info(node: &Node) -> Result<(String, Crs, SpatialSchema), ApiError> {
    match node {
        Node::Source { schema, .. } => {
            let geometry = schema.geometry_columns.first().ok_or_else(|| bad("source has no geometry column"))?;
            Ok((geometry.name.clone(), geometry.crs.clone(), schema.clone()))
        }
        _ => Err(bad("source did not produce a source node")),
    }
}

fn lon_lat(crs: &Crs) -> bool {
    matches!(crs, Crs::OgcCrs84 | Crs::Epsg(4326))
}

fn viewport(q: &QueryReq, source_crs: &Crs) -> Result<BoundingBox, ApiError> {
    let crs: Crs = q.crs.parse().map_err(|_| bad("bad crs"))?;
    if crs != *source_crs {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "crs_mismatch",
            format!("viewport CRS {crs} differs from source CRS {source_crs}; explicit transform not available in v0"),
        ));
    }
    BoundingBox::new(q.bbox[0], q.bbox[1], q.bbox[2], q.bbox[3], crs).map_err(|e| bad(e.to_string()))
}

fn right_uri<'a>(with_uri: Option<&'a str>) -> Result<&'a str, ApiError> {
    with_uri.ok_or_else(|| bad("with is required for this kind"))
}

pub(crate) fn build(
    rt: &Runtime,
    uri: &str,
    with_uri: Option<&str>,
    ctx: &ExecutionContext,
    q: &QueryReq,
) -> Result<Plan, ApiError> {
    let with = q.with_req.as_ref().ok_or_else(|| bad("with is required for this kind"))?;
    let left = rt.open_source(uri, ctx)?;
    let right = rt.open_source(right_uri(with_uri)?, ctx)?;
    let (left_geometry, left_crs, left_schema) = source_info(&left)?;
    let (right_geometry, right_crs, right_schema) = source_info(&right)?;
    let bbox = viewport(q, &left_crs)?;

    if right_crs != left_crs {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "crs_mismatch",
            format!("left source CRS {left_crs} differs from right source CRS {right_crs}; explicit transform not available in v0"),
        ));
    }

    match q.kind.as_str() {
        "near" | "near_count" => {
            if !lon_lat(&left_crs) {
                return Err(bad("near only supports OGC:CRS84 or EPSG:4326 sources"));
            }
            let distance_m = with.distance_m.ok_or_else(|| bad("with.distance_m is required for near"))?;
            if !distance_m.is_finite() || !(0.0 < distance_m && distance_m <= 5_000.0) {
                return Err(bad("with.distance_m must be greater than 0 and at most 5000"));
            }
            let distance_degrees = distance_m / METERS_PER_DEGREE;
            let expanded = BoundingBox::new(
                bbox.min_x() - distance_degrees,
                bbox.min_y() - distance_degrees,
                bbox.max_x() + distance_degrees,
                bbox.max_y() + distance_degrees,
                left_crs.clone(),
            )
            .map_err(|e| bad(e.to_string()))?;
            let distance = Distance::new(distance_degrees, DistanceUnit::Degree).map_err(|e| bad(e.to_string()))?;

            let left = apply_filter(
                Node::BboxFilter {
                    input: Box::new(left),
                    geometry_column: left_geometry.clone(),
                    bbox,
                },
                q.filter.as_ref(),
            )?;
            let left_columns = left_schema
                .fields
                .iter()
                .filter(|field| field.name != "bbox")
                .map(|field| field.name.clone())
                .collect::<Vec<_>>();
            let left = Node::Project { input: Box::new(left), columns: left_columns.clone() };

            let right = apply_filter(
                Node::BboxFilter {
                    input: Box::new(right),
                    geometry_column: right_geometry.clone(),
                    bbox: expanded,
                },
                with.filter.as_ref(),
            )?;
            let right = Node::Buffer {
                input: Box::new(Node::Project { input: Box::new(right), columns: vec![right_geometry.clone()] }),
                geometry_column: right_geometry.clone(),
                distance,
                input_crs: right_crs.clone(),
                output_crs: right_crs,
                error_tolerance: None,
                output_column: right_geometry.clone(),
            };
            let joined = Node::SpatialJoin {
                left: Box::new(left),
                right: Box::new(right),
                left_column: left_geometry,
                right_column: right_geometry,
                predicate: SpatialPredicate::Intersects,
                join_type: JoinType::Inner,
                right_prefix: "r_".into(),
            };
            if q.kind == "near_count" {
                // distinct left features near the right ones. The IR cannot group by a geometry column (and Aggregate drops
                // geometry), so group by `id`, then count the groups: a feature near several roads is counted once.
                if !left_columns.iter().any(|c| c == "id") {
                    return Err(bad("near_count needs an id column in the source"));
                }
                let per_feature = Node::Aggregate {
                    input: Box::new(Node::Project { input: Box::new(joined), columns: vec!["id".into()] }),
                    group_by: vec!["id".into()],
                    aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "n_near".into() }],
                };
                return Ok(Plan::new(Node::Aggregate {
                    input: Box::new(per_feature),
                    group_by: vec![],
                    aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                }));
            }
            // `near`: one row per (feature, matching road) pair, left columns only: a feature near several roads appears once
            // per road (identical geometry). `near_count` gives the distinct number.
            let limit = q.limit.unwrap_or(20_000).min(crate::MAX_FEATURES);
            Ok(Plan::new(Node::Limit {
                input: Box::new(Node::Project { input: Box::new(joined), columns: left_columns }),
                n: limit,
            }))
        }
        "join_count_by" => {
            let group_by = with.group_by.as_ref().ok_or_else(|| bad("with.group_by is required for join_count_by"))?;
            match right_schema.fields.iter().find(|field| field.name == *group_by) {
                Some(field) if field.data_type == FieldType::Utf8 => {}
                Some(_) => return Err(bad("with.group_by must name a utf8 column of the right source")),
                None => return Err(bad("with.group_by is not a column of the right source")),
            }

            let left = apply_filter(
                Node::BboxFilter {
                    input: Box::new(left),
                    geometry_column: left_geometry.clone(),
                    bbox: bbox.clone(),
                },
                q.filter.as_ref(),
            )?;
            let left = Node::Project { input: Box::new(left), columns: vec![left_geometry.clone()] };
            let right = apply_filter(
                Node::BboxFilter {
                    input: Box::new(right),
                    geometry_column: right_geometry.clone(),
                    bbox,
                },
                with.filter.as_ref(),
            )?;
            let right = Node::Project {
                input: Box::new(right),
                columns: vec![right_geometry.clone(), group_by.clone()],
            };
            let joined = Node::SpatialJoin {
                left: Box::new(left),
                right: Box::new(right),
                left_column: left_geometry,
                right_column: right_geometry,
                predicate: SpatialPredicate::Intersects,
                join_type: JoinType::Inner,
                right_prefix: "r_".into(),
            };
            Ok(Plan::new(Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate {
                        input: Box::new(joined),
                        group_by: vec![format!("r_{group_by}")],
                        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                    }),
                    keys: vec![SortKey { column: "count".into(), descending: true }],
                }),
                n: 200,
            }))
        }
        _ => Err(bad("unknown kind")),
    }
}
