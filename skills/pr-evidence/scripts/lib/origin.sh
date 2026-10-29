#!/usr/bin/env bash
# is_test_origin URL -> exit 0 if host is a local/test origin, 1 otherwise. Bash 3.2 safe.
is_test_origin() {
  local url="$1" host
  [ -n "$url" ] || return 1
  host="${url#*://}"        # strip scheme://
  host="${host%%/*}"        # strip /path
  host="${host%%#*}"        # strip #fragment
  host="${host%%\?*}"       # strip ?query
  host="${host##*@}"        # strip userinfo@ (only within the authority now)
  if [ "${host#\[}" != "$host" ]; then
    host="${host#\[}"; host="${host%%\]*}"   # IPv6 in [brackets]
  else
    host="${host%%:*}"      # strip :port
  fi
  host="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
  [ -n "$host" ] || return 1
  case "$host" in
    localhost|*.localhost|::1) return 0 ;;
    *.test) return 0 ;;
  esac
  # From here on, only bare dotted-IPv4 hosts (digits and dots only) may match
  # the numeric range branches below. This keeps registrable domains like
  # 127.0.0.1.evil.com or 10.evil.com from matching a leading-octet glob.
  case "$host" in
    *[!0-9.]*) return 1 ;;
  esac
  case "$host" in
    0.0.0.0) return 0 ;;
    127.*) return 0 ;;
    10.*) return 0 ;;
    192.168.*) return 0 ;;
    172.1[6-9].*|172.2[0-9].*|172.3[0-1].*) return 0 ;;
  esac
  return 1
}
