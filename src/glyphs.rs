//! Glyph outlines and per-glyph TrueType data: extractor's
//! extractOpenTypeGlyphs, extractGlyphPrograms and _extractCompositeFlags.

use std::fmt::Write as _;

use rayon::prelude::*;
use read_fonts::tables::glyf::{Anchor as TtAnchor, CompositeGlyphFlags, Glyph as TtGlyph};
use read_fonts::types::GlyphId;
use read_fonts::{FontRef, TableProvider};
use sha2::{Digest, Sha512};

use crate::Error;
use crate::instructions::lib_assembly;
use crate::plist::{Dict, Value};
use crate::pyfmt::{Num, float_repr_signed};
use crate::ufo::{Component, Glyph, Point};

/// Python's `f"{value:+}"` for an int or float.
fn signed(n: Num) -> String {
    match n {
        Num::Int(i) => format!("{i:+}"),
        Num::Float(f) => float_repr_signed(f),
    }
}

/// fontTools' otRound.
fn ot_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// fontTools' `floatToFixedToFloat(value, precisionBits=14)`.
fn f2dot14_round(v: f64) -> f64 {
    ot_round(v * 16384.0) as f64 / 16384.0
}

/// Read every glyph's outline, width, components, and TrueType program.
pub fn extract_glyphs(
    font: &FontRef,
    names: &[String],
    unicodes: &[Vec<u32>],
) -> Result<Vec<Glyph>, Error> {
    let glyf = font.glyf()?;
    let loca = font.loca(None)?;
    let hmtx = font.hmtx()?;
    let vmtx = font.vmtx().ok();
    let vorg = font.vorg().ok();
    let num = names.len();

    // Glyphs are independent of each other here, so read them in parallel.
    let read_glyph = |gid: usize| -> Result<(Glyph, Option<Vec<u8>>), Error> {
        let name = &names[gid];
        let id = GlyphId::new(gid as u32);
        let width = hmtx.advance(id).unwrap_or(0) as i64;
        let lsb = hmtx.side_bearing(id).unwrap_or(0) as i64;
        let mut glyph = Glyph::new(name.clone(), width);
        glyph.unicodes = unicodes[gid].clone();
        let mut program = None;
        match loca.get_glyf(id, &glyf)? {
            None => {}
            Some(TtGlyph::Simple(g)) => {
                program = Some(g.instructions().to_vec());
                // fontTools' glyph set shifts x by (hmtx lsb - stored xMin)
                let offset = lsb - g.x_min() as i64;
                let pts: Vec<_> = g.points().collect();
                let mut start = 0usize;
                for end in g.end_pts_of_contours() {
                    let end = end.get() as usize + 1;
                    let contour = pts.get(start..end).ok_or("bad glyf contour")?;
                    start = end;
                    let last = contour.last().ok_or("empty glyf contour")?;
                    let mut seg = if last.on_curve {
                        "line"
                    } else if last.cubic {
                        "curve"
                    } else {
                        "qcurve"
                    };
                    let mut points = Vec::with_capacity(contour.len());
                    for p in contour {
                        let typ = if p.on_curve {
                            let t = seg;
                            seg = "line";
                            Some(t)
                        } else {
                            seg = if p.cubic { "curve" } else { "qcurve" };
                            None
                        };
                        points.push(Point {
                            x: p.x as i64 + offset,
                            y: p.y as i64,
                            typ,
                        });
                    }
                    glyph.contours.push(points);
                }
            }
            Some(TtGlyph::Composite(g)) => {
                program = g.instructions().map(|i| i.to_vec());
                let mut object_libs = Dict::new();
                let mut overlap = false;
                for (ci, c) in g.components().enumerate() {
                    let base = names
                        .get(c.glyph.to_u32() as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("glyph{:05}", c.glyph.to_u32()));
                    let transform = component_info(&c).map_err(|e| format!("{name}: {e}"))?;
                    let identifier = format!("component{ci}");
                    let mut flags = Dict::new();
                    flags.insert(
                        "public.truetype.roundOffsetToGrid".into(),
                        c.flags
                            .contains(CompositeGlyphFlags::ROUND_XY_TO_GRID)
                            .into(),
                    );
                    flags.insert(
                        "public.truetype.useMyMetrics".into(),
                        c.flags.contains(CompositeGlyphFlags::USE_MY_METRICS).into(),
                    );
                    object_libs.insert(identifier.clone(), Value::Dict(flags));
                    overlap = c.flags.contains(CompositeGlyphFlags::OVERLAP_COMPOUND);
                    glyph.components.push(Component {
                        base,
                        transform,
                        identifier: Some(identifier),
                    });
                }
                glyph
                    .lib
                    .insert("public.objectLibs".into(), Value::Dict(object_libs));
                if !glyph.components.is_empty() {
                    glyph
                        .lib
                        .insert("public.truetype.overlap".into(), overlap.into());
                }
            }
        }

        // vertical metrics
        if let Some(vmtx) = &vmtx
            && let Some(height) = vmtx.advance(id)
        {
            glyph.height = height as i64;
            if let Some(vorg) = &vorg {
                glyph.lib.insert(
                    "public.verticalOrigin".into(),
                    (vorg.vertical_origin_y(id) as i64).into(),
                );
            } else {
                let tsb = vmtx.side_bearing(id).unwrap_or(0) as i64;
                let mut y_max = None;
                control_y_max(&glyf, &loca, num, id, None, 0, &mut y_max)?;
                if let Some(y_max) = y_max {
                    let origin = match add(Num::Int(tsb), y_max) {
                        Num::Int(i) => Value::Int(i),
                        Num::Float(f) => Value::Real(f),
                    };
                    glyph.lib.insert("public.verticalOrigin".into(), origin);
                }
            }
        }
        Ok((glyph, program))
    };
    let (mut glyphs, programs): (Vec<Glyph>, Vec<Option<Vec<u8>>>) = (0..num)
        .into_par_iter()
        .map(read_glyph)
        .collect::<Result<Vec<_>, Error>>()?
        .into_iter()
        .unzip();

    // extractGlyphPrograms: needs every glyph's outline for the component hash
    let index: std::collections::HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let instructions: Vec<(usize, Dict)> = (0..num)
        .into_par_iter()
        .filter_map(|gid| programs[gid].as_ref().map(|p| (gid, p)))
        .map(|(gid, program)| -> Result<(usize, Dict), Error> {
            let mut hash_data = format!("w{}", glyphs[gid].width);
            hash_glyph(&glyphs, &index, gid, true, &mut Vec::new(), &mut hash_data)?;
            let id = if hash_data.len() >= 128 {
                Sha512::digest(hash_data.as_bytes()).iter().fold(
                    String::with_capacity(128),
                    |mut h, b| {
                        let _ = write!(h, "{b:02x}");
                        h
                    },
                )
            } else {
                hash_data
            };
            let mut instructions = Dict::new();
            instructions.insert("formatVersion".into(), "1".into());
            instructions.insert("id".into(), id.into());
            instructions.insert("assembly".into(), lib_assembly(program)?.into());
            Ok((gid, instructions))
        })
        .collect::<Result<_, Error>>()?;
    for (gid, dict) in instructions {
        glyphs[gid]
            .lib
            .insert("public.truetype.instructions".into(), Value::Dict(dict));
    }
    Ok(glyphs)
}

