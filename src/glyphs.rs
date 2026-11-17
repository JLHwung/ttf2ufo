//! Glyph outlines and per-glyph TrueType data: extractor's
//! extractOpenTypeGlyphs, extractGlyphPrograms and _extractCompositeFlags.

use std::fmt::Write as _;

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

    let mut glyphs: Vec<Glyph> = Vec::with_capacity(num);
    let mut programs: Vec<Option<Vec<u8>>> = Vec::with_capacity(num);
    for (gid, name) in names.iter().enumerate() {
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
                let scale_flags = CompositeGlyphFlags::WE_HAVE_A_SCALE
                    | CompositeGlyphFlags::WE_HAVE_AN_X_AND_Y_SCALE
                    | CompositeGlyphFlags::WE_HAVE_A_TWO_BY_TWO;
                let mut object_libs = Dict::new();
                let mut overlap = false;
                for (ci, c) in g.components().enumerate() {
                    let base = names
                        .get(c.glyph.to_u32() as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("glyph{:05}", c.glyph.to_u32()));
                    let TtAnchor::Offset { x, y } = c.anchor else {
                        return Err(
                            format!("{name}: point-matched components are not supported").into(),
                        );
                    };
                    let f = |v: read_fonts::types::F2Dot14| Num::Float(v.to_f64());
                    let t = &c.transform;
                    // fontTools' (xx, xy, yx, yy); read-fonts names the 2x2 skews yx, xy
                    let transform = if c.flags.contains(CompositeGlyphFlags::WE_HAVE_A_TWO_BY_TWO) {
                        [f(t.xx), f(t.yx), f(t.xy), f(t.yy)]
                    } else if c.flags.intersects(scale_flags) {
                        [f(t.xx), Num::Int(0), Num::Int(0), f(t.yy)]
                    } else {
                        [Num::Int(1), Num::Int(0), Num::Int(0), Num::Int(1)]
                    };
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
                        transform: [
                            transform[0],
                            transform[1],
                            transform[2],
                            transform[3],
                            Num::Int(x as i64),
                            Num::Int(y as i64),
                        ],
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
                if let Some(y_max) = control_y_max(&glyf, &loca, id, 0)? {
                    glyph
                        .lib
                        .insert("public.verticalOrigin".into(), Value::Int(tsb + y_max));
                }
            }
        }
        glyphs.push(glyph);
        programs.push(program);
    }

    // extractGlyphPrograms: needs every glyph's outline for the component hash
    let index: std::collections::HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    for gid in 0..num {
        let Some(program) = &programs[gid] else {
            continue;
        };
        let mut hash_data = format!("w{}", glyphs[gid].width);
        hash_glyph(&glyphs, &index, gid, true, &mut hash_data)?;
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
        glyphs[gid].lib.insert(
            "public.truetype.instructions".into(),
            Value::Dict(instructions),
        );
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
    out: &mut String,
) -> Result<(), Error> {
    let glyph = &glyphs[gid];
    for contour in &glyph.contours {
        for p in contour {
            let tag = p.typ.map_or('o', |t| t.chars().next().unwrap());
            write!(out, "{tag}{}{:+}", p.x, p.y)?;
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
        hash_glyph(glyphs, index, base, false, out)?;
        write!(out, "({tr})]")?;
    }
    Ok(())
}

/// yMax of a glyph's control points (ControlBoundsPen), components included.
fn control_y_max(
    glyf: &read_fonts::tables::glyf::Glyf,
    loca: &read_fonts::tables::loca::Loca,
    id: GlyphId,
    depth: usize,
) -> Result<Option<i64>, Error> {
    if depth > 64 {
        return Err("component nesting too deep".into());
    }
    Ok(match loca.get_glyf(id, glyf)? {
        None => None,
        Some(TtGlyph::Simple(g)) => g.points().map(|p| p.y as i64).max(),
        Some(TtGlyph::Composite(g)) => {
            let mut y_max: Option<i64> = None;
            for c in g.components() {
                let TtAnchor::Offset { y, .. } = c.anchor else {
                    return Err("point-matched components are not supported".into());
                };
                if c.flags.intersects(
                    CompositeGlyphFlags::WE_HAVE_A_SCALE
                        | CompositeGlyphFlags::WE_HAVE_AN_X_AND_Y_SCALE
                        | CompositeGlyphFlags::WE_HAVE_A_TWO_BY_TWO,
                ) {
                    return Err("vertical origin of scaled components is not supported".into());
                }
                if let Some(m) = control_y_max(glyf, loca, c.glyph, depth + 1)? {
                    let m = m + y as i64;
                    y_max = Some(y_max.map_or(m, |v| v.max(m)));
                }
            }
            y_max
        }
    })
}
