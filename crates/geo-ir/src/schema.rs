//! Output-schema derivation for every node.

use crate::expr::AggFunc;
use crate::{IrError, Node};
use geo_types::{Field, FieldType, GeometryColumn, GeometryType, SpatialSchema};

impl Node {
    /// Schema this node produces. Fails when referenced columns are missing
    /// or the node would create ambiguous (duplicate) column names.
    pub fn output_schema(&self) -> Result<SpatialSchema, IrError> {
        match self {
            Node::Source { schema, .. } => {
                schema.validate().map_err(|e| IrError::Schema(e.to_string()))?;
                Ok(schema.clone())
            }
            Node::Filter { input, .. }
            | Node::BboxFilter { input, .. }
            | Node::Intersects { input, .. }
            | Node::Limit { input, .. }
            | Node::Sort { input, .. } => input.output_schema(),
            Node::Project { input, columns } => {
                let s = input.output_schema()?;
                let mut fields = Vec::new();
                for c in columns {
                    let f = field(&s, c)?;
                    if fields.iter().any(|x: &Field| x.name == *c) {
                        return Err(IrError::Schema(format!("duplicate projected column '{c}'")));
                    }
                    fields.push(f.clone());
                }
                let geometry_columns =
                    s.geometry_columns.iter().filter(|g| columns.contains(&g.name)).cloned().collect();
                Ok(SpatialSchema { fields, geometry_columns })
            }
            Node::Buffer { input, geometry_column, output_crs, output_column, .. } => {
                let mut s = input.output_schema()?;
                s.geometry(geometry_column).ok_or_else(|| IrError::ColumnNotFound(geometry_column.clone()))?;
                let out = GeometryColumn {
                    name: output_column.clone(),
                    geometry_type: GeometryType::Polygon,
                    crs: output_crs.clone(),
                };
                if output_column == geometry_column {
                    s.geometry_columns.retain(|g| g.name != *geometry_column);
                } else if s.fields.iter().any(|f| f.name == *output_column) {
                    return Err(IrError::Schema(format!("buffer output column '{output_column}' already exists")));
                } else {
                    s.fields.push(Field {
                        name: output_column.clone(),
                        data_type: FieldType::Other("geometry".into()),
                    });
                }
                s.geometry_columns.push(out);
                Ok(s)
            }
            Node::SpatialJoin { left, right, right_prefix, .. } => {
                let l = left.output_schema()?;
                let r = right.output_schema()?;
                let mut s = l;
                for f in &r.fields {
                    let name = format!("{right_prefix}{}", f.name);
                    if s.fields.iter().any(|x| x.name == name) {
                        return Err(IrError::Schema(format!(
                            "join output column '{name}' is ambiguous; choose a different right_prefix"
                        )));
                    }
                    s.fields.push(Field { name, data_type: f.data_type.clone() });
                }
                for g in &r.geometry_columns {
                    s.geometry_columns.push(GeometryColumn {
                        name: format!("{right_prefix}{}", g.name),
                        geometry_type: g.geometry_type,
                        crs: g.crs.clone(),
                    });
                }
                Ok(s)
            }
            Node::Aggregate { input, group_by, aggregates } => {
                let s = input.output_schema()?;
                let mut fields = Vec::new();
                for g in group_by {
                    if s.geometry(g).is_some() {
                        return Err(IrError::Schema(format!("cannot group by geometry column '{g}'")));
                    }
                    fields.push(field(&s, g)?.clone());
                }
                for a in aggregates {
                    let ty = match (a.func, &a.column) {
                        (AggFunc::Count, _) => FieldType::Int64,
                        (_, None) => {
                            return Err(IrError::Schema(format!("aggregate '{}' needs a column", a.alias)))
                        }
                        (AggFunc::Avg, Some(c)) => {
                            numeric(field(&s, c)?)?;
                            FieldType::Float64
                        }
                        (AggFunc::Sum, Some(c)) => numeric(field(&s, c)?)?.clone(),
                        (AggFunc::Min | AggFunc::Max, Some(c)) => field(&s, c)?.data_type.clone(),
                    };
                    if fields.iter().any(|f| f.name == a.alias) {
                        return Err(IrError::Schema(format!("duplicate output column '{}'", a.alias)));
                    }
                    fields.push(Field { name: a.alias.clone(), data_type: ty });
                }
                Ok(SpatialSchema { fields, geometry_columns: vec![] })
            }
        }
    }
}

pub(crate) fn field<'a>(s: &'a SpatialSchema, name: &str) -> Result<&'a Field, IrError> {
    s.fields.iter().find(|f| f.name == name).ok_or_else(|| IrError::ColumnNotFound(name.to_string()))
}

fn numeric(f: &Field) -> Result<&FieldType, IrError> {
    match f.data_type {
        FieldType::Int64 | FieldType::Float64 => Ok(&f.data_type),
        _ => Err(IrError::Schema(format!("column '{}' is not numeric", f.name))),
    }
}
