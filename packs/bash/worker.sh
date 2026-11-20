#!/bin/sh
# Meld T2 worker for Bash/POSIX shell, conformance level L1.
# Protocol: spec/t2-protocol.md. See README.md for judgment calls.
#
# JUDGMENT CALL: this is a POSIX sh script, but it uses python3's *standard
# library* (already present in the project environment, no installs) purely as
# a JSON/base64 line codec. Writing a correct JSON string decoder in portable
# sh is not practical; the executed language is still /bin/sh, and no
# third-party runtime, package or toolchain installation is needed.
set -u

PROTOCOL='meld-t2/1'
LEVEL='L1'

tmpdir=$(mktemp -d) || exit 1
trap 'rm -rf "$tmpdir"' 0 1 2 3 15

# Decode one newline-delimited JSON request into five base64 fields:
# id, method, code, region_id, params.protocol. "-" means the empty string
# (base64 output never contains "-", so tab splitting stays unambiguous).
PY_DECODE='import base64,json,sys
r=json.loads(sys.stdin.readline())
p=r.get("params") or {}
enc=lambda v: base64.b64encode(str(v).encode("utf-8")).decode("ascii") or "-"
print("\t".join(enc(x) for x in [r.get("id",0),r.get("method",""),p.get("code",""),p.get("region_id",""),p.get("protocol","")]))'

PY_ID='import base64,sys
rid=base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "0"
sys.stdout.write(str(int(rid) if rid.lstrip("-").isdigit() else rid))'

PY_DEC='import base64,sys
sys.stdout.write(base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "")'

PY_B64_FILE='import base64,sys
sys.stdout.write(base64.b64encode(open(sys.argv[1],"rb").read()).decode("ascii"))'

PY_B64_TEXT='import base64,sys
sys.stdout.write(base64.b64encode(sys.argv[1].encode("utf-8")).decode("ascii"))'

PY_EMIT_INIT='import base64,json,sys
rid=base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "0"
rid=int(rid) if rid.lstrip("-").isdigit() else rid
sys.stdout.write(json.dumps({"id":rid,"result":{"protocol":"meld-t2/1","level":"L1","caps":["stdout"]}},separators=(",",":"))+"\n")'

PY_EMIT_EXEC='import base64,json,sys
rid=base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "0"
rid=int(rid) if rid.lstrip("-").isdigit() else rid
raw=open(sys.argv[2],"rb").read().decode("utf-8","replace")
sys.stdout.write(json.dumps({"id":rid,"result":{"stdout":raw.splitlines(),"exports":[]}},separators=(",",":"),ensure_ascii=False)+"\n")'

PY_EMIT_ERROR='import base64,json,sys
rid=base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "0"
rid=int(rid) if rid.lstrip("-").isdigit() else rid
code=base64.b64decode(sys.argv[2]).decode("utf-8")
msg=base64.b64decode(sys.argv[3]).decode("utf-8","replace")
region=base64.b64decode(sys.argv[4]).decode("utf-8") if sys.argv[4] != "-" else ""
sys.stdout.write(json.dumps({"id":rid,"error":{"code":code,"message":msg,"source":{"lang":"bash","region_id":region}}},separators=(",",":"),ensure_ascii=False)+"\n")'

PY_EMIT_OK='import base64,json,sys
rid=base64.b64decode(sys.argv[1]).decode("utf-8") if sys.argv[1] != "-" else "0"
rid=int(rid) if rid.lstrip("-").isdigit() else rid
sys.stdout.write(json.dumps({"id":rid,"result":{"ok":True}},separators=(",",":"))+"\n")'

dec() { python3 -c "$PY_DEC" "$1"; }
b64_file() { python3 -c "$PY_B64_FILE" "$1"; }
b64_text() { python3 -c "$PY_B64_TEXT" "$1"; }

emit_error() {
  # $1 id_b64, $2 code, $3 message_b64, $4 region_b64
  python3 -c "$PY_EMIT_ERROR" "$1" "$(b64_text "$2")" "$3" "$4"
}

tab=$(printf '\t')
while IFS= read -r line; do
  [ -n "$line" ] || continue
  fields=$(printf '%s\n' "$line" | python3 -c "$PY_DECODE" 2>"$tmpdir/codec.err")
  if [ -z "$fields" ]; then
    emit_error '-' 'E_PROTOCOL' "$(b64_file "$tmpdir/codec.err")" '-'
    continue
  fi

  old_ifs=$IFS
  IFS="$tab"
  set -- $fields
  IFS=$old_ifs

  id_b64=$1
  method=$(dec "$2")
  code=$(dec "$3")
  region_b64=$4
  region=$(dec "$4")
  protocol=$(dec "$5")

  case "$method" in
    initialize)
      if [ "$protocol" != "$PROTOCOL" ]; then
        emit_error "$id_b64" 'E_PROTOCOL' "$(b64_text "expected $PROTOCOL, got $protocol")" '-'
        break
      fi
      python3 -c "$PY_EMIT_INIT" "$id_b64"
      ;;
    execute)
      # JUDGMENT CALL: each region runs in a fresh shell (L1 is stateless),
      # with stdin closed so region code cannot consume protocol lines. Stdout
      # is captured into an ordered array; stderr is forwarded by the host.
      : > "$tmpdir/out"
      : > "$tmpdir/err"
      sh -c "$code" </dev/null >"$tmpdir/out" 2>"$tmpdir/err"
      status=$?
      if [ "$status" -ne 0 ]; then
        if [ -s "$tmpdir/err" ]; then
          message_b64=$(b64_file "$tmpdir/err")
        else
          message_b64=$(b64_text "shell command exited with status $status")
        fi
        emit_error "$id_b64" 'RuntimeError' "$message_b64" "$region_b64"
      else
        python3 -c "$PY_EMIT_EXEC" "$id_b64" "$tmpdir/out"
      fi
      ;;
    shutdown)
      python3 -c "$PY_EMIT_OK" "$id_b64"
      break
      ;;
    *)
      # L1 declares no value bridge; fetch/import/call/drop are unsupported.
      emit_error "$id_b64" 'E_PROTOCOL' "$(b64_text "unknown or unsupported method $method at level $LEVEL")" '-'
      ;;
  esac
done
