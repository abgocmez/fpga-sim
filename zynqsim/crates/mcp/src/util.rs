use sim_api::ApiError;

/// Decodes a hex string, ignoring ASCII whitespace.
pub fn parse_hex(s: &str) -> Result<Vec<u8>, ApiError> {
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 2 != 0 {
        return Err(ApiError::invalid_argument(
            "hex string has an odd number of digits",
        ));
    }
    let nibble = |c: u8| -> Result<u8, ApiError> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(ApiError::invalid_argument(format!(
                "`{}` is not a hex digit",
                char::from(c)
            ))),
        }
    };
    clean
        .chunks(2)
        .map(|p| Ok(nibble(p[0])? << 4 | nibble(p[1])?))
        .collect()
}

/// Lower-case hex encoding.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_errors() {
        assert_eq!(
            parse_hex("de ad BE ef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(to_hex(&[0, 255, 16]), "00ff10");
        assert!(parse_hex("abc").is_err());
        assert!(parse_hex("zz").is_err());
        assert_eq!(parse_hex("").unwrap(), Vec::<u8>::new());
    }
}
