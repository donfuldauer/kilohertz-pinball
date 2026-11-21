#!/bin/sh
# S10 req 3: stack needs per engine/tier (harness/stack_need.py) for the deepest Lua sandbox
# cases (S2's list), the sim fixture and the S4 rules depth-counter probes.
# usage: stack_all.sh [engine:tier ...]   -> runs/stack/needs.txt (appends)
set -u
S10=${WEB_SPIKES}/S10
O=$S10/runs/stack; mkdir -p "$O"
ETS=${*:-"v8:default jsc:default jsc:ipint jsc:bbq jsc:omg jsc:nojit sm160:default sm160:baseline sm160:ion sm153:default"}
PROBES="sandbox:cstack_pcall_nest sandbox:cstack_pcall_nest_err sandbox:cstack_tostring sandbox:cstack_sort sandbox:cstack_gsub sandbox:cstack_concat_meta sandbox:cstack_lt_meta sandbox:cstack_index sandbox:pattern_depth sandbox:deep_nesting_load sandbox:deep_recursion sandbox:deep_recursion_pcall sandbox:baseline fixture rules:wasmi-instr-depth rules:jit-depth"
for et in $ETS; do
  e=${et%:*}; t=${et#*:}
  python3 -I "$S10/harness/stack_need.py" "$e" "$t" $PROBES | tee -a "$O/needs.txt"
done
