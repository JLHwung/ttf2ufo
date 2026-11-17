//! fontinfo.plist and the info-derived lib keys: extractor's
//! extractOpenTypeInfo, filtered through ufoLib's validators the way
//! extractor's RelaxedInfo does (invalid values are silently dropped).

use read_fonts::tables::name::NameRecord;
use read_fonts::types::Tag;
use read_fonts::{FontRef, TableProvider};

use crate::Error;
use crate::plist::{Dict, Value};

/// Mac Roman 0x80..=0xFF, from Python's mac_roman codec.
const MAC_ROMAN_HIGH: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4, 0x00E3,
    0x00E5, 0x00E7, 0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3,
    0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020, 0x00B0, 0x00A2, 0x00A3,
    0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8,
    0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211, 0x220F, 0x03C0, 0x222B, 0x00AA,
    0x00BA, 0x03A9, 0x00E6, 0x00F8, 0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB,
    0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D,
    0x2018, 0x2019, 0x00F7, 0x25CA, 0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02,
    0x2021, 0x00B7, 0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE,
    0x00CF, 0x00CC, 0x00D3, 0x00D4, 0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC,
    0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
];

fn is_ascii_text(c: u32) -> bool {
    (0x20..=0x7E).contains(&c) || matches!(c, 0x09 | 0x0A | 0x0D)
}

/// fontTools' NameRecord.toUnicode(), for the encodings it maps to
/// utf_16_be, mac_roman, ascii and latin1.
fn decode_name(platform: u16, encoding: u16, language: u16, bytes: &[u8]) -> Result<String, Error> {
    let codec = match (platform, encoding) {
        (0, 0..=6) => "utf_16_be",
        (3, 0 | 1 | 10) => "utf_16_be",
        (2, 1) => "utf_16_be",
        (2, 0) => "ascii",
        (2, 2) => "latin1",
        (1, 0) => match language {
            15 | 17 | 18 | 24..=28 | 36..=40 => {
                return Err(format!("name records in Mac encoding for language {language} are not supported").into());
            }
            _ => "mac_roman",
        },
        (1, 1 | 2 | 3 | 6 | 7 | 25 | 29 | 35 | 37) | (3, 2..=6) => {
            return Err(format!("name records in encoding ({platform}, {encoding}) are not supported").into());
        }
        _ => "ascii",
    };
    let mut bytes = bytes.to_vec();
    if codec == "utf_16_be" && bytes.len() % 2 == 1 {
        // fontTools' recovery of odd-length UTF-16
        if bytes.last() == Some(&0) {
            bytes.pop();
        } else if bytes.iter().enumerate().all(|(i, b)| if i % 2 == 1 { *b == 0 } else { is_ascii_text(*b as u32) }) {
            bytes.insert(0, 0);
        } else if bytes.first() == Some(&0) && bytes[1..].iter().all(|b| is_ascii_text(*b as u32)) {
            bytes = bytes[1..].iter().flat_map(|b| [0, *b]).collect();
        }
    }
    let string: String = match codec {
        "utf_16_be" => {
            if bytes.len() % 2 == 1 {
                return Err("undecodable UTF-16 name record".into());
            }
            let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16(&units).map_err(|_| "undecodable UTF-16 name record")?
        }
        "mac_roman" => bytes
            .iter()
            .map(|&b| if b < 0x80 { b as char } else { char::from_u32(MAC_ROMAN_HIGH[b as usize - 0x80] as u32).unwrap() })
            .collect(),
        "latin1" => bytes.iter().map(|&b| b as char).collect(),
        _ => {
            if bytes.iter().any(|b| *b >= 0x80) {
                return Err("undecodable ASCII name record".into());
            }
            bytes.iter().map(|&b| b as char).collect()
        }
    };
    // a decoded string that still looks like UTF-16BE ASCII is narrowed down
    let chars: Vec<char> = string.chars().collect();
    if chars.iter().enumerate().all(|(i, c)| if i % 2 == 0 { *c as u32 == 0 } else { is_ascii_text(*c as u32) }) {
        return Ok(chars.iter().skip(1).step_by(2).collect());
    }
    Ok(string)
}

fn bits(value: u64, start: i64) -> Vec<i64> {
    (0..64).filter(|i| value >> i & 1 == 1).map(|i| i + start).collect()
}

fn int_list(values: &[i64]) -> Value {
    Value::Array(values.iter().map(|v| Value::Int(*v)).collect())
}

