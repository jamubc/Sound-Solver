//! Structural edits made in the editor: placing an element on a pipe run and taking one out
//! (a muffler or resonator delete). Every route stays attached to its ports, and an edit that
//! would leave the project invalid is refused with the reason, leaving it unchanged.

use crate::error::{Error, Result};
use crate::geometry::{Piece, add, centreline, cross, norm, scale, sub, unit};
use crate::manifest::{self, ParamKind};
use crate::project::{PortRef, Project, Route};

/// Places a new element of type `kind` (manifest defaults) on route `route` at arc length
/// `s_mm` from its start. The route is cut there: it now ends at the element's `in`, and a new
/// route runs from the element's `out` to the old end. The element's axis follows the pipe;
/// it needs a straight run long enough for it. Branches leave horizontally across the pipe.
/// Returns the new element's id.
pub fn insert(project: &mut Project, route: &str, s_mm: f64, kind: &str) -> Result<String> {
    let mut edited = project.clone();
    let index = edited
        .system
        .routes
        .iter()
        .position(|r| r.id == route)
        .ok_or_else(|| Error::invalid(format!("no route '{route}'")))?;
    let original = edited.system.routes[index].clone();
    let points = edited.route_points(&original)?;
    let line = centreline(&points, &edited.route_radii(&original))?;
    let s = s_mm * 1e-3;
    // Centreline makes one straight piece per segment, in order.
    let (segment, from, to, s0, length) = line
        .pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Straight {
                s0,
                length,
                from,
                to,
            } => Some((*from, *to, *s0, *length)),
            Piece::Arc { .. } => None,
        })
        .enumerate()
        .find(|(_, (_, _, s0, length))| s >= *s0 && s <= s0 + length)
        .map(|(i, (f, t, s0, l))| (i, f, t, s0, l))
        .ok_or_else(|| Error::invalid("place elements on a straight run, not in a bend"))?;
    let axis = unit(sub(to, from));
    let at = add(from, scale(axis, s - s0));
    let id = unique(kind, |c| edited.element(c).is_some());
    let mut element = manifest::element(kind, &id, scale(at, 1e3), axis, &original.pipe.material)?;
    let across = unit(cross(axis, [0.0, 0.0, 1.0]));
    for p in &manifest::get(kind).expect("element built").params {
        if p.kind == ParamKind::Direction {
            let mut v = serde_json::to_value(&element).expect("elements serialise");
            v[&p.key] = serde_json::json!(across);
            element = serde_json::from_value(v).expect("direction is a Vec3");
        }
    }
    let out = element
        .port_position("out")
        .ok_or_else(|| Error::invalid(format!("'{kind}' has no in and out to sit on a pipe")))?;
    let needed = norm(sub(out, element.position_mm)) * 1e-3;
    if s - s0 + needed > length + 1e-9 {
        return Err(Error::invalid(format!(
            "a {:.0} mm element needs {:.0} mm of straight pipe from here; {:.0} mm are left",
            needed * 1e3,
            needed * 1e3,
            (length - (s - s0)) * 1e3
        )));
    }
    let split = segment.min(original.via_mm.len());
    let (via_a, via_b) = original.via_mm.split_at(split);
    let radius = |i: usize| original.bend_radius_mm.get(i).copied().flatten();
    // Hangers keep their place on the pipe; any the element now covers go.
    let resume = (s + needed) * 1e3;
    let first = Route {
        id: original.id.clone(),
        from: original.from.clone(),
        to: port(&id, "in"),
        via_mm: via_a.to_vec(),
        bend_radius_mm: (0..split).map(radius).collect(),
        pipe: original.pipe.clone(),
        hangers_mm: original
            .hangers_mm
            .iter()
            .copied()
            .filter(|&h| h < s_mm)
            .collect(),
        start_joint: original.start_joint,
        end_joint: None,
    };
    let second = Route {
        id: unique(&original.id, |c| {
            edited.system.routes.iter().any(|r| r.id == c)
        }),
        from: port(&id, "out"),
        to: original.to.clone(),
        via_mm: via_b.to_vec(),
        bend_radius_mm: (split..original.via_mm.len()).map(radius).collect(),
        pipe: original.pipe.clone(),
        hangers_mm: original
            .hangers_mm
            .iter()
            .filter(|&&h| h > resume)
            .map(|h| h - resume)
            .collect(),
        start_joint: None,
        end_joint: original.end_joint,
    };
    edited.system.routes[index] = first;
    edited.system.routes.insert(index + 1, second);
    edited.system.elements.push(element);
    edited.validate()?;
    *project = edited;
    Ok(id)
}

