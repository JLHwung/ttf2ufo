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

/// fontTools' NameRecord.toUnicode(), with the codec
/// `fontTools.misc.encodingTools.getEncoding` picks (ascii by default).
fn decode_name(platform: u16, encoding: u16, language: u16, bytes: &[u8]) -> Result<String, Error> {
    let codec = match (platform, encoding) {
        (0, 0..=6) => "utf_16_be",
        (3, 0 | 1 | 10) => "utf_16_be",
        (2, 1) => "utf_16_be",
        (2, 0) => "ascii",
        (2, 2) => "latin1",
        (1, 0) => match language {
            15 => "mac_iceland",
            17 => "mac_turkish",
            18 => "mac_croatian",
            24..=28 | 36 | 38..=40 => "mac_latin2",
            37 => "mac_romanian",
            _ => "mac_roman",
        },
        (1, 1) => "x_mac_japanese_ttx",
        (1, 2) => "x_mac_trad_chinese_ttx",
        (1, 3) => "x_mac_korean_ttx",
        (1, 6) => "mac_greek",
        (1, 7) => "mac_cyrillic",
        (1, 25) => "x_mac_simp_chinese_ttx",
        (1, 29) => "mac_latin2",
        (1, 35) => "mac_turkish",
        (1, 37) => "mac_iceland",
        (3, 2) => "shift_jis",
        (3, 3) => "gb2312",
        (3, 4) => "big5",
        (3, 5) => "euc_kr",
        (3, 6) => "johab",
        _ => "ascii",
    };
    let mut bytes = bytes.to_vec();
    if codec == "utf_16_be" && bytes.len() % 2 == 1 {
        // fontTools' recovery of odd-length UTF-16
        if bytes.last() == Some(&0) {
            bytes.pop();
        } else if bytes.iter().enumerate().all(|(i, b)| {
            if i % 2 == 1 {
                *b == 0
            } else {
                is_ascii_text(*b as u32)
            }
        }) {
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
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16(&units).map_err(|_| "undecodable UTF-16 name record")?
        }
        "mac_roman" => bytes
            .iter()
            .map(|&b| {
                if b < 0x80 {
                    b as char
                } else {
                    char::from_u32(MAC_ROMAN_HIGH[b as usize - 0x80] as u32).unwrap()
                }
            })
            .collect(),
        "latin1" => bytes.iter().map(|&b| b as char).collect(),
        "ascii" => {
            if bytes.iter().any(|b| *b >= 0x80) {
                return Err("undecodable ASCII name record".into());
            }
            bytes.iter().map(|&b| b as char).collect()
        }
        codec => crate::codecs::decode(codec, &bytes)
            .ok_or_else(|| format!("no decoder for codec {codec}"))??,
    };
    // a decoded string that still looks like UTF-16BE ASCII is narrowed down
    let chars: Vec<char> = string.chars().collect();
    if chars.iter().enumerate().all(|(i, c)| {
        if i % 2 == 0 {
            *c as u32 == 0
        } else {
            is_ascii_text(*c as u32)
        }
    }) {
        return Ok(chars.iter().skip(1).step_by(2).collect());
    }
    Ok(string)
}

fn bits(value: u64, start: i64) -> Vec<i64> {
    (0..64)
        .filter(|i| value >> i & 1 == 1)
        .map(|i| i + start)
        .collect()
}

fn int_list(values: &[i64]) -> Value {
    Value::Array(values.iter().map(|v| Value::Int(*v)).collect())
}

