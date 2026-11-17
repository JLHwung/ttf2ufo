//! Reproductions of Python behaviors that leak into extractufo's output.

/// Python's `repr(float)`: the shortest string that round-trips, in fixed
/// notation for exponents in [-4, 16) and scientific notation otherwise.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.into();
    }
    // Rust's LowerExp prints the shortest round-tripping digits: "-1.2345e3".
    let sci = format!("{value:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let n = digits.len() as i32;
    if (-4..16).contains(&exp) {
        let point = exp + 1; // position of the decimal point within digits
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= n {
            format!("{}{}.0", digits, "0".repeat((point - n) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        format!("{sign}{body}")
    } else {
        let mut m = digits[..1].to_string();
        if n > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{m}e{esign}{:02}", exp.abs())
    }
}

/// Python's `f"{value:+}"` for a float (`repr` with an explicit sign).
pub fn float_repr_signed(value: f64) -> String {
    let r = float_repr(value);
    if r.starts_with('-') { r } else { format!("+{r}") }
}

/// A number that keeps Python's int/float distinction, which matters for
/// `repr` (`1` vs `1.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    pub fn repr(self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) => float_repr(f),
        }
    }
}

/// Iteration order of a CPython `set` of non-negative ints built by adding
/// `values` in order (`list(s)` / `for x in s`). Used where extractor turns a
/// set into a list, e.g. a glyph's unicodes.
pub fn int_set_order(values: impl IntoIterator<Item = u64>) -> Vec<u64> {
    // hash(int) == int for small non-negative ints
    py_set_order(values.into_iter().map(|v| (v, v)))
}

/// CPython's `hash(tuple_of_small_non_negative_ints)` (64-bit builds).
pub fn tuple_hash(items: &[u64]) -> u64 {
    const XXPRIME_1: u64 = 11400714785074694791;
    const XXPRIME_2: u64 = 14029467366897019727;
    const XXPRIME_5: u64 = 2870177450012600261;
    let mut acc = XXPRIME_5;
    for &lane in items {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc = acc.wrapping_add((items.len() as u64) ^ (XXPRIME_5 ^ 3527539));
    if acc == u64::MAX { 1546275796 } else { acc }
}

/// Iteration order of a CPython `set` built by adding `(hash, key)` items in
/// order. `set.pop()` returns the first element of this order.
///
/// Mirrors CPython's setobject.c: open addressing over a power-of-two table
/// with 9 linear probes and perturbed rehashing, growing when 3/5 full.
pub fn py_set_order<K: Eq + Clone>(items: impl IntoIterator<Item = (u64, K)>) -> Vec<K> {
    const MIN_SIZE: usize = 8;
    const LINEAR_PROBES: usize = 9;
    const PERTURB_SHIFT: u32 = 5;

    fn insert_clean<K>(table: &mut [Option<(u64, K)>], hash: u64, key: K) {
        let mask = table.len() - 1;
        let mut perturb = hash;
        let mut i = (hash as usize) & mask;
        loop {
            if table[i].is_none() {
                table[i] = Some((hash, key));
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for j in 1..=LINEAR_PROBES {
                    if table[i + j].is_none() {
                        table[i + j] = Some((hash, key));
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb as usize)) & mask;
        }
    }

    let mut table: Vec<Option<(u64, K)>> = (0..MIN_SIZE).map(|_| None).collect();
    let mut used = 0usize;
    for (hash, key) in items {
        let mask = table.len() - 1;
        // set_add_entry: look for the key or a free slot
        let mut perturb = hash;
        let mut i = (hash as usize) & mask;
        let mut found = false;
        'probe: loop {
            let probes = if i + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
            for j in 0..=probes {
                match &table[i + j] {
                    Some((h, k)) if *h == hash && *k == key => {
                        found = true;
                        break 'probe;
                    }
                    None => {
                        table[i + j] = Some((hash, key.clone()));
                        break 'probe;
                    }
                    Some(_) => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb as usize)) & mask;
        }
        if found {
            continue;
        }
        used += 1;
        // fill == used here, as nothing is ever deleted
        if used * 5 >= mask * 3 {
            let minused = if used > 50000 { used * 2 } else { used * 4 };
            let mut newsize = MIN_SIZE;
            while newsize <= minused {
                newsize <<= 1;
            }
            let old = std::mem::replace(&mut table, (0..newsize).map(|_| None).collect());
            for (h, k) in old.into_iter().flatten() {
                insert_clean(&mut table, h, k);
            }
        }
    }
    table.into_iter().flatten().map(|(_, k)| k).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_reprs() {
        assert_eq!(float_repr(0.0), "0.0");
        assert_eq!(float_repr(-12.0), "-12.0");
        assert_eq!(float_repr(0.5), "0.5");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1234567890123456.0), "1234567890123456.0");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(-0.6123), "-0.6123");
        assert_eq!(float_repr(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(float_repr_signed(0.0), "+0.0");
        assert_eq!(float_repr_signed(-0.25), "-0.25");
    }

    #[test]
    fn set_orders() {
        // values checked against CPython 3.14: list(set([...]))
        assert_eq!(int_set_order([0x0F, 0x10]), vec![0x10, 0x0F]);
        assert_eq!(int_set_order([0x20, 0xA0]), vec![0x20, 0xA0]);
        assert_eq!(int_set_order([0xA0, 0x20]), vec![0xA0, 0x20]);
        assert_eq!(int_set_order([0x41, 0x391]), vec![0x41, 0x391]);
        assert_eq!(int_set_order([5, 4, 3, 2, 1, 0]), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn tuple_hashes() {
        assert_eq!(tuple_hash(&[0, 0, 0]), 3010437511937009226);
        assert_eq!(tuple_hash(&[1, 2, 3]), 529344067295497451);
        assert_eq!(tuple_hash(&[5, 0, 12]), 7181335228091685182);
        assert_eq!(tuple_hash(&[17, 3, 0]), 7587551416444501075);
    }

    #[test]
    fn tuple_set_orders_match_cpython() {
        // "inserted | list(set(inserted)) | set(inserted).pop()", from CPython 3.14
        let cases = include_str!("../tests/data/tuple_set_cases.txt");
        let parse = |s: &str| -> Vec<Vec<u64>> {
            s.split(';').map(|t| t.split(',').map(|v| v.parse().unwrap()).collect()).collect()
        };
        for line in cases.lines() {
            let parts: Vec<&str> = line.split(" | ").collect();
            let order = py_set_order(parse(parts[0]).into_iter().map(|t| (tuple_hash(&t), t)));
            assert_eq!(order, parse(parts[1]), "{line}");
            assert_eq!(order[0], parse(parts[2])[0], "{line}");
        }
    }

    #[test]
    fn set_orders_match_cpython() {
        // "inserted values | list(set(values))", generated with CPython 3.14
        let cases = include_str!("../tests/data/set_order_cases.txt");
        for line in cases.lines() {
            let (input, expected) = line.split_once(" | ").unwrap();
            let parse = |s: &str| -> Vec<u64> { s.split(' ').map(|v| v.parse().unwrap()).collect() };
            assert_eq!(int_set_order(parse(input)), parse(expected), "{input}");
        }
    }
}
