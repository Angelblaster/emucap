#include "emucap_json_num.h"

#include <cassert>
#include <cstdint>
#include <limits>
#include <string>

#ifdef _WIN32
static_assert(sizeof(long) == 4, "the Windows regression test must exercise LLP64");
#endif

int main() {
  std::uint64_t u64 = 0;
  assert(emucap_json_u64(R"({"address":2147483648})", "address", u64)
         == EmucapJsonNumberStatus::valid);
  assert(u64 == UINT64_C(0x80000000));

  std::uint32_t u32 = 0;
  assert(emucap_json_u32(R"({"address":4294967295})", "address", u32)
         == EmucapJsonNumberStatus::valid);
  assert(u32 == std::numeric_limits<std::uint32_t>::max());
  assert(emucap_json_u32(R"({"address":"0x80000000"})", "address", u32)
         == EmucapJsonNumberStatus::valid);
  assert(u32 == UINT32_C(0x80000000));

  assert(emucap_json_u64(R"({"value":18446744073709551615})", "value", u64)
         == EmucapJsonNumberStatus::valid);
  assert(u64 == std::numeric_limits<std::uint64_t>::max());

  const std::string invalid[] = {
      R"({"address":-1})",
      R"({"address":4294967296})",
      R"({"address":18446744073709551616})",
      R"({"address":"0x"})",
      R"({"address":"0x80000000junk"})",
      R"({"address":1.5})",
  };
  assert(emucap_json_u32(invalid[0], "address", u32)
         == EmucapJsonNumberStatus::invalid);
  for (std::size_t index = 1; index < sizeof(invalid) / sizeof(invalid[0]); index++) {
    assert(emucap_json_u32(invalid[index], "address", u32)
           == EmucapJsonNumberStatus::invalid);
  }
  assert(emucap_json_u32(R"({"length":4})", "address", u32)
         == EmucapJsonNumberStatus::absent);

  return 0;
}