/// HashPointPen pre-image of a glyph, drawn as extractor does: through a
/// RoundingPointPen at the top level (`rounding`), directly for nested
/// components.
fn hash_glyph(
    glyphs: &[Glyph],
    index: &std::collections::HashMap<&str, usize>,
    gid: usize,
    rounding: bool,
    stack: &mut Vec<usize>,
    out: &mut String,
) -> Result<(), Error> {
    let glyph = &glyphs[gid];
    // hot path: f"{tag}{x:g}{y:+g}" written by hand (coordinates are ints)
    let mut num = itoa::Buffer::new();
    for contour in &glyph.contours {
        for p in contour {
            out.push(p.typ.map_or('o', |t| t.as_bytes()[0] as char));
            out.push_str(num.format(p.x));
            if p.y >= 0 {
                out.push('+');
            }
            out.push_str(num.format(p.y));
        }
        out.push('|');
    }
    for c in &glyph.components {
        let tr: String = if rounding {
            let mut s = String::new();
            for v in &c.transform[..4] {
                s.push_str(&float_repr_signed(f2dot14_round(v.as_f64())));
            }
            for v in &c.transform[4..] {
                write!(s, "{:+}", ot_round(v.as_f64()))?;
            }
            s
        } else {
            c.transform.iter().map(|v| signed(*v)).collect()
        };
        out.push('[');
        let base = *index
            .get(c.base.as_str())
            .ok_or_else(|| format!("missing component base glyph {}", c.base))?;
        if stack.contains(&base) || stack.len() > 1000 {
            // Python recurses until RecursionError
            return Err(format!("component cycle through glyph {}", c.base).into());
        }
        stack.push(base);
        hash_glyph(glyphs, index, base, false, stack, out)?;
        stack.pop();
        write!(out, "({tr})]")?;
    }
    Ok(())
}

