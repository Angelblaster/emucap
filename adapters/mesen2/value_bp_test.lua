-- emucap-core.lua의 폭>1 write 값-BP 회귀 테스트(스탠드얼론). `lua value_bp_test.lua`.
-- 두 로직의 사본을 mirror한다 — 한쪽을 바꾸면 emucap-core.lua도 함께 갱신한다.
--   (1) access_value의 write 누적(burst 정체성): 한 store로 연속 쓰인 폭 전체만 매치, 무관한 산발
--       write는 폐기.
--   (2) set_breakpoint의 미러 등록(폭 확장 ⊥ 뱅크 미러): $2000-$7FFF 폭>1 write 값-BP는 상위 바이트
--       주소'와' 뱅크 미러를 둘 다 등록.

local function eq(a, b, msg)
  if a ~= b then error(("FAIL %s: %s ~= %s"):format(msg, tostring(a), tostring(b))) end
end

-- ── (1) access_value write 누적 사본 ──────────────────────────────────────────
-- emucap-core.lua access_value의 write 분기와 동일 로직. addr 상대 오프셋으로 per-byte write를 누적하고,
-- 폭 전체가 저→고 연속 burst로 관측됐을 때만 재구성 값을, 아니면 nil을 돌린다.
local function av_write(bp, addr, value)
  local len = bp.value_len
  local off = addr - bp.start
  if off >= 0x800000 then off = off - 0x800000 end   -- $80 뱅크 미러 → canonical 오프셋
  if off < 0 or off >= len then return nil end
  local buf = bp.wbytes
  if off == 0 then
    buf = { [0] = value }; bp.wbytes = buf; bp.wnext = 1
  elseif buf ~= nil and off == bp.wnext then
    buf[off] = value; bp.wnext = off + 1
  else
    bp.wbytes = nil; bp.wnext = nil; return nil
  end
  if bp.wnext < len then return nil end
  local v = 0
  for i = 0, len - 1 do v = v + buf[i] * (256 ^ i) end
  bp.wbytes = nil; bp.wnext = nil
  return v
end

-- 진성 연속 2바이트 store(저→고): 폭 완결 시 전체 값 재구성
local bp2 = { start = 0x7E0100, value_len = 2 }
eq(av_write(bp2, 0x7E0100, 0x34), nil, "2B low → 미결")
eq(av_write(bp2, 0x7E0101, 0x12), 0x1234, "2B high → 0x1234 완결")

-- 무관한 두 단일바이트 write(상위바이트 먼저, 저바이트 나중 = 역순): spurious 매치 금지
local bpU = { start = 0x7E0100, value_len = 2 }
eq(av_write(bpU, 0x7E0101, 0xAA), nil, "역순 high → 미결(burst 시작 아님)")
eq(av_write(bpU, 0x7E0100, 0xBB), nil, "역순 low → 새 burst 시작일 뿐, 여전히 미결(가짜 완결 없음)")

-- 무관한 산발 write가 옛 상위바이트를 완결시키지 않음: high(store A) … 뒤늦은 low(store B)는 위와 동일 —
-- low가 새 burst를 시작하므로 store A의 stale high와 절대 합쳐지지 않는다.
local bpS = { start = 0x7E0100, value_len = 2 }
av_write(bpS, 0x7E0101, 0x99)                                   -- store A: high만
eq(bpS.wbytes, nil, "역순 high는 버퍼에 남지 않음")
eq(av_write(bpS, 0x7E0100, 0x77), nil, "store B: low → 미결(A의 high와 재구성 안 됨)")

-- 폭>1 non-contiguous(len=3, 중간 오프셋 건너뜀): 완결 금지
local bp3 = { start = 0x000000, value_len = 3 }
eq(av_write(bp3, 0x000000, 0x01), nil, "3B off0 → 미결")
eq(av_write(bp3, 0x000002, 0x03), nil, "3B off2(off1 건너뜀) → burst 폐기")
eq(av_write(bp3, 0x000001, 0x02), nil, "3B off1 → wnext(2 아님)와 불일치? off1==wnext(1)면 이어짐이나 폐기 후 wnext=nil")

-- 진성 연속 3바이트 store(저→고 순차)는 완결
local bp3b = { start = 0x000000, value_len = 3 }
eq(av_write(bp3b, 0x000000, 0x11), nil, "3B off0")
eq(av_write(bp3b, 0x000001, 0x22), nil, "3B off1")
eq(av_write(bp3b, 0x000002, 0x33), 0x332211, "3B off2 → 0x332211 완결")