/// `time.strftime("%Y/%m/%d %H:%M:%S", time.gmtime(max(0, date + mac_epoch_diff)))`
/// for a `head` timestamp as fontTools decodes it.
fn format_date(raw: i64) -> String {
    let mut value = raw;
    if value > 0xFFFF_FFFF {
        value &= 0xFFFF_FFFF;
    }
    if value < 0x7C25_9DC0 {
        value += 0x7C25_9DC0;
    }
    let secs = (value - 2_082_844_800).max(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Python's `f"{round(x, 6):.3f}"`.
fn version_string(x: f64) -> String {
    let rounded: f64 = format!("{x:.6}").parse().unwrap();
    format!("{rounded:.3}")
}

/// (fontinfo.plist, lib keys) for the font.
pub fn extract_info(font: &FontRef) -> Result<(Dict, Dict), Error> {
    let mut info = Dict::new();
    let mut lib = Dict::new();
    let set = |info: &mut Dict, key: &str, value: Value| {
        info.insert(key.to_string(), value);
    };

    // head
    let head = font.head()?;
    let version = version_string(head.font_revision().to_f64());
    let (major, minor) = version.split_once('.').unwrap();
    set(&mut info, "versionMajor", Value::Int(major.parse()?));
    let minor: i64 = minor.parse()?;
    if minor >= 0 {
        set(&mut info, "versionMinor", Value::Int(minor));
    }
    set(&mut info, "unitsPerEm", Value::Int(head.units_per_em() as i64));
    set(&mut info, "openTypeHeadCreated", format_date(head.created().as_secs()).into());
    set(&mut info, "openTypeHeadLowestRecPPEM", Value::Int(head.lowest_rec_ppem() as i64));
    let flags = bits(head.flags().bits() as u64, 0);
    if flags.iter().all(|b| *b <= 14) {
        set(&mut info, "openTypeHeadFlags", int_list(&flags));
    }
    let mac_style = bits(head.mac_style().bits() as u64, 0);
    let style = match (mac_style.contains(&0), mac_style.contains(&1)) {
        (true, true) => "bold italic",
        (true, false) => "bold",
        (false, true) => "italic",
        _ => "regular",
    };
    set(&mut info, "styleMapStyleName", style.into());

    // name
    let name = font.name()?;
    let data = name.string_data();
    let mut records: Vec<(u16, u16, u16, u16, String)> = Vec::new();
    // nameIDs dict: insertion order of first occurrence, value of the last
    let mut name_ids: indexmap::IndexMap<(u16, u16, u16, u16), String> = indexmap::IndexMap::new();
    for record in name.name_record() {
        let r: &NameRecord = record;
        let start = r.string_offset().to_u32() as usize;
        let raw = data.as_bytes().get(start..start + r.length() as usize).ok_or("bad name record")?.to_vec();
        let s = decode_name(r.platform_id(), r.encoding_id(), r.language_id(), &raw)?;
        let key = (r.name_id().to_u16(), r.platform_id(), r.encoding_id(), r.language_id());
        name_ids.insert(key, s.clone());
        records.push((key.0, key.1, key.2, key.3, s));
    }
    type Priority = (u16, Option<u16>, Option<u16>, Option<u16>);
    let skim = |priority: &[Priority]| -> Option<String> {
        for &(nid, pid, eid, lid) in priority {
            for ((n, p, e, l), text) in &name_ids {
                if *n != nid || pid.is_some_and(|v| v != *p) || eid.is_some_and(|v| v != *e) || lid.is_some_and(|v| v != *l) {
                    continue;
                }
                return Some(text.clone());
            }
        }
        None
    };
    let prio = |id: u16| vec![(id, Some(1), Some(0), Some(0)), (id, Some(1), None, None), (id, None, None, None)];
    let attributes: [(&str, Vec<u16>); 23] = [
        ("familyName", vec![16, 1]),
        ("styleName", vec![17, 2]),
        ("styleMapFamilyName", vec![1]),
        ("copyright", vec![0]),
        ("postscriptFontName", vec![6]),
        ("trademark", vec![7]),
        ("openTypeNameDesigner", vec![9]),
        ("openTypeNameDesignerURL", vec![12]),
        ("openTypeNameManufacturer", vec![8]),
        ("openTypeNameManufacturerURL", vec![11]),
        ("openTypeNameLicense", vec![13]),
        ("openTypeNameLicenseURL", vec![14]),
        ("openTypeNameVersion", vec![5]),
        ("openTypeNameUniqueID", vec![3]),
        ("openTypeNameDescription", vec![10]),
        ("openTypeNamePreferredFamilyName", vec![16]),
        ("openTypeNamePreferredSubfamilyName", vec![17]),
        ("openTypeNameCompatibleFullName", vec![18]),
        ("openTypeNameSampleText", vec![20]),
        ("openTypeNameWWSFamilyName", vec![21]),
        ("openTypeNameWWSSubfamilyName", vec![22]),
        ("", vec![]),
        ("", vec![]),
    ];
    for (attr, ids) in attributes.iter().filter(|(a, _)| !a.is_empty()) {
        let priority: Vec<_> = ids.iter().flat_map(|id| prio(*id)).collect();
        if let Some(v) = skim(&priority) {
            set(&mut info, attr, v.into());
        }
    }
    records.sort();
    let name_records: Vec<Value> = records
        .into_iter()
        .map(|(nid, pid, eid, lid, s)| {
            let mut d = Dict::new();
            d.insert("nameID".into(), Value::Int(nid as i64));
            d.insert("platformID".into(), Value::Int(pid as i64));
            d.insert("encodingID".into(), Value::Int(eid as i64));
            d.insert("languageID".into(), Value::Int(lid as i64));
            d.insert("string".into(), s.into());
            Value::Dict(d)
        })
        .collect();
    set(&mut info, "openTypeNameRecords", Value::Array(name_records));

    // OS/2
    if let Ok(os2) = font.os2() {
        let width = os2.us_width_class() as i64;
        if (1..=9).contains(&width) {
            set(&mut info, "openTypeOS2WidthClass", Value::Int(width));
        }
        set(&mut info, "openTypeOS2WeightClass", Value::Int(os2.us_weight_class() as i64));
        let selection: Vec<i64> =
            bits(os2.fs_selection().bits() as u64, 0).into_iter().filter(|b| (1..=4).contains(b)).collect();
        set(&mut info, "openTypeOS2Selection", int_list(&selection));
        let vendor_bytes = os2.ach_vend_id().to_be_bytes();
        if !vendor_bytes.is_ascii() {
            return Err("non-ASCII OS/2 vendor IDs are not supported".into());
        }
        let vendor: String = vendor_bytes.iter().map(|&b| b as char).collect();
        set(&mut info, "openTypeOS2VendorID", vendor.trim_end_matches('\0').to_string().into());
        let p = os2.panose_10();
        set(&mut info, "openTypeOS2Panose", int_list(&p.iter().map(|v| *v as i64).collect::<Vec<_>>()));
        let ranges = bits(os2.ul_unicode_range_1() as u64, 0)
            .into_iter()
            .chain(bits(os2.ul_unicode_range_2() as u64, 32))
            .chain(bits(os2.ul_unicode_range_3() as u64, 64))
            .chain(bits(os2.ul_unicode_range_4() as u64, 96))
            .collect::<Vec<_>>();
        set(&mut info, "openTypeOS2UnicodeRanges", int_list(&ranges));
        if let (Some(c1), Some(c2)) = (os2.ul_code_page_range_1(), os2.ul_code_page_range_2()) {
            let pages = bits(c1 as u64, 0).into_iter().chain(bits(c2 as u64, 32)).collect::<Vec<_>>();
            set(&mut info, "openTypeOS2CodePageRanges", int_list(&pages));
        }
        if let Some(v) = os2.sx_height() {
            set(&mut info, "xHeight", Value::Int(v as i64));
        }
        if let Some(v) = os2.s_cap_height() {
            set(&mut info, "capHeight", Value::Int(v as i64));
        }
        let typo_asc = os2.s_typo_ascender() as i64;
        let typo_desc = os2.s_typo_descender() as i64;
        set(&mut info, "ascender", Value::Int(typo_asc));
        set(&mut info, "descender", Value::Int(typo_desc));
        set(&mut info, "openTypeOS2TypoAscender", Value::Int(typo_asc));
        set(&mut info, "openTypeOS2TypoDescender", Value::Int(typo_desc));
        set(&mut info, "openTypeOS2TypoLineGap", Value::Int(os2.s_typo_line_gap() as i64));
        set(&mut info, "openTypeOS2WinAscent", Value::Int(os2.us_win_ascent() as i64));
        set(&mut info, "openTypeOS2WinDescent", Value::Int(os2.us_win_descent() as i64));
        let fs_type = bits(os2.fs_type() as u64, 0);
        if fs_type.iter().all(|b| matches!(b, 0 | 1 | 2 | 3 | 8 | 9)) {
            set(&mut info, "openTypeOS2Type", int_list(&fs_type));
        }
        for (key, v) in [
            ("openTypeOS2SubscriptXSize", os2.y_subscript_x_size()),
            ("openTypeOS2SubscriptYSize", os2.y_subscript_y_size()),
            ("openTypeOS2SubscriptXOffset", os2.y_subscript_x_offset()),
            ("openTypeOS2SubscriptYOffset", os2.y_subscript_y_offset()),
            ("openTypeOS2SuperscriptXSize", os2.y_superscript_x_size()),
            ("openTypeOS2SuperscriptYSize", os2.y_superscript_y_size()),
            ("openTypeOS2SuperscriptXOffset", os2.y_superscript_x_offset()),
            ("openTypeOS2SuperscriptYOffset", os2.y_superscript_y_offset()),
            ("openTypeOS2StrikeoutSize", os2.y_strikeout_size()),
            ("openTypeOS2StrikeoutPosition", os2.y_strikeout_position()),
        ] {
            set(&mut info, key, Value::Int(v as i64));
        }
    } else {
        return Err("fonts without an 'OS/2' table are not supported".into());
    }

    // hhea
    if let Ok(hhea) = font.hhea() {
        set(&mut info, "openTypeHheaAscender", Value::Int(hhea.ascender().to_i16() as i64));
        set(&mut info, "openTypeHheaDescender", Value::Int(hhea.descender().to_i16() as i64));
        set(&mut info, "openTypeHheaLineGap", Value::Int(hhea.line_gap().to_i16() as i64));
        set(&mut info, "openTypeHheaCaretSlopeRise", Value::Int(hhea.caret_slope_rise() as i64));
        set(&mut info, "openTypeHheaCaretSlopeRun", Value::Int(hhea.caret_slope_run() as i64));
        set(&mut info, "openTypeHheaCaretOffset", Value::Int(hhea.caret_offset() as i64));
    }
    // vhea
    if let Ok(vhea) = font.vhea() {
        set(&mut info, "openTypeVheaVertTypoAscender", Value::Int(vhea.ascender().to_i16() as i64));
        set(&mut info, "openTypeVheaVertTypoDescender", Value::Int(vhea.descender().to_i16() as i64));
        set(&mut info, "openTypeVheaVertTypoLineGap", Value::Int(vhea.line_gap().to_i16() as i64));
        set(&mut info, "openTypeVheaCaretSlopeRise", Value::Int(vhea.caret_slope_rise() as i64));
        set(&mut info, "openTypeVheaCaretSlopeRun", Value::Int(vhea.caret_slope_run() as i64));
        set(&mut info, "openTypeVheaCaretOffset", Value::Int(vhea.caret_offset() as i64));
    }
    // post
    let post = font.post()?;
    set(&mut info, "italicAngle", Value::Real(post.italic_angle().to_f64()));
    set(&mut info, "postscriptUnderlineThickness", Value::Int(post.underline_thickness().to_i16() as i64));
    set(&mut info, "postscriptUnderlinePosition", Value::Int(post.underline_position().to_i16() as i64));
    set(&mut info, "postscriptIsFixedPitch", Value::Bool(post.is_fixed_pitch() != 0));

    // gasp
    if let Some(data) = font.table_data(Tag::new(b"gasp")) {
        let d = data.as_bytes();
        let n = u16::from_be_bytes([d[2], d[3]]) as usize;
        let mut ranges: Vec<(i64, u16)> = (0..n)
            .map(|i| {
                let at = 4 + 4 * i;
                (u16::from_be_bytes([d[at], d[at + 1]]) as i64, u16::from_be_bytes([d[at + 2], d[at + 3]]))
            })
            .collect();
        // fontTools keeps a dict (last record per size wins), then sorts
        let mut by_size = std::collections::BTreeMap::new();
        for (size, behavior) in ranges.drain(..) {
            by_size.insert(size, behavior);
        }
        let records: Vec<Value> = by_size
            .into_iter()
            .map(|(size, b)| {
                let mut d = Dict::new();
                d.insert("rangeMaxPPEM".into(), Value::Int(size));
                d.insert("rangeGaspBehavior".into(), int_list(&bits((b & 0xF) as u64, 0)));
                Value::Dict(d)
            })
            .collect();
        set(&mut info, "openTypeGaspRangeRecords", Value::Array(records));
    }

    // _extractLibInfo
    lib.insert("public.openTypeHeadModified".into(), format_date(head.modified().as_secs()).into());
    lib.insert("public.openTypePostUnderlinePosition".into(), Value::Int(post.underline_position().to_i16() as i64));
    if font.table_data(Tag::new(b"CFF ")).is_some() {
        return Err("CFF-flavored fonts are not supported".into());
    }
    Ok((info, lib))
}