/// `time.strftime("%Y/%m/%d %H:%M:%S", time.gmtime(max(0, date + mac_epoch_diff)))`
/// for a `head` timestamp as fontTools decodes it.
fn format_date(raw: i64) -> String {
    // fontTools reads the field as an unsigned 64-bit integer
    let mut value = raw as u64;
    if value > 0xFFFF_FFFF {
        value &= 0xFFFF_FFFF;
    }
    let mut value = value as i64;
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
    format!(
        "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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
    fn set(info: &mut Dict, key: &str, value: impl Into<Value>) {
        info.insert(key.to_string(), value.into());
    }

    // head
    let head = font.head()?;
    let version = version_string(head.font_revision().to_f64());
    let (major, minor) = version.split_once('.').unwrap();
    set(&mut info, "versionMajor", Value::Int(major.parse()?));
    let minor: i64 = minor.parse()?;
    if minor >= 0 {
        set(&mut info, "versionMinor", minor);
    }
    set(&mut info, "unitsPerEm", head.units_per_em());
    set(
        &mut info,
        "openTypeHeadCreated",
        format_date(head.created().as_secs()),
    );
    set(
        &mut info,
        "openTypeHeadLowestRecPPEM",
        head.lowest_rec_ppem(),
    );
    // The raw field: read-fonts' `head.flags()` drops the bits it doesn't
    // define (5-10), which fontTools keeps.
    let raw_flags: u16 = head.offset_data().read_at(head.flags_byte_range().start)?;
    let flags = bits(raw_flags as u64, 0);
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
    set(&mut info, "styleMapStyleName", style);

    // name
    let name = font.name()?;
    let data = name.string_data();
    let mut records: Vec<(u16, u16, u16, u16, String)> = Vec::new();
    // nameIDs dict: insertion order of first occurrence, value of the last
    let mut name_ids: indexmap::IndexMap<(u16, u16, u16, u16), String> = indexmap::IndexMap::new();
    for record in name.name_record() {
        let r: &NameRecord = record;
        let start = r.string_offset().to_u32() as usize;
        let raw = data
            .as_bytes()
            .get(start..start + r.length() as usize)
            .ok_or("bad name record")?
            .to_vec();
        let s = decode_name(r.platform_id(), r.encoding_id(), r.language_id(), &raw)?;
        let key = (
            r.name_id().to_u16(),
            r.platform_id(),
            r.encoding_id(),
            r.language_id(),
        );
        name_ids.insert(key, s.clone());
        records.push((key.0, key.1, key.2, key.3, s));
    }
    type Priority = (u16, Option<u16>, Option<u16>, Option<u16>);
    let skim = |priority: &[Priority]| -> Option<String> {
        for &(nid, pid, eid, lid) in priority {
            for ((n, p, e, l), text) in &name_ids {
                if *n != nid
                    || pid.is_some_and(|v| v != *p)
                    || eid.is_some_and(|v| v != *e)
                    || lid.is_some_and(|v| v != *l)
                {
                    continue;
                }
                return Some(text.clone());
            }
        }
        None
    };
    let prio = |id: u16| {
        vec![
            (id, Some(1), Some(0), Some(0)),
            (id, Some(1), None, None),
            (id, None, None, None),
        ]
    };
    let attributes: [(&str, Vec<u16>); 21] = [
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
    ];
    for (attr, ids) in &attributes {
        let priority: Vec<_> = ids.iter().flat_map(|id| prio(*id)).collect();
        if let Some(v) = skim(&priority) {
            set(&mut info, attr, v);
        }
    }
    records.sort();
    // Python sorts (nameID, platformID, encodingID, languageID, dict) tuples;
    // two records with the same IDs compare their dicts: TypeError.
    if records
        .windows(2)
        .any(|w| (w[0].0, w[0].1, w[0].2, w[0].3) == (w[1].0, w[1].1, w[1].2, w[1].3))
    {
        return Err("duplicate name records cannot be sorted".into());
    }
    let name_records: Vec<Value> = records
        .into_iter()
        .map(|(nid, pid, eid, lid, s)| {
            let mut d = Dict::new();
            d.insert("nameID".into(), nid.into());
            d.insert("platformID".into(), pid.into());
            d.insert("encodingID".into(), eid.into());
            d.insert("languageID".into(), lid.into());
            d.insert("string".into(), s.into());
            Value::Dict(d)
        })
        .collect();
    set(&mut info, "openTypeNameRecords", Value::Array(name_records));

    // OS/2
    let Ok(os2) = font.os2() else {
        return Err("fonts without an 'OS/2' table are not supported".into());
    };
    let width = os2.us_width_class() as i64;
    if (1..=9).contains(&width) {
        set(&mut info, "openTypeOS2WidthClass", width);
    }
    set(&mut info, "openTypeOS2WeightClass", os2.us_weight_class());
    let selection: Vec<i64> = bits(os2.fs_selection().bits() as u64, 0)
        .into_iter()
        .filter(|b| (1..=4).contains(b))
        .collect();
    set(&mut info, "openTypeOS2Selection", int_list(&selection));
    let vendor_bytes = os2.ach_vend_id().to_be_bytes();
    if !vendor_bytes.is_ascii() {
        return Err("non-ASCII OS/2 vendor IDs are not supported".into());
    }
    let vendor: String = vendor_bytes.iter().map(|&b| b as char).collect();
    set(
        &mut info,
        "openTypeOS2VendorID",
        vendor.trim_end_matches('\0').to_string(),
    );
    let p = os2.panose_10();
    set(
        &mut info,
        "openTypeOS2Panose",
        int_list(&p.iter().map(|v| *v as i64).collect::<Vec<_>>()),
    );
    let ranges = bits(os2.ul_unicode_range_1() as u64, 0)
        .into_iter()
        .chain(bits(os2.ul_unicode_range_2() as u64, 32))
        .chain(bits(os2.ul_unicode_range_3() as u64, 64))
        .chain(bits(os2.ul_unicode_range_4() as u64, 96))
        .collect::<Vec<_>>();
    set(&mut info, "openTypeOS2UnicodeRanges", int_list(&ranges));
    if let (Some(c1), Some(c2)) = (os2.ul_code_page_range_1(), os2.ul_code_page_range_2()) {
        let pages = bits(c1 as u64, 0)
            .into_iter()
            .chain(bits(c2 as u64, 32))
            .collect::<Vec<_>>();
        set(&mut info, "openTypeOS2CodePageRanges", int_list(&pages));
    }
    if let Some(v) = os2.sx_height() {
        set(&mut info, "xHeight", v);
    }
    if let Some(v) = os2.s_cap_height() {
        set(&mut info, "capHeight", v);
    }
    for (key, v) in [
        ("ascender", os2.s_typo_ascender()),
        ("descender", os2.s_typo_descender()),
        ("openTypeOS2TypoAscender", os2.s_typo_ascender()),
        ("openTypeOS2TypoDescender", os2.s_typo_descender()),
        ("openTypeOS2TypoLineGap", os2.s_typo_line_gap()),
    ] {
        set(&mut info, key, v);
    }
    set(&mut info, "openTypeOS2WinAscent", os2.us_win_ascent());
    set(&mut info, "openTypeOS2WinDescent", os2.us_win_descent());
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
        (
            "openTypeOS2SuperscriptXOffset",
            os2.y_superscript_x_offset(),
        ),
        (
            "openTypeOS2SuperscriptYOffset",
            os2.y_superscript_y_offset(),
        ),
        ("openTypeOS2StrikeoutSize", os2.y_strikeout_size()),
        ("openTypeOS2StrikeoutPosition", os2.y_strikeout_position()),
    ] {
        set(&mut info, key, v);
    }

    // hhea, vhea
    if let Ok(hhea) = font.hhea() {
        for (key, v) in [
            ("openTypeHheaAscender", hhea.ascender().to_i16()),
            ("openTypeHheaDescender", hhea.descender().to_i16()),
            ("openTypeHheaLineGap", hhea.line_gap().to_i16()),
            ("openTypeHheaCaretSlopeRise", hhea.caret_slope_rise()),
            ("openTypeHheaCaretSlopeRun", hhea.caret_slope_run()),
            ("openTypeHheaCaretOffset", hhea.caret_offset()),
        ] {
            set(&mut info, key, v);
        }
    }
    if let Ok(vhea) = font.vhea() {
        for (key, v) in [
            ("openTypeVheaVertTypoAscender", vhea.ascender().to_i16()),
            ("openTypeVheaVertTypoDescender", vhea.descender().to_i16()),
            ("openTypeVheaVertTypoLineGap", vhea.line_gap().to_i16()),
            ("openTypeVheaCaretSlopeRise", vhea.caret_slope_rise()),
            ("openTypeVheaCaretSlopeRun", vhea.caret_slope_run()),
            ("openTypeVheaCaretOffset", vhea.caret_offset()),
        ] {
            set(&mut info, key, v);
        }
    }
    // post
    let post = font.post()?;
    set(
        &mut info,
        "italicAngle",
        Value::Real(post.italic_angle().to_f64()),
    );
    set(
        &mut info,
        "postscriptUnderlineThickness",
        post.underline_thickness().to_i16(),
    );
    set(
        &mut info,
        "postscriptUnderlinePosition",
        post.underline_position().to_i16(),
    );
    set(
        &mut info,
        "postscriptIsFixedPitch",
        Value::Bool(post.is_fixed_pitch() != 0),
    );

    // gasp
    if let Some(data) = font.table_data(Tag::new(b"gasp")) {
        let d = data.as_bytes();
        let u16_at = |at: usize| -> Result<u16, Error> {
            Ok(u16::from_be_bytes(
                d.get(at..at + 2).ok_or("bad 'gasp' table")?.try_into()?,
            ))
        };
        // fontTools asserts the version and that no data is left over
        let version = u16_at(0)?;
        if version > 1 {
            return Err(format!("unknown 'gasp' format: {version}").into());
        }
        let n = u16_at(2)? as usize;
        if d.len() > 4 + 4 * n {
            return Err("too much data in 'gasp' table".into());
        }
        // fontTools keeps a dict (last record per size wins), then sorts
        let by_size: std::collections::BTreeMap<i64, u16> = (0..n)
            .map(|i| -> Result<(i64, u16), Error> {
                let at = 4 + 4 * i;
                Ok((u16_at(at)? as i64, u16_at(at + 2)?))
            })
            .collect::<Result<_, _>>()?;
        let records: Vec<Value> = by_size
            .into_iter()
            .map(|(size, b)| {
                let mut d = Dict::new();
                d.insert("rangeMaxPPEM".into(), Value::Int(size));
                d.insert(
                    "rangeGaspBehavior".into(),
                    int_list(&bits((b & 0xF) as u64, 0)),
                );
                Value::Dict(d)
            })
            .collect();
        set(&mut info, "openTypeGaspRangeRecords", Value::Array(records));
    }

    // _extractLibInfo
    lib.insert(
        "public.openTypeHeadModified".into(),
        format_date(head.modified().as_secs()).into(),
    );
    lib.insert(
        "public.openTypePostUnderlinePosition".into(),
        post.underline_position().to_i16().into(),
    );
    if font.table_data(Tag::new(b"CFF ")).is_some() {
        return Err("CFF-flavored fonts are not supported".into());
    }
    Ok((info, lib))
}
