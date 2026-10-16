//! IP parameters as the hardware handoff and `project.toml` carry them: text by name.

use std::collections::BTreeMap;

/// IP parameters by their Vivado names (`C_GPIO_WIDTH`), values as text (`4`, `0x00000000`,
/// `true`).
pub type Params = BTreeMap<String, String>;

/// Parses a decimal, `0x` hexadecimal or boolean (`true`/`false`) number.
pub fn parse_number(text: &str) -> Option<u64> {
    let t = text.trim();
    match t.to_ascii_lowercase().as_str() {
        "true" => return Some(1),
        "false" => return Some(0),
        _ => {}
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).ok();
    }
    t.parse::<u64>().ok()
}

/// The numeric parameter `key`, or `default` when it is absent.
pub fn number(p: &Params, key: &str, default: u64) -> Result<u64, String> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => parse_number(v).ok_or_else(|| {
            format!("parameter {key} = `{v}` is not a number (decimal, 0x hex or true/false)")
        }),
    }
}

/// The boolean parameter `key` (any non-zero number is true).
pub fn flag(p: &Params, key: &str, default: bool) -> Result<bool, String> {
    Ok(number(p, key, u64::from(default))? != 0)
}

/// The numeric parameter `key` limited to `lo..=hi`.
pub fn ranged(p: &Params, key: &str, default: u64, lo: u64, hi: u64) -> Result<u64, String> {
    let v = number(p, key, default)?;
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("parameter {key} = {v} is outside {lo}..={hi}"))
    }
}

/// `old` with the byte lanes of `new` selected by `strobe` (bit *n* = lane *n*) written in.
pub fn merge(old: u32, new: u32, strobe: u8) -> u32 {
    let mut v = old;
    for lane in 0..4 {
        if strobe & (1 << lane) != 0 {
            let m = 0xFFu32 << (8 * lane);
            v = (v & !m) | (new & m);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_flags_and_ranges() {
        let mut p = Params::new();
        p.insert("A".into(), "0x10".into());
        p.insert("B".into(), "true".into());
        p.insert("C".into(), "40".into());
        p.insert("D".into(), "x".into());
        assert_eq!(number(&p, "A", 0), Ok(16));
        assert_eq!(number(&p, "Z", 7), Ok(7));
        assert_eq!(flag(&p, "B", false), Ok(true));
        assert!(ranged(&p, "C", 1, 1, 32).is_err());
        assert!(number(&p, "D", 0).unwrap_err().contains("not a number"));
    }
}