/// Takes element `id` out: the route into its `in` and the route from its `out` become one
/// route through the element's port positions, with the first route's pipe. Only elements
/// with one inlet and one outlet can go; replace others instead.
pub fn remove(project: &mut Project, id: &str) -> Result<()> {
    let mut edited = project.clone();
    let element = edited
        .element(id)
        .ok_or_else(|| Error::invalid(format!("no element '{id}'")))?
        .clone();
    if element.kind.port_names() != ["in", "out"] {
        return Err(Error::invalid(format!(
            "only an element with one inlet and one outlet can be removed; '{id}' has {}",
            element.kind.port_names().join(", ")
        )));
    }
    let (pin, pout) = (port(id, "in"), port(id, "out"));
    let touching = |p: &PortRef| {
        edited
            .system
            .routes
            .iter()
            .position(|r| &r.from == p || &r.to == p)
            .ok_or_else(|| {
                Error::invalid(format!("port {}.{} is not connected", p.element, p.port))
            })
    };
    let (a, b) = (touching(&pin)?, touching(&pout)?);
    let length = |r: &Route| -> Result<f64> {
        Ok(centreline(&edited.route_points(r)?, &edited.route_radii(r))?.length * 1e3)
    };
    let (length_a, length_b) = (
        length(&edited.system.routes[a])?,
        length(&edited.system.routes[b])?,
    );
    // Orient the first route to end at `in` and the second to start at `out`.
    let into = oriented(&edited.system.routes[a], length_a, |r| r.to == pin);
    let from = oriented(&edited.system.routes[b], length_b, |r| r.from == pout);
    let (p_in, p_out) = (
        element.port_position("in").expect("two-port element"),
        element.port_position("out").expect("two-port element"),
    );
    let offset = length_a + norm(sub(p_out, p_in));
    let mut via = into.via_mm.clone();
    let mut radii = padded(&into);
    for p in [p_in, p_out] {
        if via.last().is_none_or(|q| norm(sub(p, *q)) > 1.0) {
            via.push(p);
            radii.push(None);
        }
    }
    if from
        .via_mm
        .first()
        .is_some_and(|q| norm(sub(*q, *via.last().unwrap())) <= 1.0)
    {
        via.pop();
        radii.pop();
    }
    via.extend(&from.via_mm);
    radii.extend(padded(&from));
    let joined = Route {
        id: into.id.clone(),
        from: into.from.clone(),
        to: from.to.clone(),
        via_mm: via,
        bend_radius_mm: radii,
        pipe: into.pipe.clone(),
        hangers_mm: into
            .hangers_mm
            .iter()
            .copied()
            .chain(from.hangers_mm.iter().map(|h| h + offset))
            .collect(),
        start_joint: into.start_joint,
        end_joint: from.end_joint,
    };
    edited.system.routes[a] = joined;
    edited.system.routes.remove(b);
    edited.system.elements.retain(|e| e.id != id);
    edited.basis.remove(&format!("system.elements.{id}"));
    edited.validate()?;
    *project = edited;
    Ok(())
}

fn port(element: &str, port: &str) -> PortRef {
    PortRef {
        element: element.into(),
        port: port.into(),
    }
}

/// `route` (centreline `length_mm` long), reversed unless `good` already holds.
fn oriented(route: &Route, length_mm: f64, good: impl Fn(&Route) -> bool) -> Route {
    if good(route) {
        return route.clone();
    }
    let mut r = route.clone();
    std::mem::swap(&mut r.from, &mut r.to);
    std::mem::swap(&mut r.start_joint, &mut r.end_joint);
    r.via_mm.reverse();
    let mut radii = padded(route);
    radii.reverse();
    r.bend_radius_mm = radii;
    r.hangers_mm = route
        .hangers_mm
        .iter()
        .rev()
        .map(|h| length_mm - h)
        .collect();
    r
}

/// Bend radii with one entry per via point.
fn padded(route: &Route) -> Vec<Option<f64>> {
    (0..route.via_mm.len())
        .map(|i| route.bend_radius_mm.get(i).copied().flatten())
        .collect()
}

/// `base` or `base-2`, `base-3`, … whichever is free.
fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken(c))
        .expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vec3;
    use crate::validation::grid::STRAIGHT_PIPE;

    #[test]
    fn insert_then_remove_restores_the_route() {
        let mut project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let before = project.clone();
        // Mid-pipe runs 2.6 m straight to the tip: a chamber 1 m in.
        let id = insert(&mut project, "mid-pipe", 1000.0, "expansion_chamber").unwrap();
        assert_eq!(project.system.routes.len(), before.system.routes.len() + 1);
        let el = project.element(&id).unwrap();
        let start = before.route_points(&before.system.routes[1]).unwrap()[0];
        assert!((norm(sub(scale(start, 1e3), el.position_mm)) - 1000.0).abs() < 1e-6);
        assert!(
            insert(&mut project.clone(), "downpipe", 160.0, "valve").is_err(),
            "in the bend"
        );
        remove(&mut project, &id).unwrap();
        assert_eq!(project.system.routes.len(), before.system.routes.len());
        assert!(project.element(&id).is_none());
        let joined = project.route_points(&project.system.routes[1]).unwrap();
        let length = |p: &[Vec3]| p.windows(2).map(|w| norm(sub(w[1], w[0]))).sum::<f64>();
        assert!(
            (length(&joined) - length(&before.route_points(&before.system.routes[1]).unwrap()))
                .abs()
                < 1e-9
        );
    }
}
