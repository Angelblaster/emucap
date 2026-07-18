emucap_windows_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$1"
  else
    printf '%s\n' "$1"
  fi
}

emucap_runtime_base() {
  if [ -n "${EMUCAP_EMU_HOME:-}" ]; then
    printf '%s\n' "$EMUCAP_EMU_HOME"
    return 0
  fi
  case "$(uname -s 2>/dev/null || echo unknown)" in
    Darwin)
      printf '%s/Library/Application Support/emucap\n' "${HOME:?HOME is required}"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      if [ -n "${LOCALAPPDATA:-}" ]; then
        printf '%s/emucap\n' "$(emucap_windows_path "$LOCALAPPDATA")"
      else
        printf '%s/emucap\n' "$(emucap_windows_path "${TEMP:?LOCALAPPDATA or TEMP is required}")"
      fi
      ;;
    *)
      if [ -n "${XDG_DATA_HOME:-}" ]; then
        printf '%s/emucap\n' "$XDG_DATA_HOME"
      else
        printf '%s/.local/share/emucap\n' "${HOME:?HOME is required}"
      fi
      ;;
  esac
}

emucap_session_token_file() {
  if [ -n "${EMUCAP_SESSION_TOKEN_FILE:-}" ]; then
    printf '%s\n' "$EMUCAP_SESSION_TOKEN_FILE"
    return 0
  fi
  local base
  base="$(emucap_runtime_base)"
  printf '%s/sessions/compatibility/session-token-%s\n' "${base%/}" "$1"
}
