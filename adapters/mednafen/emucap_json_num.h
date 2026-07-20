#pragma once

#include <cerrno>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <limits>
#include <string>

enum class EmucapJsonNumberStatus {
  absent,
  valid,
  invalid,
};

inline bool emucap_json_number_delimiter(char value) {
  return value == '\0' || value == ',' || value == '}' || value == ']';
}

inline EmucapJsonNumberStatus emucap_json_u64(
    const std::string& input,
    const char* key,
    std::uint64_t& output,
    std::size_t from = 0) {
  const std::string pattern = std::string("\"") + key + "\"";
  const std::size_t key_pos = input.find(pattern, from);
  if (key_pos == std::string::npos) return EmucapJsonNumberStatus::absent;

  const std::size_t colon = input.find(':', key_pos + pattern.size());
  if (colon == std::string::npos) return EmucapJsonNumberStatus::invalid;
  std::size_t pos = colon + 1;
  while (pos < input.size() && std::isspace(static_cast<unsigned char>(input[pos]))) pos++;

  const bool quoted = pos < input.size() && input[pos] == '"';
  if (quoted) pos++;
  if (pos >= input.size() || input[pos] == '-' || input[pos] == '+')
    return EmucapJsonNumberStatus::invalid;

  int base = 10;
  if (pos + 2 <= input.size() && input[pos] == '0'
      && (input[pos + 1] == 'x' || input[pos + 1] == 'X')) {
    base = 16;
    pos += 2;
  }

  const char* begin = input.c_str() + pos;
  char* end = nullptr;
  errno = 0;
  const unsigned long long parsed = std::strtoull(begin, &end, base);
  if (end == begin || errno == ERANGE) return EmucapJsonNumberStatus::invalid;

  if (quoted) {
    if (*end != '"') return EmucapJsonNumberStatus::invalid;
    end++;
  }
  while (*end != '\0' && std::isspace(static_cast<unsigned char>(*end))) end++;
  if (!emucap_json_number_delimiter(*end)) return EmucapJsonNumberStatus::invalid;

  output = static_cast<std::uint64_t>(parsed);
  return EmucapJsonNumberStatus::valid;
}

inline EmucapJsonNumberStatus emucap_json_u32(
    const std::string& input,
    const char* key,
    std::uint32_t& output,
    std::size_t from = 0) {
  std::uint64_t parsed = 0;
  const EmucapJsonNumberStatus status = emucap_json_u64(input, key, parsed, from);
  if (status != EmucapJsonNumberStatus::valid) return status;
  if (parsed > std::numeric_limits<std::uint32_t>::max())
    return EmucapJsonNumberStatus::invalid;
  output = static_cast<std::uint32_t>(parsed);
  return EmucapJsonNumberStatus::valid;
}