-- 핵심 회귀($80 뱅크 미러 히트): 폭>1 write 값-BP가 canonical addr가 아닌 $80 미러(0x802118)로 발화해도
-- access_value가 canonical 오프셋으로 정규화해 누적·완결한다(정규화 없으면 off=0x800000+ → 영영 미발화).
local bpM = { start = 0x2118, value_len = 2 }
eq(av_write(bpM, 0x802118, 0x34), nil, "$80 미러 low(0x802118) → 미결")
eq(av_write(bpM, 0x802119, 0x12), 0x1234, "$80 미러 high(0x802119) → 0x1234 완결")

-- ── (2) 미러 등록(span ⊥ bank_mirror) 사본 ────────────────────────────────────
-- emucap-core.lua set_breakpoint의 span/mirrors 계산과 동일.
local function compute_mirrors(SYS, p, bp)
  local span = bp.end_
  if p.kind == "write" and bp.has_value and bp.value_len > 1 and bp.start == bp.end_ then
    span = bp.start + bp.value_len - 1
  end
  local mirrors = { { bp.start, span } }
  if SYS.bank_mirror and (p.kind == "read" or p.kind == "write") and p.memory_type == SYS.default_memtype
     and bp.start == bp.end_ and bp.start >= 0x2000 and bp.start < 0x8000 then
    mirrors = { { bp.start, span }, { bp.start + 0x800000, span + 0x800000 } }
  end
  return mirrors
end

local SNES = { bank_mirror = true, default_memtype = "snesMemory" }

-- 핵심 회귀: $2000-$7FFF 폭=2 write 값-BP → 두 바이트 '그리고' 두 뱅크 미러 모두 등록.
do
  local p = { kind = "write", memory_type = "snesMemory" }
  local bp = { start = 0x2118, end_ = 0x2118, has_value = true, value_len = 2 }
  local m = compute_mirrors(SNES, p, bp)
  eq(#m, 2, "$2118 폭2 write: 뱅크 미러 2개")
  eq(m[1][1], 0x2118, "미러1 lo"); eq(m[1][2], 0x2119, "미러1 hi(폭 전체)")
  eq(m[2][1], 0x802118, "미러2 lo($80)"); eq(m[2][2], 0x802119, "미러2 hi(폭 전체)")
end

-- 단일바이트 read at $2118: 폭 확장 없음, 두 단일바이트 뱅크 미러(종전 동작 유지)
do
  local p = { kind = "read", memory_type = "snesMemory" }
  local bp = { start = 0x2118, end_ = 0x2118, has_value = false, value_len = 1 }
  local m = compute_mirrors(SNES, p, bp)
  eq(#m, 2, "$2118 단일 read: 미러 2개")
  eq(m[1][2], 0x2118, "미러1 단일바이트"); eq(m[2][2], 0x802118, "미러2 단일바이트")
end

-- 폭=2 read at $2118: read는 상위바이트를 메모리에서 읽으므로 폭 확장 안 함(단일바이트 미러 유지)
do
  local p = { kind = "read", memory_type = "snesMemory" }
  local bp = { start = 0x2118, end_ = 0x2118, has_value = true, value_len = 2 }
  local m = compute_mirrors(SNES, p, bp)
  eq(m[1][2], 0x2118, "폭2 read 미러1 단일바이트"); eq(m[2][2], 0x802118, "폭2 read 미러2 단일바이트")
end

-- 폭=2 write, 뱅크 미러 범위 밖(LowRAM $1FF0 < $2000): 폭 전체 단일 미러만
do
  local p = { kind = "write", memory_type = "snesMemory" }
  local bp = { start = 0x1FF0, end_ = 0x1FF0, has_value = true, value_len = 2 }
  local m = compute_mirrors(SNES, p, bp)
  eq(#m, 1, "LowRAM 폭2 write: 미러 1개")
  eq(m[1][1], 0x1FF0, "lo"); eq(m[1][2], 0x1FF1, "hi(폭 전체)")
end

-- 뱅크 미러 없는 시스템(bank_mirror=nil)의 폭=2 write: 폭 전체 단일 미러
do
  local Z80 = { bank_mirror = nil, default_memtype = "smsMemory" }
  local p = { kind = "write", memory_type = "smsMemory" }
  local bp = { start = 0xC000, end_ = 0xC000, has_value = true, value_len = 2 }
  local m = compute_mirrors(Z80, p, bp)
  eq(#m, 1, "non-bank 폭2 write: 미러 1개")
  eq(m[1][2], 0xC001, "hi(폭 전체)")
end

print("ALL VALUE-BP TESTS PASSED")
