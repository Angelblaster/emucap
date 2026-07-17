use super::parse_predicate;

#[test]
fn parse_predicate_accepts_hex_like_mcp() {
    // #45: CLI도 0x/$ 16진을 받아야(MCP와 동일). address·value 16진, length 10진.
    let p = parse_predicate("wram:0x7e0010:2:eq:$1234").unwrap();
    assert_eq!(p.address, 0x7e0010);
    assert_eq!(p.value, 0x1234);
    assert_eq!(p.length, 2);
    // 10진도 여전히 동작
    let d = parse_predicate("wram:100:1:eq:5").unwrap();
    assert_eq!(d.address, 100);
    assert_eq!(d.value, 5);
}