/// fontTools' `GlyphComponent.getComponentInfo()` transform:
/// (xx, xy, yx, yy, dx, dy), with ints where fontTools has ints.
fn component_info(c: &read_fonts::tables::glyf::Component) -> Result<[Num; 6], Error> {
    let TtAnchor::Offset { x, y } = c.anchor else {
        return Err("point-matched components are not supported".into());
    };
    let f = |v: read_fonts::types::F2Dot14| Num::Float(v.to_f64());
    let t = &c.transform;
    let scale_flags = CompositeGlyphFlags::WE_HAVE_A_SCALE
        | CompositeGlyphFlags::WE_HAVE_AN_X_AND_Y_SCALE
        | CompositeGlyphFlags::WE_HAVE_A_TWO_BY_TWO;
    // fontTools' (xx, xy, yx, yy); read-fonts names the 2x2 skews yx, xy
    let [xx, xy, yx, yy] = if c.flags.contains(CompositeGlyphFlags::WE_HAVE_A_TWO_BY_TWO) {
        [f(t.xx), f(t.yx), f(t.xy), f(t.yy)]
    } else if c.flags.intersects(scale_flags) {
        [f(t.xx), Num::Int(0), Num::Int(0), f(t.yy)]
    } else {
        [Num::Int(1), Num::Int(0), Num::Int(0), Num::Int(1)]
    };
    Ok([xx, xy, yx, yy, Num::Int(x as i64), Num::Int(y as i64)])
}

/// Python `a + b` on ints and floats.
fn add(a: Num, b: Num) -> Num {
    match (a, b) {
        (Num::Int(a), Num::Int(b)) => Num::Int(a + b),
        _ => Num::Float(a.as_f64() + b.as_f64()),
    }
}

/// Python `a * b` on ints and floats.
fn mul(a: Num, b: Num) -> Num {
    match (a, b) {
        (Num::Int(a), Num::Int(b)) => Num::Int(a * b),
        _ => Num::Float(a.as_f64() * b.as_f64()),
    }
}

/// Python `0.5 * (a + b)`, BasePen's implied on-curve point.
fn half_sum(a: Num, b: Num) -> Num {
    Num::Float(0.5 * add(a, b).as_f64())
}

type Pt = (Num, Num);

/// `Glyph.draw`'s cubic implied point: `maybeInt((a + b) * 0.5)` per axis.
fn maybe_int_mid(a: Pt, b: Pt) -> Pt {
    let mid = |u: Num, v: Num| {
        let m = add(u, v).as_f64() * 0.5;
        if m == m.trunc() {
            Num::Int(m as i64)
        } else {
            Num::Float(m)
        }
    };
    (mid(a.0, b.0), mid(a.1, b.1))
}

/// `Transform.transform(other)`: `outer` applied after `inner`.
fn compose(outer: &[Num; 6], inner: &[Num; 6]) -> [Num; 6] {
    let [xx1, xy1, yx1, yy1, dx1, dy1] = *inner;
    let [xx2, xy2, yx2, yy2, dx2, dy2] = *outer;
    [
        add(mul(xx1, xx2), mul(xy1, yx2)),
        add(mul(xx1, xy2), mul(xy1, yy2)),
        add(mul(yx1, xx2), mul(yy1, yx2)),
        add(mul(yx1, xy2), mul(yy1, yy2)),
        add(add(mul(xx2, dx1), mul(yx2, dy1)), dx2),
        add(add(mul(xy2, dx1), mul(yy2, dy1)), dy2),
    ]
}

/// `Transform.transformPoint`, or the point itself without a TransformPen.
fn apply(t: Option<&[Num; 6]>, (x, y): Pt) -> Pt {
    match t {
        None => (x, y),
        Some(&[xx, xy, yx, yy, dx, dy]) => (
            add(add(mul(xx, x), mul(yx, y)), dx),
            add(add(mul(xy, x), mul(yy, y)), dy),
        ),
    }
}

/// `updateBounds` on yMax: Python's `max(yMax, y)` keeps the earlier value
/// on a tie, which decides between an int and an equal float.
fn update(y_max: &mut Option<Num>, y: Num) {
    if y_max.is_none_or(|m| y.as_f64() > m.as_f64()) {
        *y_max = Some(y);
    }
}

/// `BasePen.qCurveTo` via `decomposeQuadraticSegment` into
/// `ControlBoundsPen._qCurveToOne`: off-curve and implied points in order.
fn quad_segments(points: &[Pt], y_max: &mut Option<Num>) {
    let n = points.len() - 1;
    for i in 0..n - 1 {
        update(y_max, points[i].1);
        update(y_max, half_sum(points[i].1, points[i + 1].1));
    }
    update(y_max, points[n - 1].1);
    update(y_max, points[n].1);
}

