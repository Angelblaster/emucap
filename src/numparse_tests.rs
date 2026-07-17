use super::parse_num_str;

#[test]
fn decimal_and_hex_forms() {
    assert_eq!(parse_num_str("8471").unwrap(), 8471);
    assert_eq!(parse_num_str("0x2117").unwrap(), 0x2117);
    assert_eq!(parse_num_str("0X2117").unwrap(), 0x2117);
    assert_eq!(parse_num_str("$2117").unwrap(), 0x2117);
    assert_eq!(parse_num_str("0x80_420b").unwrap(), 0x0080_420b);
    assert_eq!(parse_num_str(" 0x420B ").unwrap(), 0x420b);
    assert_eq!(parse_num_str("+16").unwrap(), 16);
    // 따옴표 이중인코딩
    assert_eq!(parse_num_str("\"$80BC95\"").unwrap(), 0x80_BC95);
}

#[test]
fn rejects_garbage() {
    assert!(parse_num_str("zzz").is_err());
    assert!(parse_num_str("0x").is_err());
    assert!(parse_num_str("").is_err());
}
