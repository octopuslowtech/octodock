#!/bin/sh

read -r first || exit 1

emit() { printf '%s\n' "$1"; }
drain() { while read -r _; do :; done; }

init='{"type":"system","subtype":"init","cwd":"/tmp","session_id":"T-1","tools":["shell_command"],"mcp_servers":[],"agent_mode":"low"}'
prompt='{"type":"user","message":{"role":"user","content":[{"type":"text","text":"prompt"}]},"parent_tool_use_id":null,"session_id":"T-1"}'
tool_use='{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"tool_use","id":"TU-1","name":"shell_command","input":{"command":"echo hi","workdir":"/tmp"}}],"stop_reason":"tool_use","usage":{"input_tokens":0,"cache_creation_input_tokens":100,"cache_read_input_tokens":0,"output_tokens":5}},"parent_tool_use_id":null,"session_id":"T-1"}'
tool_result='{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"TU-1","content":"hi","is_error":false}]},"parent_tool_use_id":null,"session_id":"T-1"}'

say() {
  emit "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"$1\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":1,\"cache_creation_input_tokens\":2,\"cache_read_input_tokens\":3,\"output_tokens\":4,\"max_tokens\":1000}},\"parent_tool_use_id\":null,\"session_id\":\"T-1\"}"
}

finish() {
  drain
  emit "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"result\":\"$1\",\"session_id\":\"T-1\"}"
}

case "$first" in

*scenario:happy*)
  emit "$init"
  emit "$prompt"
  emit "$tool_use"
  emit "$tool_result"
  say pong
  finish pong
  ;;

*scenario:steer*)
  emit "$init"
  emit "$prompt"
  emit "$tool_use"
  read -r steer || exit 1
  case "$steer" in *'"steer":true'*) ;; *) exit 1 ;; esac
  emit "$tool_result"
  emit "$prompt"
  say steered
  finish steered
  ;;

*scenario:args*)
  emit "$init"
  say "$*"
  finish args
  ;;

*scenario:hang*)
  emit "$init"
  exec sleep 30
  ;;

*scenario:error*)
  emit "$init"
  emit '{"type":"result","subtype":"error_during_execution","is_error":true,"error":"boom","session_id":"T-1"}'
  ;;

esac