/// yMax of a glyph's control points as extractor gets it: the glyph drawn
/// (`Glyph.draw`) into a `ControlBoundsPen`, which decomposes components
/// through a `TransformPen` unless their transform is the identity. The
/// result is a Python int or float, as the arithmetic makes it.
fn control_y_max(
    glyf: &read_fonts::tables::glyf::Glyf,
    loca: &read_fonts::tables::loca::Loca,
    num_glyphs: usize,
    id: GlyphId,
    transform: Option<&[Num; 6]>,
    depth: usize,
    y_max: &mut Option<Num>,
) -> Result<(), Error> {
    if depth > 1000 {
        // a component cycle: Python recurses until RecursionError
        return Err("component nesting too deep".into());
    }
    match loca.get_glyf(id, glyf)? {
        None => {}
        Some(TtGlyph::Simple(g)) => {
            let pts: Vec<_> = g.points().collect();
            let tp = |p: &read_fonts::tables::glyf::CurvePoint| {
                apply(transform, (Num::Int(p.x as i64), Num::Int(p.y as i64)))
            };
            let mut start = 0usize;
            for end in g.end_pts_of_contours() {
                let end = end.get() as usize + 1;
                let contour = pts.get(start..end).ok_or("bad glyf contour")?;
                start = end;
                if contour.is_empty() {
                    return Err("empty glyf contour".into());
                }
                let raw = |p: &read_fonts::tables::glyf::CurvePoint| -> Pt {
                    (Num::Int(p.x as i64), Num::Int(p.y as i64))
                };
                match contour.iter().position(|p| p.on_curve) {
                    None if contour.iter().any(|p| p.cubic) => {
                        // all off-curve cubic: implied on-curve points
                        // computed in glyph space, then curveTo(p1, p2, p3)
                        if !contour.iter().all(|p| p.cubic) || contour.len() % 2 != 0 {
                            return Err("malformed cubic glyf contour".into());
                        }
                        let n = contour.len();
                        let (l, f) = (raw(&contour[n - 1]), raw(&contour[0]));
                        update(y_max, apply(transform, maybe_int_mid(l, f)).1);
                        for i in (0..n).step_by(2) {
                            let p2 = raw(&contour[i + 1]);
                            let p4 = raw(&contour[(i + 2) % n]);
                            update(y_max, tp(&contour[i]).1);
                            update(y_max, tp(&contour[i + 1]).1);
                            update(y_max, apply(transform, maybe_int_mid(p2, p4)).1);
                        }
                    }
                    None => {
                        // qCurveTo(*offCurves, None): implied start point first
                        let mut points: Vec<Pt> = contour.iter().map(tp).collect();
                        let (last, first) = (points[points.len() - 1], points[0]);
                        let implied = (half_sum(last.0, first.0), half_sum(last.1, first.1));
                        update(y_max, implied.1);
                        points.push(implied);
                        quad_segments(&points, y_max);
                    }
                    Some(first_on) => {
                        // rotated to end on the first on-curve point, the moveTo
                        let f = first_on + 1;
                        let rotated: Vec<_> = contour[f..].iter().chain(&contour[..f]).collect();
                        update(y_max, tp(rotated[rotated.len() - 1]).1);
                        let mut rest = &rotated[..];
                        while !rest.is_empty() {
                            let next_on = rest.iter().position(|p| p.on_curve).unwrap() + 1;
                            if next_on == 1 {
                                if rest.len() > 1 {
                                    update(y_max, tp(rest[0]).1);
                                }
                            } else if rest[..next_on - 1].iter().any(|p| p.cubic) {
                                let count = next_on;
                                if !rest[..count - 1].iter().all(|p| p.cubic)
                                    || count < 3
                                    || (count - 1) % 2 != 0
                                {
                                    return Err("malformed cubic glyf contour".into());
                                }
                                for i in (0..count - 3).step_by(2) {
                                    update(y_max, tp(rest[i]).1);
                                    update(y_max, tp(rest[i + 1]).1);
                                    let mid = maybe_int_mid(raw(rest[i + 1]), raw(rest[i + 2]));
                                    update(y_max, apply(transform, mid).1);
                                }
                                for p in &rest[count - 3..count] {
                                    update(y_max, tp(p).1);
                                }
                            } else {
                                let seg: Vec<Pt> = rest[..next_on].iter().map(|p| tp(p)).collect();
                                quad_segments(&seg, y_max);
                            }
                            rest = &rest[next_on..];
                        }
                    }
                }
            }
        }
        Some(TtGlyph::Composite(g)) => {
            for c in g.components() {
                let info = component_info(&c)?;
                // DecomposingPen skips components missing from the glyph set
                if c.glyph.to_u32() as usize >= num_glyphs {
                    continue;
                }
                let t = match transform {
                    Some(outer) => compose(outer, &info),
                    None => info,
                };
                let identity = t
                    .iter()
                    .zip([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
                    .all(|(v, d)| v.as_f64() == d);
                let inner = (!identity).then_some(&t);
                control_y_max(glyf, loca, num_glyphs, c.glyph, inner, depth + 1, y_max)?;
            }
        }
    }
    Ok(())
}
